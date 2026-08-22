//! A short-lived cache for movement transitions within one terrain snapshot.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;

use openshard_protocol::direction::Direction;
use openshard_protocol::wire::Graphic;
use openshard_protocol::world::Point;

use crate::{LandTile, Terrain, Tile, step_from};

/// Counts collected by [`CachedTerrain`] for one route query.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TransitionCacheStats {
    /// Calls served by an entry already computed for this terrain snapshot.
    pub hits: usize,
    /// Calls delegated to the wrapped terrain and retained as a new entry.
    pub misses: usize,
    /// Number of distinct directional transitions retained.
    pub entries: usize,
}

/// Cache exact neighbouring movement answers for one route query.
///
/// This deliberately has no invalidation API: construct one around the real
/// terrain and another around a doors-open terrain, then discard both when the
/// route/frame ends.  That makes a changed door, item, or mobile unable to
/// reuse an answer from an older snapshot.
pub struct CachedTerrain<'a> {
    terrain: &'a dyn Terrain,
    /// One compact entry per source position, rather than one hash entry per
    /// directed edge.  A source height is part of the key: staircases and
    /// stacked floors can answer differently from the same `(x, y)`.
    transitions: RefCell<HashMap<Point, Directions>>,
    stats: RefCell<TransitionCacheStats>,
}

/// The eight answers that can leave one point.
///
/// `known` distinguishes an unchecked direction from a checked-but-blocked
/// direction; `allowed` makes the common refusal case a bit test.  Landing
/// points remain whole values rather than only z offsets because `Terrain`'s
/// contract permits an implementation to resolve a step somewhere unexpected.
#[derive(Clone, Copy, Debug, Default)]
struct Directions {
    known: u8,
    allowed: u8,
    landings: [Option<Point>; 8],
}

impl fmt::Debug for CachedTerrain<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CachedTerrain")
            .field("stats", &self.stats())
            .finish_non_exhaustive()
    }
}

impl<'a> CachedTerrain<'a> {
    #[must_use]
    pub fn new(terrain: &'a dyn Terrain) -> Self {
        Self {
            terrain,
            transitions: RefCell::new(HashMap::new()),
            stats: RefCell::new(TransitionCacheStats::default()),
        }
    }

    #[must_use]
    pub fn stats(&self) -> TransitionCacheStats {
        let mut stats = *self.stats.borrow();
        stats.entries = self
            .transitions
            .borrow()
            .values()
            .map(|directions| directions.known.count_ones() as usize)
            .sum();
        stats
    }

    fn direction_to(from: Point, to: Point) -> Option<Direction> {
        Direction::ALL
            .into_iter()
            .find(|&direction| step_from(from, direction) == Some(to))
    }

    const fn bit(direction: Direction) -> u8 {
        1 << direction.to_bits()
    }
}

impl Terrain for CachedTerrain<'_> {
    fn can_step(&self, from: Point, to: Point) -> Option<Point> {
        let Some(direction) = Self::direction_to(from, to) else {
            return self.terrain.can_step(from, to);
        };
        let bit = Self::bit(direction);
        let cached = self.transitions.borrow().get(&from).and_then(|directions| {
            if directions.known & bit == 0 {
                return None;
            }
            if directions.allowed & bit == 0 {
                return Some(None);
            }
            Some(directions.landings[direction.to_bits() as usize])
        });
        if let Some(answer) = cached {
            self.stats.borrow_mut().hits += 1;
            return answer;
        }
        let answer = self.terrain.can_step(from, to);
        let mut transitions = self.transitions.borrow_mut();
        let directions = transitions.entry(from).or_default();
        directions.known |= bit;
        if answer.is_some() {
            directions.allowed |= bit;
        }
        directions.landings[direction.to_bits() as usize] = answer;
        self.stats.borrow_mut().misses += 1;
        answer
    }

    fn ground_z(&self, tile: Tile) -> Option<i8> {
        self.terrain.ground_z(tile)
    }
    fn land_tile(&self, tile: Tile) -> Option<LandTile> {
        self.terrain.land_tile(tile)
    }
    fn statics_at(&self, tile: Tile, out: &mut Vec<(Graphic, i8)>) {
        self.terrain.statics_at(tile, out);
    }
    fn stand_z(&self, tile: Tile, near_z: i32) -> Option<i32> {
        self.terrain.stand_z(tile, near_z)
    }
    fn spawn_z(&self, tile: Tile, near_z: i32) -> Option<i32> {
        self.terrain.spawn_z(tile, near_z)
    }
    fn can_fit(&self, tile: Tile, z: i32, height: i32) -> bool {
        self.terrain.can_fit(tile, z, height)
    }
    fn land_is_water(&self, tile: Tile) -> bool {
        self.terrain.land_is_water(tile)
    }
    fn sight_clear(&self, from: Point, to: Point) -> bool {
        self.terrain.sight_clear(from, to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{OpenWorld, find_path, step_allowed};

    struct CountingTerrain(RefCell<usize>);

    impl Terrain for CountingTerrain {
        fn can_step(&self, _from: Point, to: Point) -> Option<Point> {
            *self.0.borrow_mut() += 1;
            Some(to)
        }
    }

    #[test]
    fn caches_a_directional_transition_once() {
        let terrain = CountingTerrain(RefCell::new(0));
        let cached = CachedTerrain::new(&terrain);
        let from = Point::new(10, 10, 0);
        let to = Point::new(11, 10, 0);
        let north = Point::new(10, 9, 0);
        assert_eq!(cached.can_step(from, to), Some(to));
        assert_eq!(cached.can_step(from, to), Some(to));
        assert_eq!(cached.can_step(from, north), Some(north));
        assert_eq!(*terrain.0.borrow(), 2);
        assert_eq!(
            cached.stats(),
            TransitionCacheStats {
                hits: 1,
                misses: 2,
                entries: 2
            }
        );
    }

    #[test]
    fn cache_instances_never_share_answers() {
        let terrain = CountingTerrain(RefCell::new(0));
        let from = Point::new(10, 10, 0);
        let to = Point::new(11, 10, 0);
        let real = CachedTerrain::new(&terrain);
        let doors_open = CachedTerrain::new(&terrain);
        real.can_step(from, to);
        doors_open.can_step(from, to);
        assert_eq!(*terrain.0.borrow(), 2);
    }

    #[test]
    fn diagonal_checks_reuse_both_flanks_and_the_landing() {
        let terrain = CountingTerrain(RefCell::new(0));
        let cached = CachedTerrain::new(&terrain);
        let from = Point::new(10, 10, 0);
        let diagonal = Direction::NorthEast;
        assert_eq!(step_allowed(&cached, from, diagonal), step_from(from, diagonal));
        assert_eq!(step_allowed(&cached, from, diagonal), step_from(from, diagonal));
        assert_eq!(*terrain.0.borrow(), 3, "two flanks and one landing");
        assert_eq!(cached.stats().hits, 3);
        assert_eq!(cached.stats().misses, 3);
        assert_eq!(cached.stats().entries, 3);
    }

    #[test]
    fn cached_searches_keep_the_same_route_and_reuse_transitions() {
        let from = Point::new(10, 10, 0);
        let to = Point::new(20, 17, 0);
        let expected = find_path(&OpenWorld, from, to, 600);
        let cached = CachedTerrain::new(&OpenWorld);
        assert_eq!(find_path(&cached, from, to, 600), expected);
        assert_eq!(find_path(&cached, from, to, 600), expected);
        assert!(cached.stats().hits > 0);
    }
}
