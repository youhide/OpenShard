//! A* pathfinding over the terrain a mobile walks.
//!
//! # Why the AI needs this and greedy stepping does not do
//!
//! A creature that just steps *toward* its quarry — the direction of the straight
//! line — walks into the first wall between them and sticks there, shuffling
//! against it. That is what Sphere's pursuit does, and it is why its monsters feel
//! broken. ServUO plans a route; this does too, and improves on it in two cheap
//! ways: the search is bounded so it can never stall the tick, and it refuses to
//! cut the corner of a wall (a diagonal step is only taken when both tiles beside
//! it are open), so a path never clips through a building's edge.
//!
//! # A pure function over the map
//!
//! [`find_path`] takes a [`Terrain`] and two points and returns the steps between
//! them, or `None` when there is no route within its node budget. It touches no
//! world state and rolls no dice, so it is deterministic — the same map and the
//! same endpoints plan the same path, which is what keeps a replay's monsters on
//! the same trail. Walkability, height and reach are the terrain's to judge
//! (`can_step` already encodes climb and slope); this only decides *where* to try.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::collections::hash_map::Entry;
use std::sync::OnceLock;
use std::time::Instant;

use openshard_map::grid::Tile;
use openshard_map::overlay::Cover;
use openshard_protocol::direction::Direction;
use openshard_protocol::world::Point;
use rustc_hash::FxHashMap;

use crate::footing::Footing;
use crate::navigation::Region;
use crate::walk::steps_out_of;

/// The work one query may do, counted rather than clocked.
///
/// **A search is bounded by its node budget and by nothing else.** What this
/// exists for is a query that is *many* searches — [`find_long_path`] runs two
/// region floods and up to nine refinement passes over a corridor, each with the
/// same per-search budget — where the thing that used to stop the whole from
/// running away was a 50 ms wall clock, read once per node expansion inside
/// every one of them.
///
/// A clock in there was wrong three times over. It made the answer depend on
/// what else the machine was doing, in a tick
/// [`architecture.md`](../../../../docs/architecture.md) calls deterministic and
/// replayable — the same shard, the same inputs, a different route under load.
/// It made four tests green alone and red together. And it cost 6.5% of a
/// search: `clock_gettime` was the only syscall in the hot loop and one of the
/// eight hottest symbols in a profile of it.
///
/// The ceiling it replaces is the same ceiling, in the unit the budgets beside
/// it are already written in: node expansions.
///
/// [`find_long_path`]: crate::find_long_path
pub(crate) struct Effort {
    left:  usize,
    spent: usize,
}

impl Effort {
    /// A query allowed `nodes` expansions across every search it makes.
    pub(crate) fn of(nodes: usize) -> Self {
        Self {
            left:  nodes,
            spent: 0,
        }
    }

    /// What the query has expanded so far, over every search and flood in it.
    ///
    /// Diagnostic, and the reading the ceiling above it is set from: a number of
    /// nodes is a thing a bench can quote and a machine cannot move.
    pub(crate) fn spent(&self) -> usize {
        self.spent
    }

    /// What one search inside this query may finalise: its own budget, or
    /// whatever the query has left, whichever is less.
    pub(crate) fn allowance(&self, budget: usize) -> usize {
        budget.min(self.left)
    }

    /// Whether the query has spent everything it was given. A caller that has
    /// work left to hand out stops here.
    pub(crate) fn spent_out(&self) -> bool {
        self.left == 0
    }

    /// Charge the query for work already done.
    ///
    /// A search charges what it finalised; a region flood charges the places it
    /// expanded. Both are the same unit, which is what makes one wallet able to
    /// hold a query made of both.
    pub(crate) fn spend(&mut self, nodes: usize) {
        self.left = self.left.saturating_sub(nodes);
        self.spent += nodes;
    }
}

/// How far the search is allowed to trust its own estimate over the cost it has
/// actually paid.
///
/// A\* ranks a candidate by `g + h`, and the reason it can promise the shortest
/// route is that `h` never overshoots. Multiplying `h` by more than one breaks
/// that promise on purpose: the frontier stops spreading sideways along the
/// plateau of equally-good detours and drives at the goal instead, which is
/// **fewer nodes for a route that may be longer**. The bound is the classic one
/// — a route this finds is at most the weight times the shortest — and it holds
/// because a finalised place is never reopened here.
///
/// **A ratio and not a float**, because the tick is replayable: `h * 5 / 4` is
/// the same number on every machine and in every build, and `h as f32 * 1.25`
/// is not quite.
///
/// Two ship, and which one a call names is the difference between *walking* and
/// *measuring*: a body's own route is planned at [`Self::PLANNING`], and
/// anything that has to be able to compare two answers — a baked edge cost, a
/// probe, a test that means "the shortest" — asks at [`Self::EXACT`]. There is
/// no third, and `map_path_probe`'s `--weight` is how a fourth would be argued
/// for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Weight {
    numerator:   u32,
    denominator: u32,
}

impl Weight {
    /// The heuristic taken at its word: the shortest route, and A\*'s promise
    /// intact.
    ///
    /// What the *graph* is built at, and what a measurement is taken against.
    /// A baked edge cost is a statement about the facet — the corridor picks
    /// between hops by comparing them — so it may not be the length of a route
    /// that merely happens to be short enough.
    pub const EXACT: Self = Self {
        numerator:   1,
        denominator: 1,
    };

    /// What a **body's own route** is planned at: a quarter more walking
    /// accepted, in exchange for a search that reaches a quarter more of the
    /// map inside the same node budget.
    ///
    /// **Measured, over 33,280 destinations from two origins on facet 0**, at
    /// both shipped budgets. Against the exact search:
    ///
    /// | | castle, 400 | castle, 600 | open country, 400 |
    /// |---|---|---|---|
    /// | destinations reached | +24.7% | +23.3% | +6.8% |
    /// | total route length | +0.20% | +0.32% | +0.19% |
    /// | routes that got longer at all | 195 of 2,828 | 352 of 3,223 | 613 of 10,143 |
    /// | the worst one | +2 steps | +4 steps | +3 steps |
    /// | arrivals *lost* | 0 | 0 | 0 |
    ///
    /// The two origins are the two shapes of ground and they answer different
    /// halves: at the castle the budget is what refuses, and a weight spends it
    /// better; in open country the *map* is what refuses — water and cliff — and
    /// the weight saturates by 9/8 because there is nothing left to reach.
    ///
    /// **Why not more.** 3/2 reaches +36% at the castle for +0.34%, and 2/1
    /// reaches +49% for +2.36% and a worst route 12 steps long over the
    /// shortest. 5/4 is where the cost is still under a quarter of a percent
    /// and no single route is stretched past a step or two — a player who
    /// clicks across a courtyard cannot see it, and one who could would be
    /// looking at 2/1.
    ///
    /// **Why not less.** 9/8 buys 14% for 0.09%, which is the same trade at
    /// half scale; the castle's numbers say the curve has not turned by 5/4.
    ///
    /// The bound holds whatever the ground: a route planned at this weight is
    /// never longer than five quarters of the shortest one.
    pub const PLANNING: Self = Self {
        numerator:   5,
        denominator: 4,
    };

    /// A weight of `numerator / denominator`.
    ///
    /// # Panics
    ///
    /// If the denominator is zero, or the ratio is below one — a weight under
    /// one is not a cheaper search but a slower one, and asking for it is a
    /// caller with the fraction upside down.
    #[must_use]
    pub fn of(numerator: u32, denominator: u32) -> Self {
        assert!(denominator > 0, "a weight is a ratio, not a division by zero");
        assert!(
            numerator >= denominator,
            "a weight under one only makes the search wander further; {numerator}/{denominator}"
        );
        Self {
            numerator,
            denominator,
        }
    }

    /// The ratio this weight is, for a report that has to name it.
    ///
    /// `map_path_probe` keeps the pair it parsed beside the weight it built,
    /// with a note saying "a ratio is not recoverable from the search parameter
    /// it becomes". This is that recovery: a route journal writes `5/4` into
    /// its session line without a second copy of the number travelling beside
    /// the weight to say what it was.
    #[must_use]
    pub const fn ratio(self) -> (u32, u32) {
        (self.numerator, self.denominator)
    }

    /// This weight applied to one estimate.
    ///
    /// `u64` in the middle: a facet's widest heuristic is 16 bits and the
    /// numerator is a small integer, so nothing here can overflow in practice —
    /// but the product of two `u32`s is the one place where "in practice" would
    /// have to be argued rather than seen.
    fn applied(self, h: u32) -> u32 {
        if self == Self::EXACT {
            return h;
        }
        (u64::from(h) * u64::from(self.numerator) / u64::from(self.denominator)) as u32
    }
}

/// How thoroughly one exact search inside a long query looks: how many standing
/// places it may finalise, and how far it may trust its own estimate.
///
/// The two travel together everywhere a corridor is refined — every hop of it is
/// asked the same way — and separately they are eight arguments where the
/// functions carrying them already have seven of their own.
///
/// Not on [`find_path`]'s own signature, where the two are named at the call and
/// there is nothing to bundle them for.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Rigour {
    pub(crate) budget: usize,
    pub(crate) weight: Weight,
}

/// One entry on the open list, ordered so [`BinaryHeap`] with [`Reverse`] pops
/// the cheapest-and-straightest first. See the note where this is pushed for
/// why the Manhattan distance is in the ordering at all.
///
/// **One integer, not six fields**, and the field order is the reason it can be:
/// the ranking is `f`, then `h`, then the Manhattan distance, then the node's
/// own coordinates to make equal candidates deterministic — which is exactly
/// what comparing one number whose fields are laid out most-significant-first
/// does. The six-field version derived the same order as a chain of up to six
/// compares and branches, and a heap runs that chain ~log₂(600) ≈ 9 times per
/// push: `BinaryHeap::push` was 12.9% of a profile of the search.
///
/// The widths are what each field can actually reach: `f` is a cost bounded by
/// the budget plus a heuristic bounded by the facet, the Manhattan distance is
/// twice that, and 24 bits holds either with room to spare.
///
/// ```text
/// 111        87        63        39      23      7   0
///  | f (24) | h (24) | man (24) | x (16) | y (16) | z (8) |
/// ```
///
/// **The height is one of those coordinates and not a payload.** A candidate is
/// a place to stand, so the same tile reached at two heights is two entries —
/// see [`PathNodeKey`]. It is stored with its sign bit flipped, which is what
/// makes an unsigned compare of the whole word rank it the way `i8` does.
///
/// **The bottom forty bits are [`PathNodeKey`] with that one bit flipped**, and
/// that is by construction rather than by coincidence: the key is what the
/// table is keyed by and the coordinates are what the entry ranks by, so
/// packing them twice would be packing the same three numbers twice. A push
/// takes the key it has just made, and a pop hands one straight back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct OpenEntry(u128);

/// The width of each of the three cost fields, as a mask.
const FIELD: u32 = (1 << 24) - 1;

/// The one bit between a [`PathNodeKey`]'s height byte and an [`OpenEntry`]'s:
/// `i8::MIN` becomes 0 and `i8::MAX` becomes 255, so an unsigned compare of the
/// whole word ranks the height the way `i8` does. The key wants no such thing —
/// it is an identity, and only ever compared for equality.
const HEIGHT_BIAS: u64 = 0x80;

impl OpenEntry {
    /// A candidate standing at `at`, ranked by `f`, then `h`, then `manhattan`.
    ///
    /// The three costs are clamped rather than masked: a value past 24 bits
    /// would otherwise wrap into the field above it and rank a candidate as the
    /// *cheapest* thing on the list. Nothing on a UO facet comes near — the
    /// widest heuristic a 65,536-tile map can produce is 16 bits — so this is a
    /// guard rather than a case, and the failure it guards against is a search
    /// that visibly wanders rather than one that crashes.
    fn new(f: u32, h: u32, manhattan: u32, at: PathNodeKey) -> Self {
        let f = u128::from(f.min(FIELD));
        let h = u128::from(h.min(FIELD));
        let manhattan = u128::from(manhattan.min(FIELD));
        Self((f << 88) | (h << 64) | (manhattan << 40) | u128::from(at.0 ^ HEIGHT_BIAS))
    }

    /// The place this candidate stands on, and its distance to the goal — the
    /// two things a pop reads back out.
    ///
    /// The place comes back as the *key*, which is what the pop does with it:
    /// one table lookup, one comparison against the goal, and only then — for
    /// the node that is actually being expanded — a [`PathNodeKey::place`].
    fn place(self) -> (PathNodeKey, u32) {
        let at = PathNodeKey(((self.0 & 0xff_ffff_ffff) as u64) ^ HEIGHT_BIAS);
        let h = ((self.0 >> 64) & u128::from(FIELD)) as u32;
        (at, h)
    }
}

/// Plan a walk from `from` to the place `to` names, at most `budget` nodes
/// explored.
///
/// Returns the sequence of steps — the caller usually takes the first each beat
/// and re-plans as the quarry moves — or `None` if the goal is unreachable within
/// the budget (blocked, or simply too far for the cap). An empty `Vec` means
/// `from` already stands on the goal.
///
/// **A destination is a place and not a tile**, so a body under a floor asked
/// to be on it is given the route round the staircase, and a body on a floor it
/// cannot get to is refused. Which place a caller's z names is
/// [`destination_place`]'s to resolve, and it is generous about it — see there.
///
/// The budget bounds the cost: a search that would explore more than `budget`
/// standing places gives up rather than spend the tick. A few hundred is ample
/// for moving about a town; open-world roaming would want caching, not a bigger
/// cap.
///
/// *Is there a way there* is what this answers, and `None` is a real answer to
/// it. A caller that has to do something with an unreachable destination —
/// a move order a player gave, which is owed a body that walks up to whatever
/// stops it — wants [`find_path_toward`], which is the same search read for that
/// question instead.
/// The weight is named at the call and never assumed: a body's own route is
/// planned at [`Weight::PLANNING`], and anything measuring the map — a baked
/// edge cost, a probe, a test that means *the shortest* — asks at
/// [`Weight::EXACT`].
#[must_use]
pub fn find_path(
    footing: &Footing<'_>,
    from: Point,
    to: Point,
    budget: usize,
    weight: Weight,
) -> Option<Vec<Direction>> {
    let started = debug_enabled().then(Instant::now);
    let search = explore(footing, from, to, budget, weight, None);
    debug_slow("find_path", from, to, budget, started, &search);
    search.arrived.then_some(search.route)
}

/// Plan a walk that gets as close to `to` as the ground allows, whether or not
/// it can be reached.
///
/// The same search as [`find_path`], answered for a *move order* rather than for
/// a question. A player who clicks on a wall, on the far bank of a river, or
/// into a room whose only door is shut has asked to go **there**, and the body a
/// client owes them walks up to whatever stops it and stands — which is the
/// reference client's own answer, and the only one that does not have this end
/// sending steps it already knows the shard will refuse.
///
/// The route ends on the reached place closest to the goal, by the same Chebyshev
/// measure the search steers by, with the shorter route winning a tie. `None`
/// when nothing reachable is any closer than where the body already stands —
/// walled in, or already as close as it gets; either way there is nothing worth
/// walking. An empty `Vec` still means `from` stands on the goal. The measure
/// is planar, so a floor above the goal's own column counts as having arrived
/// over it.
///
/// The budget is the same bound and cuts the same way: what comes back is then
/// the closest tile the search got to before it ran out, which is a route in the
/// goal's direction rather than a refusal.
#[must_use]
pub fn find_path_toward(
    footing: &Footing<'_>,
    from: Point,
    to: Point,
    budget: usize,
    weight: Weight,
) -> Option<Vec<Direction>> {
    let started = debug_enabled().then(Instant::now);
    let search = explore(footing, from, to, budget, weight, None);
    debug_slow("find_path_toward", from, to, budget, started, &search);
    (search.arrived || !search.route.is_empty()).then_some(search.route)
}

/// The same search as [`find_path`], reported rather than answered.
///
/// One search, both readings, and what it cost: `arrived` is what
/// [`find_path`] gates its `Some` on, `route` is what [`find_path_toward`]
/// hands back when it did not, and `explored` and `exit` are what neither of
/// them can say. Nothing on the walking path calls this — a body is owed a
/// route, and asking through here would only make it drop the same fields
/// again. It exists for the measurement the node budgets are argued from.
///
/// The weight is named at the call here and nowhere else the search is asked
/// from: [`Weight::EXACT`] is what the two entry points above pass and what
/// every caller in the tree gets, and this is where a probe can price the
/// alternative.
#[must_use]
pub fn search_path(
    footing: &Footing<'_>,
    from: Point,
    to: Point,
    budget: usize,
    weight: Weight,
) -> PathSearch {
    explore(footing, from, to, budget, weight, None)
}

/// What one search found, before either entry point above reads it — and what
/// it cost to find out.
///
/// [`find_path`] and [`find_path_toward`] are this read two ways, and both
/// throw the cost away: a body wants a route, not a node count. A *probe* wants
/// the opposite, because the node budgets are set from it — 400 for server AI,
/// 600 for a client plan — and until something reports `explored` those numbers
/// can only be guessed at. [`search_path`] is that reading.
#[derive(Clone, Debug)]
pub struct PathSearch {
    /// Whether [`Self::route`] ends on the goal itself — the tile *and* the
    /// height, since [`PathNodeKey`] is what the search compares.
    pub arrived:  bool,
    /// The steps to the goal, or — where it was never reached — to the place that
    /// came closest to it. Empty when nothing the search reached bettered its own
    /// start: with `arrived`, that is a body already standing on the goal, and
    /// without it, a body with nowhere closer to go.
    pub route:    Vec<Direction>,
    /// Number of standing places removed from the open list and finalised.
    ///
    /// Not tiles: a column with two floors in it can be finalised twice. On
    /// Britannia that is 0.6% of columns, so the count is what it always was
    /// everywhere else.
    pub explored: usize,
    /// Standing places the search wrote down: every one it finalised, plus the
    /// frontier it reached and never popped.
    ///
    /// [`Self::explored`] is what the budget counts; this is what the table
    /// holds, and the two differ by the frontier. It is here because the table
    /// is now reserved up front — see `visit_capacity` — and a reservation
    /// argued from a guess is a rehash nobody sees.
    pub written:  usize,
    /// Why the search stopped.  This is diagnostic only; the two entry points
    /// above keep the established `Option<Vec<Direction>>` contract.
    pub exit:     SearchExit,
}

/// Why a search stopped looking.
///
/// The two that are not [`Self::Goal`] are two different failures wearing one
/// `None`: a walled-in start, and a budget that ran out mid-route. Which one it
/// was is what decides whether a bigger budget would have helped.
///
/// **There is no third failure any more.** A search used to be able to outlive
/// a wall clock, which is a failure that says nothing about the map and
/// everything about the machine; the ceiling is counted now — see [`Effort`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchExit {
    /// The goal tile was popped: the route arrives.
    Goal,
    /// The open list emptied. Nothing reachable is left to try, so no budget
    /// would have changed the answer.
    Exhausted,
    /// `budget` standing places were finalised and the goal was not among them.
    ///
    /// Inside a long query the budget that ran out may be the *query's* rather
    /// than this search's — one wallet is shared by every search a corridor
    /// takes — and from in here the two are the same fact: there was no more
    /// work to spend.
    Budget,
}

/// A* over the terrain, once, with both answers kept: the goal's own route, and
/// the best approach to it.
///
/// The approach costs the search nothing — every candidate already carries its
/// distance to the goal, since that is what A* orders the open list by, so
/// keeping the best one seen is a comparison per pop. What it buys is that
/// "there is no way" and "here is how far the way goes" come out of *one*
/// search over one terrain, and cannot disagree about which tiles were reachable.
pub(crate) fn find_path_within(
    footing: &Footing<'_>,
    from: Point,
    to: Point,
    rigour: Rigour,
    effort: &mut Effort,
    within: Option<Region>,
) -> Option<Vec<Direction>> {
    let search = search(footing, from, to, rigour, effort, within);
    search.arrived.then_some(search.route)
}

pub(crate) fn find_path_toward_within(
    footing: &Footing<'_>,
    from: Point,
    to: Point,
    rigour: Rigour,
    effort: &mut Effort,
    within: Option<Region>,
) -> Option<Vec<Direction>> {
    let search = search(footing, from, to, rigour, effort, within);
    (search.arrived || !search.route.is_empty()).then_some(search.route)
}

/// One search inside a query that is paying for several: bounded by whichever
/// of the two budgets is smaller, and charged for what it actually finalised.
///
/// The wallet is read *here* and not in the loop below, which is the point of
/// counting rather than clocking: a limit that cannot change while a search runs
/// is a limit the hot path never has to look at.
fn search(
    footing: &Footing<'_>,
    from: Point,
    to: Point,
    rigour: Rigour,
    effort: &mut Effort,
    within: Option<Region>,
) -> PathSearch {
    let allowance = effort.allowance(rigour.budget);
    let search = explore(footing, from, to, allowance, rigour.weight, within);
    effort.spend(search.explored);
    search
}

fn explore(
    footing: &Footing<'_>,
    from: Point,
    to: Point,
    budget: usize,
    weight: Weight,
    within: Option<Region>,
) -> PathSearch {
    // The destination as a *place*, which is what the search compares against:
    // see `destination_place` for why the caller's own z is not it.
    let goal = destination_place(footing, from, to);
    let start = from;
    if start == goal {
        return PathSearch {
            arrived:  true,
            route:    Vec::new(),
            explored: 0,
            written:  0,
            exit:     SearchExit::Goal,
        };
    }

    // Pack the standing place into one integer. Besides making the key cheaper
    // to hash, this lets FxHash use its integer fast path. The key carries the
    // whole landing point, so nothing has to remember where a node was.
    //
    // **One table, and not three.** What A* knows about a place is its cost, the
    // step it was reached by and whether it has been finalised, and keying three
    // containers by the same key asked FxHash the same question three times: a
    // neighbour cost four hash lookups (`closed`, `cost` read, `cost` write,
    // `came_from` write) where one `entry` answers all of them, and a pop cost
    // two where `get_mut` answers both. See `Visit`.
    //
    // **Sized up front**, because the node budget is the bound on how much of it
    // can ever be finalised: growing from empty put three `reserve_rehash`es
    // among the hottest symbols of a profile — 5.8% of a search spent copying
    // tables that could have been born big enough.
    let mut visited: FxHashMap<PathNodeKey, Visit> =
        FxHashMap::with_capacity_and_hasher(visit_capacity(budget), rustc_hash::FxBuildHasher);
    // The tuple's third field is a tie-breaker, not a second admissible heuristic:
    // Chebyshev alone cannot tell a straight cardinal line from a route that
    // drifts off it and back — both cost the same eight-way step count. Manhattan
    // distance can: it only grows on a diagonal that moves you off an axis you
    // will have to close later, so among equal-`f` candidates it is smaller for
    // the one that stayed straight. Breaking ties by it does not change *whether*
    // a shortest path is found (see the cost check below), only which one among
    // several equally short routes A* settles on — the one the client's own map
    // asked for a straight walk on stays a straight walk.
    let mut open: BinaryHeap<Reverse<OpenEntry>> = BinaryHeap::with_capacity(open_capacity(budget));

    // The two places the search compares against, packed once. Everything in
    // the loop below speaks keys — the table is keyed by one, the goal test is
    // one `u64` compare, and the frontier's own bookkeeping carries one — so
    // the only place a standing place is unpacked back into coordinates is
    // where [`steps_out_of`] is asked for them.
    let start_key = node_key(start);
    let goal_key = node_key(goal);
    visited.insert(
        start_key,
        Visit {
            cost:   0,
            from:   None,
            closed: false,
        },
    );
    let (h0, manhattan) = estimate(from, to);
    // The weight lands on `f` and nowhere else. The `h` the entry carries stays
    // the true Chebyshev distance, because that is what the approach half of the
    // search reports — *how close did the way get* is a measurement of the map,
    // not a preference of the search.
    open.push(Reverse(OpenEntry::new(
        weight.applied(h0),
        h0,
        manhattan,
        start_key,
    )));
    // How close a finalised node has come to the goal, and what it cost to get
    // there. Seeded with the start, so a node takes the place only by being
    // *strictly* closer: walking to somewhere no nearer than here is not getting
    // closer, it is only walking. Among equally close nodes the cheaper route
    // wins, which is the same "do not wander" preference the tie-break above is.
    //
    // The measure is `heuristic`'s, which is planar — so a body that got onto
    // the roof over the goal has "arrived" as far as the approach is concerned.
    // That is the right answer for a move order (it is as close as the ground
    // goes) and it is why `arrived` is a separate field rather than `h == 0`.
    let mut closest = (h0, 0, start_key);
    let mut exit = SearchExit::Exhausted;
    // Finalised places, which is what the budget counts and what `PathSearch`
    // reports. Kept rather than read off a `closed` set's length, because the
    // one table now holds the frontier as well.
    let mut explored = 0;
    while let Some(Reverse(entry)) = open.pop() {
        // The key *is* the place, so the popped entry is the whole node and
        // nothing has to be looked up to find out where it was.
        let (key, h) = entry.place();
        // One lookup answers both halves of a pop: whether a cheaper pop already
        // finalised this place, and — where it did not — what the route to it
        // cost. Two tables asked that as two hashes of the same key.
        let seen = visited
            .get_mut(&key)
            .expect("a candidate is written to the table before it is pushed");
        if seen.closed {
            continue;
        }
        seen.closed = true;
        let here_cost = seen.cost;
        explored += 1;
        if key == goal_key {
            return PathSearch {
                arrived: true,
                route: reconstruct(&visited, start_key, goal_key),
                explored,
                written: visited.len(),
                exit: SearchExit::Goal,
            };
        }
        // The first pop of a node is its cheapest, so this is its final cost —
        // see `Visit::closed`.
        if (h, here_cost) < (closest.0, closest.1) {
            closest = (h, here_cost, key);
        }
        if explored > budget {
            exit = SearchExit::Budget;
            break;
        }
        // The whole node at once — `steps_out_of` and not eight `step_allowed`
        // calls, which would resolve the tile being stepped off eight times and
        // each cardinal neighbour twice. Same answers, in the same order:
        // `step_allowed` is one slot of this. See `docs/world/evidence/2026-08-25-the-span-layer.md`'s
        // N3 for what the difference is worth.
        //
        // `steps_out_of`, not `can_step` per neighbour: a diagonal may not clip
        // a wall corner, and that half of the rule is not the terrain's to
        // answer — see `step_allowed` for why it is shared with the shard and
        // the client rather than restated here.
        let steps = steps_out_of(footing, key.place());
        // The region bound, where there is one: a search inside one region of
        // the navigation graph may not wander out of it and back. It was a
        // decorating terrain that answered `None` outside the rectangle, which
        // made staying put a property of the *ground* rather than of the
        // question being asked.
        //
        // **Asked once a node rather than once a neighbour.** Whether a search
        // is bounded at all cannot change while it runs, so the `Option` is
        // opened out here and the eight landings are filtered in one pass; the
        // loop below then sees the same array either way. Testing it inside the
        // loop made an always-`None` read of a 12-byte `Option` 1.9% of a
        // profile of the whole probe.
        let steps = match within {
            Some(region) => steps.map(|landing| landing.filter(|&at| region.contains(at))),
            None => steps,
        };
        let next_cost = here_cost + 1;
        // The array is in `Direction::ALL`'s own order — `steps_out_of` fills it
        // by `to_bits`, which is the discriminant — so zipping is the same
        // pairing the index was, without the bounds check the index carried.
        for (&dir, landing) in Direction::ALL.iter().zip(steps) {
            let Some(landing) = landing else {
                continue;
            };
            let landing_key = node_key(landing);
            // One `entry` for what used to be four separate lookups of one key:
            // is the place closed, what did it cost before, and the two writes
            // that answer "cheaper this way". A neighbour is eight per node, so
            // this is where the three tables cost the most.
            match visited.entry(landing_key) {
                Entry::Occupied(mut seen) => {
                    let seen = seen.get_mut();
                    if seen.closed || next_cost >= seen.cost {
                        continue;
                    }
                    seen.cost = next_cost;
                    seen.from = Some((key, dir));
                }
                Entry::Vacant(slot) => {
                    slot.insert(Visit {
                        cost:   next_cost,
                        from:   Some((key, dir)),
                        closed: false,
                    });
                }
            }
            let (h, manhattan) = estimate(landing, to);
            open.push(Reverse(OpenEntry::new(
                next_cost + weight.applied(h),
                h,
                manhattan,
                landing_key,
            )));
        }
    }
    // The goal was never popped: what there is to say is how far the way got.
    PathSearch {
        arrived: false,
        route: match closest.2 == start_key {
            true => Vec::new(),
            false => reconstruct(&visited, start_key, closest.2),
        },
        explored,
        written: visited.len(),
        exit,
    }
}

/// Whether slow-query diagnostics were asked for, read from the environment
/// once.
///
/// It gates the clock as well as the printing: timing a search that will not be
/// reported is two `clock_gettime` calls a query for nothing, and this crate has
/// just finished taking the last clock out of the loop inside it.
pub(crate) fn debug_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("OPENSHARD_PATH_DEBUG").is_some())
}

/// Optional slow-query diagnostics.  Pathfinding is used from the render
/// thread, so diagnostics are opt-in and only print after the query is over;
/// they never add per-node logging to a search.
///
/// `started` is `None` when nothing asked for them — see [`debug_enabled`].
fn debug_slow(
    kind: &str,
    from: Point,
    to: Point,
    budget: usize,
    started: Option<Instant>,
    search: &PathSearch,
) {
    let Some(started) = started else {
        return;
    };
    let elapsed = started.elapsed();
    let threshold = std::env::var("OPENSHARD_PATH_DEBUG_MS")
        .ok()
        .and_then(|value| value.parse::<u128>().ok())
        .unwrap_or(10);
    if elapsed.as_millis() < threshold {
        return;
    }
    eprintln!(
        "path-debug kind={kind} from=({}, {}, {}) to=({}, {}, {}) budget={budget} elapsed_ms={:.3} explored={} exit={:?} arrived={} route_steps={}",
        from.x,
        from.y,
        from.z,
        to.x,
        to.y,
        to.z,
        elapsed.as_secs_f64() * 1_000.0,
        search.explored,
        search.exit,
        search.arrived,
        search.route.len(),
    );
}

/// Walk the parent chain from the goal back to the start, collecting the steps in
/// travel order.
///
/// **The chain is over places, not tiles**, so a route that leaves a column and
/// comes back to it higher up is a chain that visits `(x, y)` twice and
/// terminates all the same — see [`PathNodeKey`]. Keyed by tile it would either
/// stop short at the first visit or never have been found at all.
fn reconstruct(
    visited: &FxHashMap<PathNodeKey, Visit>,
    start: PathNodeKey,
    goal: PathNodeKey,
) -> Vec<Direction> {
    let mut steps = Vec::new();
    let mut at = goal;
    while at != start {
        let (parent, dir) = visited[&at]
            .from
            .expect("only the start is reached by no step, and the walk stops there");
        steps.push(dir);
        at = parent;
    }
    steps.reverse();
    steps
}

/// What the search knows about one standing place.
///
/// The three tables A* wants keyed by the same place — the cost to reach it, the
/// step it was reached by, and whether it has been finalised — held as one
/// record, so a lookup answers all three. See where `visited` is declared for
/// what asking them separately cost.
struct Visit {
    /// The cheapest route to this place found so far. Final once
    /// [`Self::closed`], since the first pop of a place is its cheapest.
    cost:   u32,
    /// The place stepped from, and the step that landed here.
    ///
    /// **`None` is the start**, which no step reached — the one place in the
    /// table whose absence of a parent is a fact about the search rather than a
    /// value nobody filled in, and what terminates [`reconstruct`].
    from:   Option<(PathNodeKey, Direction)>,
    /// Whether the place has been finalised: popped off the open list with its
    /// cost settled, counted against the budget, and expanded.
    closed: bool,
}

/// How many places a search of `budget` finalised nodes writes down.
///
/// Every finalised place is one, and so is every place the frontier reached and
/// never popped — which is what makes the table bigger than the budget it is
/// reserved from.
///
/// **Twice the budget, and the sample says why.** `map_path_probe` prints the
/// ratio; over 37,248 destinations from each of two origins on facet 0, at both
/// shipped budgets:
///
/// | | castle (1363, 1600) | open country (1500, 1900) |
/// |---|---|---|
/// | budget 400, median | 485 | 506 |
/// | budget 400, peak | 653 (×1.63) | 736 (×1.84) |
/// | budget 600, median | 687 | 710 |
/// | budget 600, peak | 862 (×1.43) | 1,041 (×1.73) |
///
/// Nothing in 149,000 searches passes ×1.84. Over-reserving costs 24 bytes a
/// slot on a table that lives for one search; under-reserving costs a rehash
/// that copies every slot already in it, which is what the three tables this
/// replaced were doing on every search — 5.8% of a profile.
fn visit_capacity(budget: usize) -> usize {
    budget.saturating_mul(2)
}

/// How many candidates the open list is born able to hold.
///
/// The heap holds the frontier *plus* its own stale entries: a place is pushed
/// again every time a cheaper route to it is found, and the older entry is left
/// to be popped and skipped. The frontier alone — the same sample's places
/// written down, less the ones finalised — peaks at 0.84 of the budget, so the
/// whole budget is the round number above it that also covers a share of the
/// duplicates. At 16 bytes an entry that is 9.6 KiB at the client's 600.
fn open_capacity(budget: usize) -> usize {
    budget
}

/// One standing place, packed for A*'s hash tables: a tile **and** the height
/// a body's feet are at on it.
///
/// **Not a tile, and that is the whole of `navigation_spans.md`'s N3b.** A
/// column can hold more than one place to stand — a bridge over a road, the
/// first floor of a house over its ground floor — and keying the search by the
/// tile collapses them into one slot in `closed`. What that costs is not a
/// missed route but a wrong answer: the column is closed the first time it is
/// reached at *some* height, so the other height can never be entered, and a
/// destination on it is reported as reached.
///
/// **The discriminator is the height and not a span index**, which is where
/// this departs from the plan that asked for `(x, y, span)`. A span is the
/// map's own surface, and the surfaces this search lands on are not all the
/// map's: a house, a ship's deck and a placed stair are the
/// [`Overlay`](openshard_map::overlay::Overlay)'s, and
/// [`climbed`](crate::walk) picks them with no span to name. The height is what
/// both layers already agree to speak in — it is what a landing *is* — so it is
/// what the key is made of. Two surfaces of one column at one height are one
/// place to stand, which is the right identity anyway.
///
/// Forty bits of a `u64`, laid out `x`, `y`, `z`, so hashing stays FxHash's
/// integer fast path and no coordinate can be silently truncated the way a
/// `u32` would have to truncate one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct PathNodeKey(u64);

impl PathNodeKey {
    /// The place the key names, unpacked.
    ///
    /// A pop does this **once**, for the node it is about to expand — the
    /// lookup, the goal test and the frontier's own bookkeeping all speak keys,
    /// and only [`steps_out_of`] wants coordinates back.
    fn place(self) -> Point {
        Point::new((self.0 >> 24) as u16, (self.0 >> 8) as u16, self.0 as u8 as i8)
    }
}

#[inline]
fn node_key(at: Point) -> PathNodeKey {
    // `as u8` on the height: the bits, not the value — this is an identity, and
    // the only thing asked of it is that two different places differ.
    PathNodeKey((u64::from(at.x) << 24) | (u64::from(at.y) << 8) | u64::from(at.z as u8))
}

/// The node a caller's destination names: the place to stand at `to`'s column
/// nearest the height it was asked at.
///
/// **A destination is a point and a node is a place, and the two are not the
/// same thing.** Every caller has a z and almost none of them has the *exact*
/// z of the surface it means: the coarse graph's nodes carry the land's height
/// under a bridge they mean the deck of, a probe sweeps a neighbourhood at the
/// height its origin stands at, and a client's click carries whatever the tile
/// it hit was drawn at. Before N3b every one of those arrived, because arrival
/// compared tiles and threw the height away. Comparing nodes without resolving
/// first would swap that lie for a refusal, which is not the repair.
///
/// So the height is resolved against what is actually there — the map's spans
/// and the live world's surfaces, the same union a landing can come from —
/// exactly the way [`Overlay::surface_at`](openshard_map::overlay::Overlay::surface_at)
/// resolves one, with the tie broken by the lower surface so the answer does
/// not depend on which layer was read first.
///
/// **The start's own column offers the start's own height**, because a body
/// standing there is proof that there is somewhere to stand: a search from a
/// place to itself answers "you are here" rather than hunting for a surface the
/// world does not list. A column nothing names a surface on keeps the height it
/// was asked at, and nothing will ever land on it — a goal in a wall, which is
/// the refusal it always was.
///
/// **Public because an order outlives the search that answers it.** A client
/// holding a move order has to be able to say *the body has arrived*, and that
/// comparison is against the place the destination named rather than the height
/// the click carried — a table's top is 26 and the art the cursor hit is at 20.
/// Resolving once, where the order is taken, is what keeps that test and this
/// search agreeing; resolving it a second way would be the second policy
/// [`design_frame_assembly.md`](../../../../docs/render/design_frame_assembly.md) is about. Idempotent, so a place that
/// has already been through here goes through again unchanged.
#[must_use]
pub fn destination_place(footing: &Footing<'_>, from: Point, to: Point) -> Point {
    let tile = Tile::new(to.x, to.y);
    let wanted = i32::from(to.z);
    let mapped = footing
        .map
        .iter()
        .flat_map(|map| map.spans().surfaces(to.x, to.y))
        .map(|span| span.stand_z);
    let placed = footing
        .overlay
        .surfaces_at(tile)
        .map(Cover::surface)
        // A surface no body could be represented as standing on is not a place
        // a landing can name either — `landing` drops the same ones.
        .filter_map(|z| i8::try_from(z).ok());
    let standing = (Tile::new(from.x, from.y) == tile).then_some(from.z);
    let z = mapped
        .chain(placed)
        .chain(standing)
        .min_by_key(|&z| ((i32::from(z) - wanted).abs(), z))
        .unwrap_or(to.z);
    Point::new(to.x, to.y, z)
}

/// The two distances a candidate is ranked by, over the one pair of deltas both
/// are made of.
///
/// The first is the **heuristic**: Chebyshev, the count of eight-way steps,
/// which never overshoots the true cost and so keeps A* optimal. The second is
/// the open list's **tie-breaker**: Manhattan distance, which — unlike
/// Chebyshev — is not blind to a detour off an axis the goal is on. See the note
/// on `open`.
///
/// One function because they are one subtraction: `max(dx, dy)` and `dx + dy`
/// read the same two deltas, and asking for them separately took the difference
/// of each coordinate twice, eight times a node.
fn estimate(from: Point, to: Point) -> (u32, u32) {
    let dx = i32::from(from.x).abs_diff(i32::from(to.x));
    let dy = i32::from(from.y).abs_diff(i32::from(to.y));
    (dx.max(dy), dx + dy)
}

#[cfg(test)]
mod tests {
    use openshard_map::overlay::{
        Cover,
        Doors,
        Overlay,
    };

    use super::*;

    /// The packed open-list entry ranks candidates exactly the way the six
    /// fields it replaced did.
    ///
    /// The order **is** the search's answer among equally short routes, so a
    /// packing that ranks differently does not fail — it silently returns a
    /// different route than the one the client's own map was drawn for. The
    /// oracle is the tuple the struct used to be, compared over every pair of a
    /// sample that puts each field either side of a boundary in turn, negative
    /// heights included.
    #[test]
    fn a_packed_entry_ranks_the_way_its_fields_do() {
        let sample = [
            (0, 0, 0, Point::new(0, 0, 0)),
            (0, 0, 0, Point::new(0, 0, -128)),
            (0, 0, 0, Point::new(0, 0, 127)),
            (0, 0, 0, Point::new(0, 1, -1)),
            (0, 0, 0, Point::new(1, 0, 0)),
            (0, 0, 1, Point::new(0, 0, 0)),
            (0, 1, 0, Point::new(0, 0, 0)),
            (1, 0, 0, Point::new(0, 0, 0)),
            (7, 3, 9, Point::new(1363, 1600, 30)),
            (7, 3, 9, Point::new(1363, 1600, -30)),
            (8, 3, 9, Point::new(1363, 1600, 30)),
            (FIELD, FIELD, FIELD, Point::new(u16::MAX, u16::MAX, 127)),
        ];
        for &(f, h, manhattan, at) in &sample {
            for &(other_f, other_h, other_manhattan, other_at) in &sample {
                let fields = (f, h, manhattan, at.x, at.y, at.z);
                let other_fields = (
                    other_f,
                    other_h,
                    other_manhattan,
                    other_at.x,
                    other_at.y,
                    other_at.z,
                );
                assert_eq!(
                    OpenEntry::new(f, h, manhattan, node_key(at)).cmp(&OpenEntry::new(
                        other_f,
                        other_h,
                        other_manhattan,
                        node_key(other_at)
                    )),
                    fields.cmp(&other_fields),
                    "{fields:?} against {other_fields:?}"
                );
            }
            // And a pop reads back what was pushed: the place is an identity the
            // search looks the node up by, so a bit lost here is a lookup that
            // misses. Both halves of that identity are checked — the key the
            // table is looked up by, and the coordinates it unpacks to — because
            // the entry now carries the key rather than the point.
            let (key, popped) = OpenEntry::new(f, h, manhattan, node_key(at)).place();
            assert_eq!((key, popped), (node_key(at), h));
            assert_eq!(key.place(), at);
        }
    }

    /// The exact weight is the estimate untouched — the one thing that must be
    /// true for `Weight::EXACT` to be a *name* for today's search rather than a
    /// multiplication by one that rounds.
    #[test]
    fn the_exact_weight_leaves_every_estimate_alone() {
        for h in [0, 1, 7, 96, 65_535, FIELD] {
            assert_eq!(Weight::EXACT.applied(h), h, "the exact weight moved {h}");
        }
        // And a real one multiplies, rounding down — so a weight never claims a
        // hop is further than it is by more than it is wide.
        assert_eq!(Weight::of(5, 4).applied(100), 125);
        assert_eq!(Weight::of(5, 4).applied(3), 3);
        assert_eq!(Weight::of(2, 1).applied(65_535), 131_070);
    }

    #[test]
    #[should_panic = "a weight is a ratio"]
    fn a_weight_is_not_a_division_by_zero() {
        let _ = Weight::of(1, 0);
    }

    #[test]
    #[should_panic = "only makes the search wander further"]
    fn a_weight_under_one_is_the_fraction_upside_down() {
        let _ = Weight::of(4, 5);
    }

    /// A weight trades nodes for route length, and both halves of that trade are
    /// checked here: what comes back is a route a body can actually walk, and it
    /// is no longer than the weight times the shortest one.
    ///
    /// The bound is what makes the weight a decision rather than a gamble — a
    /// caller that says `5/4` is saying it will accept a quarter more walking,
    /// and nothing may exceed that however the wall is shaped.
    #[test]
    fn a_weighted_search_walks_a_real_route_inside_its_own_bound() {
        let world = walled_world(12, 8, 12, 8);
        let footing = over(&world);
        let from = Point::new(10, 10, 0);
        let to = Point::new(14, 10, 0);
        let shortest = find_path(&footing, from, to, 1000, Weight::EXACT).expect("there is a way around");
        for (numerator, denominator) in [(9_usize, 8_usize), (5, 4), (3, 2), (2, 1)] {
            let weight = Weight::of(numerator as u32, denominator as u32);
            let search = search_path(&footing, from, to, 1000, weight);
            assert!(
                search.arrived,
                "whether there is a way around does not depend on the weight"
            );
            // Walked by the shipped step rule rather than by arithmetic over the
            // directions: a search that looks at fewer nodes must not plan a step
            // nobody may take.
            let mut at = from;
            for dir in &search.route {
                at = crate::step_allowed(&footing, at, *dir)
                    .expect("the weighted search planned a step nobody may take");
            }
            assert_eq!((at.x, at.y), (14, 10), "it still arrives");
            assert!(
                search.route.len() * denominator <= shortest.len() * numerator,
                "{numerator}/{denominator} stretched {} steps past its bound over the shortest {}",
                search.route.len(),
                shortest.len(),
            );
        }
    }

    /// Ground with nothing on it: no map, so no floor and no walls, and the
    /// overlay is the only thing that can refuse a step.
    fn open_world() -> Overlay {
        Overlay::default()
    }

    /// A vertical wall of impassable tiles the path must go around — a column
    /// at `wall_x` spanning `wall_from..=wall_to`, with one opening.
    fn walled_world(wall_x: u16, wall_from: u16, wall_to: u16, opening_y: u16) -> Overlay {
        let mut overlay = Overlay::default();
        for y in wall_from..=wall_to {
            if y != opening_y {
                overlay.set(Tile::new(wall_x, y), vec![Cover::blocking(0, 20)]);
            }
        }
        overlay
    }

    /// The ground a search is asked over, with `overlay` the only thing on it.
    fn over(overlay: &Overlay) -> Footing<'_> {
        Footing::new(None, overlay, Doors::AsTheyStand)
    }

    /// Walk a path from a start and return where it lands.
    fn walk_path(from: Point, path: &[Direction]) -> Point {
        let mut at = from;
        for dir in path {
            let (dx, dy) = dir.step();
            at = Point::new((i32::from(at.x) + dx) as u16, (i32::from(at.y) + dy) as u16, at.z);
        }
        at
    }

    #[test]
    fn a_path_on_open_ground_is_the_shortest_length() {
        // Three tiles east: the route is three steps (any equal-cost mix of due-east
        // and diagonals), never a detour.
        let from = Point::new(10, 10, 0);
        let path = find_path(
            &over(&open_world()),
            from,
            Point::new(13, 10, 0),
            100,
            Weight::EXACT,
        )
        .expect("open ground is always reachable");
        assert_eq!(path.len(), 3, "no detour on open ground");
        let end = walk_path(from, &path);
        assert_eq!((end.x, end.y), (13, 10), "it arrives");
    }

    #[test]
    fn already_at_the_goal_is_an_empty_path() {
        let path = find_path(
            &over(&open_world()),
            Point::new(5, 5, 0),
            Point::new(5, 5, 0),
            100,
            Weight::EXACT,
        )
        .unwrap();
        assert!(path.is_empty());
    }

    #[test]
    fn a_path_routes_around_a_wall() {
        // A wall at x=12 from y=8..12 with an opening at y=8; a route from the west
        // to the east must detour up to the gap rather than push into the wall.
        let world = walled_world(12, 8, 12, 8);
        let from = Point::new(10, 10, 0);
        let path = find_path(&over(&world), from, Point::new(14, 10, 0), 1000, Weight::EXACT)
            .expect("there is a way around");
        // It must never stand on a blocked tile, and reach the far side.
        let mut at = from;
        for dir in &path {
            at = walk_path(at, std::slice::from_ref(dir));
            assert!(
                crate::can_step(&over(&world), at, at).is_some(),
                "the path steps onto a blocked tile at {},{}",
                at.x,
                at.y
            );
        }
        assert_eq!((at.x, at.y), (14, 10), "it arrived on the far side");
    }

    #[test]
    fn an_unreachable_goal_within_budget_is_none() {
        // Seal the goal behind a wall with no opening: no route exists.
        let world = walled_world(12, 0, u16::MAX, u16::MAX);
        assert!(
            find_path(
                &over(&world),
                Point::new(10, 10, 0),
                Point::new(14, 10, 0),
                500,
                Weight::EXACT
            )
            .is_none()
        );
    }

    /// The move order's answer to that same sealed wall: not "no", but "this
    /// far". The route stops against the wall on the goal's own row — a body
    /// walked up to what stopped it, which is what a player who clicked over
    /// there is owed and what stops the client sending steps into it.
    #[test]
    fn an_unreachable_goal_is_walked_toward_until_the_ground_runs_out() {
        let world = walled_world(12, 0, u16::MAX, u16::MAX);
        let from = Point::new(10, 10, 0);
        let to = Point::new(14, 10, 0);
        let path = find_path_toward(&over(&world), from, to, 500, Weight::EXACT)
            .expect("there is somewhere closer to stand");
        let end = walk_path(from, &path);
        assert_eq!(
            (end.x, end.y),
            (11, 10),
            "the walk stops on the last tile before the wall, on the goal's own row"
        );
    }

    /// And when there is nowhere closer — the body is already against the thing
    /// it was sent to — there is nothing to walk, which is not the same as a
    /// route of length zero. Answering with one step "toward" it would be a step
    /// away from the goal dressed up as progress.
    #[test]
    fn nothing_closer_to_stand_is_nothing_to_walk() {
        let world = walled_world(12, 0, u16::MAX, u16::MAX);
        // Standing against the wall, sent to the tile on its far side.
        let from = Point::new(11, 10, 0);
        assert_eq!(
            find_path_toward(&over(&world), from, Point::new(12, 10, 0), 500, Weight::EXACT),
            None
        );
    }

    /// A mezzanine over `(5, 5)` and one step up to it from the east, so the
    /// only way onto it leaves its column and comes back.
    ///
    /// No map: with nothing under it a body keeps the height it walks in at,
    /// so every z in here is one the overlay put there. `Cover::standing` is
    /// not climbable, so its surface is its base and it is met at its base —
    /// two of them, one `MAX_STEP_UP` apart, are a staircase with one tread.
    fn a_mezzanine() -> Overlay {
        let mut overlay = Overlay::default();
        overlay.set(Tile::new(5, 5), vec![Cover::standing(4, 0)]);
        overlay.set(Tile::new(6, 5), vec![Cover::standing(2, 0)]);
        overlay
    }

    /// One column, two places to stand, and the route between them is a loop —
    /// `navigation_spans.md`'s N3b, and the whole reason a node is not a tile.
    ///
    /// Nothing in UO moves up in place: the step rule changes a body's height
    /// as a *consequence* of a horizontal step. So getting from under the
    /// mezzanine onto it means stepping off `(5, 5)` and back onto it higher,
    /// which a search keyed by the tile can never do — the column is closed by
    /// the first pop and the return is forbidden by the closed set rather than
    /// by the world.
    #[test]
    fn a_route_out_of_a_column_and_back_reaches_its_other_floor() {
        let world = a_mezzanine();
        let footing = over(&world);
        let under = Point::new(5, 5, 0);
        let over_head = Point::new(5, 5, 4);
        let route =
            find_path(&footing, under, over_head, 100, Weight::EXACT).expect("the mezzanine has a way up");
        assert_eq!(
            route,
            vec![Direction::East, Direction::West],
            "the way up is out of the column and back over it"
        );
        // Walked by the shipped step rule rather than by the test's own
        // arithmetic: the route has to be one a body could actually take.
        let mut at = under;
        for dir in &route {
            at = crate::step_allowed(&footing, at, *dir).expect("the search planned a step nobody may take");
        }
        assert_eq!(at, over_head, "the loop comes home one storey up");
    }

    /// And the lie it replaces: with the tread taken away there is no way up,
    /// and the answer is a refusal rather than an empty route.
    ///
    /// **This is what the tile key answered before N3b.** `start == goal`
    /// compared tiles, so a body under a floor asking to be on it was told it
    /// had arrived and given nothing to walk — the caller then stood still
    /// believing it was upstairs.
    #[test]
    fn a_floor_with_no_way_up_is_refused_and_not_answered_with_an_empty_route() {
        let mut world = a_mezzanine();
        world.set(Tile::new(6, 5), Vec::new());
        assert_eq!(
            find_path(
                &over(&world),
                Point::new(5, 5, 0),
                Point::new(5, 5, 4),
                100,
                Weight::EXACT
            ),
            None,
            "an unreachable floor of one's own column is not an arrival"
        );
    }

    /// The height a caller asks at is resolved to the place that is there.
    ///
    /// Almost no caller has the exact z of the surface it means — the coarse
    /// graph carries the land's height under a deck, a probe sweeps at its
    /// origin's height — so the goal is the nearest standing place to what was
    /// asked, and *which* place that is decides the answer. Asking nearer the
    /// mezzanine than the ground is asking for the mezzanine.
    #[test]
    fn a_goals_height_names_the_nearest_place_to_stand() {
        let world = a_mezzanine();
        let footing = over(&world);
        let under = Point::new(5, 5, 0);
        assert_eq!(
            find_path(&footing, under, Point::new(5, 5, 3), 100, Weight::EXACT),
            Some(vec![Direction::East, Direction::West]),
            "three of four units up is the mezzanine, and it is climbed"
        );
        assert_eq!(
            find_path(&footing, under, Point::new(5, 5, 1), 100, Weight::EXACT),
            Some(Vec::new()),
            "one unit up is the ground the body is already standing on"
        );
    }

    /// A goal that *is* reachable is answered the same way by both, so a caller
    /// that only ever asks for the move order's version never gets a worse route
    /// for it.
    #[test]
    fn a_reachable_goal_is_the_same_route_either_way() {
        let world = walled_world(12, 8, 12, 8);
        let from = Point::new(10, 10, 0);
        let to = Point::new(14, 10, 0);
        assert_eq!(
            find_path_toward(&over(&world), from, to, 1000, Weight::EXACT),
            find_path(&over(&world), from, to, 1000, Weight::EXACT),
            "the approach must not second-guess a route that arrives"
        );
    }
}
