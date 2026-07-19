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
use std::collections::{BinaryHeap, HashMap, HashSet};

use openshard_protocol::{Direction, Point};

use crate::walk::{step_from, Terrain};

/// A tile in the search, keyed by its column and row. Height is carried on the
/// resolved [`Point`], not in the key: two routes reaching one tile at different
/// heights are still the same tile to walk to.
type Tile = (u16, u16);

/// Plan a walk from `from` to the tile of `to`, at most `budget` tiles explored.
///
/// Returns the sequence of steps — the caller usually takes the first each beat
/// and re-plans as the quarry moves — or `None` if the goal is unreachable within
/// the budget (blocked, or simply too far for the cap). An empty `Vec` means
/// `from` already stands on the goal tile.
///
/// The budget bounds the cost: a search that would explore more than `budget`
/// tiles gives up rather than spend the tick. A few hundred is ample for moving
/// about a town; open-world roaming would want caching, not a bigger cap.
#[must_use]
pub fn find_path(
    terrain: &dyn Terrain,
    from: Point,
    to: Point,
    budget: usize,
) -> Option<Vec<Direction>> {
    let goal: Tile = (to.x, to.y);
    let start: Tile = (from.x, from.y);
    if start == goal {
        return Some(Vec::new());
    }

    // The resolved point (with its real z) at each reached tile, the cheapest cost
    // to reach it, and how — the parent tile and the step taken — to rebuild the
    // route. `closed` finalises a tile the first time it is popped: Chebyshev is a
    // consistent heuristic for uniform-cost eight-way movement, so the first pop is
    // the cheapest and a later, staler copy in the heap is skipped.
    let mut point_at: HashMap<Tile, Point> = HashMap::new();
    let mut cost: HashMap<Tile, u32> = HashMap::new();
    let mut came_from: HashMap<Tile, (Tile, Direction)> = HashMap::new();
    let mut closed: HashSet<Tile> = HashSet::new();
    let mut open: BinaryHeap<Reverse<(u32, u32, u16, u16)>> = BinaryHeap::new();

    point_at.insert(start, from);
    cost.insert(start, 0);
    let h0 = heuristic(from, to);
    open.push(Reverse((h0, h0, start.0, start.1)));

    while let Some(Reverse((_f, _h, cx, cy))) = open.pop() {
        let tile = (cx, cy);
        // Skip a tile already finalised by a cheaper pop.
        if !closed.insert(tile) {
            continue;
        }
        if tile == goal {
            return Some(reconstruct(&came_from, start, goal));
        }
        if closed.len() > budget {
            return None;
        }

        let current = point_at[&tile];
        let here_cost = cost[&tile];
        for dir in Direction::ALL {
            let Some(guess) = step_from(current, dir) else {
                continue;
            };
            // A diagonal may not clip a wall corner: both tiles beside it must be
            // steppable too, the same rule the client enforces.
            if is_diagonal(dir) && !corner_open(terrain, current, dir) {
                continue;
            }
            let Some(landing) = terrain.can_step(current, guess) else {
                continue;
            };
            let next: Tile = (landing.x, landing.y);
            if closed.contains(&next) {
                continue;
            }
            let next_cost = here_cost + 1;
            if next_cost >= cost.get(&next).copied().unwrap_or(u32::MAX) {
                continue;
            }
            cost.insert(next, next_cost);
            point_at.insert(next, landing);
            came_from.insert(next, (tile, dir));
            let h = heuristic(landing, to);
            open.push(Reverse((next_cost + h, h, next.0, next.1)));
        }
    }
    None
}

/// Walk the parent chain from the goal back to the start, collecting the steps in
/// travel order.
fn reconstruct(
    came_from: &HashMap<Tile, (Tile, Direction)>,
    start: Tile,
    goal: Tile,
) -> Vec<Direction> {
    let mut steps = Vec::new();
    let mut tile = goal;
    while tile != start {
        let (parent, dir) = came_from[&tile];
        steps.push(dir);
        tile = parent;
    }
    steps.reverse();
    steps
}

/// The remaining distance estimate: Chebyshev, the count of eight-way steps, which
/// never overshoots the true cost and so keeps A* optimal.
fn heuristic(from: Point, to: Point) -> u32 {
    let dx = i32::from(from.x).abs_diff(i32::from(to.x));
    let dy = i32::from(from.y).abs_diff(i32::from(to.y));
    dx.max(dy)
}

/// Whether a direction moves on both axes — a diagonal.
fn is_diagonal(dir: Direction) -> bool {
    let (dx, dy) = dir.step();
    dx != 0 && dy != 0
}

/// Whether both cardinal tiles flanking a diagonal are steppable, so the diagonal
/// does not cut through a wall's corner. The flanks of a diagonal are the two
/// wire directions either side of it (NE lies between N and E).
fn corner_open(terrain: &dyn Terrain, from: Point, diagonal: Direction) -> bool {
    let d = diagonal.to_bits();
    let flanks = [
        Direction::from_bits((d + 7) % 8),
        Direction::from_bits((d + 1) % 8),
    ];
    flanks.iter().all(|&card| {
        step_from(from, card)
            .and_then(|tile| terrain.can_step(from, tile))
            .is_some()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::walk::OpenWorld;

    /// A flat world with a vertical wall of impassable tiles the path must go
    /// around — every tile walkable except a column at `wall_x` spanning `gap`
    /// rows, with one opening.
    struct WalledWorld {
        wall_x: u16,
        wall_from: u16,
        wall_to: u16,
        opening_y: u16,
    }
    impl Terrain for WalledWorld {
        fn can_step(&self, _from: Point, to: Point) -> Option<Point> {
            let blocked = to.x == self.wall_x
                && to.y >= self.wall_from
                && to.y <= self.wall_to
                && to.y != self.opening_y;
            if blocked {
                None
            } else {
                Some(to)
            }
        }
    }

    /// Walk a path from a start and return where it lands.
    fn walk_path(from: Point, path: &[Direction]) -> Point {
        let mut at = from;
        for dir in path {
            let (dx, dy) = dir.step();
            at = Point::new(
                (i32::from(at.x) + dx) as u16,
                (i32::from(at.y) + dy) as u16,
                at.z,
            );
        }
        at
    }

    #[test]
    fn a_path_on_open_ground_is_the_shortest_length() {
        // Three tiles east: the route is three steps (any equal-cost mix of due-east
        // and diagonals), never a detour.
        let from = Point::new(10, 10, 0);
        let path = find_path(&OpenWorld, from, Point::new(13, 10, 0), 100)
            .expect("open ground is always reachable");
        assert_eq!(path.len(), 3, "no detour on open ground");
        let end = walk_path(from, &path);
        assert_eq!((end.x, end.y), (13, 10), "it arrives");
    }

    #[test]
    fn already_at_the_goal_is_an_empty_path() {
        let path = find_path(&OpenWorld, Point::new(5, 5, 0), Point::new(5, 5, 0), 100).unwrap();
        assert!(path.is_empty());
    }

    #[test]
    fn a_path_routes_around_a_wall() {
        // A wall at x=12 from y=8..12 with an opening at y=8; a route from the west
        // to the east must detour up to the gap rather than push into the wall.
        let world = WalledWorld {
            wall_x: 12,
            wall_from: 8,
            wall_to: 12,
            opening_y: 8,
        };
        let from = Point::new(10, 10, 0);
        let path =
            find_path(&world, from, Point::new(14, 10, 0), 1000).expect("there is a way around");
        // It must never stand on a blocked tile, and reach the far side.
        let mut at = from;
        for dir in &path {
            at = walk_path(at, std::slice::from_ref(dir));
            assert!(
                world.can_step(at, at).is_some(),
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
        let world = WalledWorld {
            wall_x: 12,
            wall_from: 0,
            wall_to: u16::MAX,
            opening_y: u16::MAX, // effectively no gap near the route
        };
        assert!(find_path(&world, Point::new(10, 10, 0), Point::new(14, 10, 0), 500).is_none());
    }
}
