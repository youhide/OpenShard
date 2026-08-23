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
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use openshard_map::overlay::Cover;
use openshard_protocol::direction::Direction;
use openshard_protocol::world::Point;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::footing::Footing;
use crate::navigation::Region;
use openshard_map::grid::Tile;

use crate::walk::steps_out_of;

/// How long one search may run before it gives up, whatever its budget says.
///
/// A bound on the *tick*, not on the answer: pathfinding runs on the client's
/// render thread as well as inside the shard's beat, and neither can afford a
/// search that walks a whole facet.
pub const MAX_SEARCH_TIME: Duration = Duration::from_millis(50);

/// One entry on the open list, ordered so [`BinaryHeap`] with [`Reverse`] pops
/// the cheapest-and-straightest first. See the note where this is pushed for
/// why [`Self::manhattan`] is there at all.
///
/// Field order is intentional: the derived ordering ranks `f`, then `h`, then
/// the Manhattan distance, followed by the node's own coordinates only to make
/// equal candidates deterministic.
///
/// **The height is one of those coordinates and not a payload.** A candidate is
/// a place to stand, so the same tile reached at two heights is two entries —
/// see [`PathNodeKey`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct OpenEntry {
    f: u32,
    h: u32,
    manhattan: u32,
    x: u16,
    y: u16,
    z: i8,
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
/// [`goal_node`]'s to resolve, and it is generous about it — see there.
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
#[must_use]
pub fn find_path(footing: &Footing<'_>, from: Point, to: Point, budget: usize) -> Option<Vec<Direction>> {
    let started = Instant::now();
    let search = search(footing, from, to, budget, started + MAX_SEARCH_TIME, None);
    debug_slow("find_path", from, to, budget, started.elapsed(), &search);
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
) -> Option<Vec<Direction>> {
    let started = Instant::now();
    let search = search(footing, from, to, budget, started + MAX_SEARCH_TIME, None);
    debug_slow("find_path_toward", from, to, budget, started.elapsed(), &search);
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
#[must_use]
pub fn search_path(footing: &Footing<'_>, from: Point, to: Point, budget: usize) -> PathSearch {
    search(footing, from, to, budget, Instant::now() + MAX_SEARCH_TIME, None)
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
    pub arrived: bool,
    /// The steps to the goal, or — where it was never reached — to the place that
    /// came closest to it. Empty when nothing the search reached bettered its own
    /// start: with `arrived`, that is a body already standing on the goal, and
    /// without it, a body with nowhere closer to go.
    pub route: Vec<Direction>,
    /// Number of standing places removed from the open list and finalised.
    ///
    /// Not tiles: a column with two floors in it can be finalised twice. On
    /// Britannia that is 0.6% of columns, so the count is what it always was
    /// everywhere else.
    pub explored: usize,
    /// Why the search stopped.  This is diagnostic only; the two entry points
    /// above keep the established `Option<Vec<Direction>>` contract.
    pub exit: SearchExit,
}

/// Why a search stopped looking.
///
/// The three that are not [`Self::Goal`] are three different failures wearing
/// one `None`: a walled-in start, a budget that ran out mid-route, and a search
/// that outlived [`MAX_SEARCH_TIME`]. Which one it was is what decides whether
/// a bigger budget would have helped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchExit {
    /// The goal tile was popped: the route arrives.
    Goal,
    /// The open list emptied. Nothing reachable is left to try, so no budget
    /// would have changed the answer.
    Exhausted,
    /// `budget` standing places were finalised and the goal was not among them.
    Budget,
    /// [`MAX_SEARCH_TIME`] passed inside the search.
    Deadline,
}

/// A* over the terrain, once, with both answers kept: the goal's own route, and
/// the best approach to it.
///
/// The approach costs the search nothing — every candidate already carries its
/// distance to the goal, since that is what A* orders the open list by, so
/// keeping the best one seen is a comparison per pop. What it buys is that
/// "there is no way" and "here is how far the way goes" come out of *one*
/// search over one terrain, and cannot disagree about which tiles were reachable.
pub(crate) fn find_path_until(
    footing: &Footing<'_>,
    from: Point,
    to: Point,
    budget: usize,
    deadline: Instant,
    within: Option<Region>,
) -> Option<Vec<Direction>> {
    let search = search(footing, from, to, budget, deadline, within);
    search.arrived.then_some(search.route)
}

pub(crate) fn find_path_toward_until(
    footing: &Footing<'_>,
    from: Point,
    to: Point,
    budget: usize,
    deadline: Instant,
    within: Option<Region>,
) -> Option<Vec<Direction>> {
    let search = search(footing, from, to, budget, deadline, within);
    (search.arrived || !search.route.is_empty()).then_some(search.route)
}

fn search(
    footing: &Footing<'_>,
    from: Point,
    to: Point,
    budget: usize,
    deadline: Instant,
    within: Option<Region>,
) -> PathSearch {
    // The destination as a *place*, which is what the search compares against:
    // see `goal_node` for why the caller's own z is not it.
    let goal = goal_node(footing, from, to);
    let start = from;
    if start == goal {
        return PathSearch {
            arrived: true,
            route: Vec::new(),
            explored: 0,
            exit: SearchExit::Goal,
        };
    }

    // Pack the standing place into one integer. Besides making the key cheaper
    // to hash, this lets FxHash use its integer fast path. The key carries the
    // whole landing point, so nothing has to remember where a node was.
    let mut cost: FxHashMap<PathNodeKey, u32> = FxHashMap::default();
    let mut came_from: FxHashMap<PathNodeKey, (PathNodeKey, Direction)> = FxHashMap::default();
    let mut closed: FxHashSet<PathNodeKey> = FxHashSet::default();
    // The tuple's third field is a tie-breaker, not a second admissible heuristic:
    // Chebyshev alone cannot tell a straight cardinal line from a route that
    // drifts off it and back — both cost the same eight-way step count. Manhattan
    // distance can: it only grows on a diagonal that moves you off an axis you
    // will have to close later, so among equal-`f` candidates it is smaller for
    // the one that stayed straight. Breaking ties by it does not change *whether*
    // a shortest path is found (see the cost check below), only which one among
    // several equally short routes A* settles on — the one the client's own map
    // asked for a straight walk on stays a straight walk.
    let mut open: BinaryHeap<Reverse<OpenEntry>> = BinaryHeap::new();

    cost.insert(node_key(start), 0);
    let h0 = heuristic(from, to);
    open.push(Reverse(OpenEntry {
        f: h0,
        h: h0,
        manhattan: manhattan(from, to),
        x: start.x,
        y: start.y,
        z: start.z,
    }));
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
    let mut closest = (h0, 0, start);
    let mut exit = SearchExit::Exhausted;
    while let Some(Reverse(OpenEntry {
        h,
        x: cx,
        y: cy,
        z: cz,
        ..
    })) = open.pop()
    {
        if Instant::now() >= deadline {
            exit = SearchExit::Deadline;
            break;
        }
        // The key *is* the place, so the popped entry is the whole node and
        // nothing has to be looked up to find out where it was.
        let current = Point::new(cx, cy, cz);
        // Skip a node already finalised by a cheaper pop.
        if !closed.insert(node_key(current)) {
            continue;
        }
        let explored = closed.len();
        if current == goal {
            return PathSearch {
                arrived: true,
                route: reconstruct(&came_from, start, goal),
                explored,
                exit: SearchExit::Goal,
            };
        }
        let here_cost = cost[&node_key(current)];
        // The first pop of a node is its cheapest, so this is its final cost —
        // see `closed`.
        if (h, here_cost) < (closest.0, closest.1) {
            closest = (h, here_cost, current);
        }
        if closed.len() > budget {
            exit = SearchExit::Budget;
            break;
        }
        // The whole node at once — `steps_out_of` and not eight `step_allowed`
        // calls, which would resolve the tile being stepped off eight times and
        // each cardinal neighbour twice. Same answers, in the same order:
        // `step_allowed` is one slot of this. See `docs/map/navigation_spans.md`'s
        // N3 for what the difference is worth.
        let steps = steps_out_of(footing, current);
        for dir in Direction::ALL {
            // `steps_out_of`, not `can_step` per neighbour: a diagonal may not
            // clip a wall corner, and that half of the rule is not the terrain's
            // to answer — see `step_allowed` for why it is shared with the shard
            // and the client rather than restated here.
            let Some(landing) = steps[dir.to_bits() as usize] else {
                continue;
            };
            // The region bound, where there is one: a search inside one region
            // of the navigation graph may not wander out of it and back. It was
            // a decorating terrain that answered `None` outside the rectangle,
            // which made staying put a property of the *ground* rather than of
            // the question being asked.
            if within.is_some_and(|region| !region.contains(landing)) {
                continue;
            }
            let next = node_key(landing);
            if closed.contains(&next) {
                continue;
            }
            let next_cost = here_cost + 1;
            if next_cost >= cost.get(&next).copied().unwrap_or(u32::MAX) {
                continue;
            }
            cost.insert(next, next_cost);
            came_from.insert(next, (node_key(current), dir));
            let h = heuristic(landing, to);
            open.push(Reverse(OpenEntry {
                f: next_cost + h,
                h,
                manhattan: manhattan(landing, to),
                x: landing.x,
                y: landing.y,
                z: landing.z,
            }));
        }
    }
    // The goal was never popped: what there is to say is how far the way got.
    PathSearch {
        arrived: false,
        route: match closest.2 == start {
            true => Vec::new(),
            false => reconstruct(&came_from, start, closest.2),
        },
        explored: closed.len(),
        exit,
    }
}

/// Optional slow-query diagnostics.  Pathfinding is used from the render
/// thread, so diagnostics are opt-in and only print after the query is over;
/// they never add per-node logging to a search.
fn debug_slow(
    kind: &str,
    from: Point,
    to: Point,
    budget: usize,
    elapsed: std::time::Duration,
    search: &PathSearch,
) {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if !*ENABLED.get_or_init(|| std::env::var_os("OPENSHARD_PATH_DEBUG").is_some()) {
        return;
    }
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
    came_from: &FxHashMap<PathNodeKey, (PathNodeKey, Direction)>,
    start: Point,
    goal: Point,
) -> Vec<Direction> {
    let mut steps = Vec::new();
    let start = node_key(start);
    let mut at = node_key(goal);
    while at != start {
        let (parent, dir) = came_from[&at];
        steps.push(dir);
        at = parent;
    }
    steps.reverse();
    steps
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
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct PathNodeKey(u64);

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
fn goal_node(footing: &Footing<'_>, from: Point, to: Point) -> Point {
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

/// The remaining distance estimate: Chebyshev, the count of eight-way steps, which
/// never overshoots the true cost and so keeps A* optimal.
fn heuristic(from: Point, to: Point) -> u32 {
    let dx = i32::from(from.x).abs_diff(i32::from(to.x));
    let dy = i32::from(from.y).abs_diff(i32::from(to.y));
    dx.max(dy)
}

/// The open list's tie-breaker: Manhattan distance, which — unlike Chebyshev —
/// is not blind to a detour off an axis the goal is on. See the note on `open`.
fn manhattan(from: Point, to: Point) -> u32 {
    let dx = i32::from(from.x).abs_diff(i32::from(to.x));
    let dy = i32::from(from.y).abs_diff(i32::from(to.y));
    dx + dy
}

#[cfg(test)]
mod tests {
    use super::*;
    use openshard_map::overlay::{Cover, Doors, Overlay};

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
        let path = find_path(&over(&open_world()), from, Point::new(13, 10, 0), 100)
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
        let path =
            find_path(&over(&world), from, Point::new(14, 10, 0), 1000).expect("there is a way around");
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
        assert!(find_path(&over(&world), Point::new(10, 10, 0), Point::new(14, 10, 0), 500).is_none());
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
        let path =
            find_path_toward(&over(&world), from, to, 500).expect("there is somewhere closer to stand");
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
            find_path_toward(&over(&world), from, Point::new(12, 10, 0), 500),
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
        let route = find_path(&footing, under, over_head, 100).expect("the mezzanine has a way up");
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
            find_path(&over(&world), Point::new(5, 5, 0), Point::new(5, 5, 4), 100),
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
            find_path(&footing, under, Point::new(5, 5, 3), 100),
            Some(vec![Direction::East, Direction::West]),
            "three of four units up is the mezzanine, and it is climbed"
        );
        assert_eq!(
            find_path(&footing, under, Point::new(5, 5, 1), 100),
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
            find_path_toward(&over(&world), from, to, 1000),
            find_path(&over(&world), from, to, 1000),
            "the approach must not second-guess a route that arrives"
        );
    }
}
