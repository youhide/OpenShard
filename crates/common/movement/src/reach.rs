//! One flood over the step rule, and everyone who needs one.
//!
//! # Why it is one thing
//!
//! A flood over the step rule is how this crate proves anything about ground:
//! a search that refuses is only interesting against an answer to *is there a
//! way at all*, and the only honest answer walks the rule itself. Three of them
//! were written independently — the scene fixture's oracle, `coarse_bench`'s
//! ground truth, and `span_check`'s two-rule comparison — and each walked the
//! eight directions one [`step_allowed`](crate::step_allowed) at a time.
//!
//! **That is the whole expansion asked for eight times and used once.**
//! `step_allowed` is *defined* as one slot of
//! [`steps_out_of`](crate::steps_out_of), which resolves the place being
//! stepped off once and each cardinal neighbour once rather than once as a
//! destination and again as some diagonal's flank. N4 stopped paying it in the
//! bake and `docs/map/navigation_spans.md` filed the rest; this module is the
//! rest, in one place, where a fourth copy cannot be written without noticing
//! that a third already exists.
//!
//! # What it answers
//!
//! **Keyed by tile, not by place.** A tile first reached at one height is
//! marked reached, and a second route arriving at another height is not
//! explored again — so a gallery over a street is one entry and not two. That
//! under-counts and never over-counts, which is the direction an oracle wants:
//! a tile this calls reachable really is one. A flood keyed by *place* is a
//! different structure over a different index — [`NavigationGraph`] has one per
//! region, and the facet-wide one is what
//! `docs/map/navigation_spans.md`'s N5 is waiting on.
//!
//! [`NavigationGraph`]: crate::NavigationGraph
//!
//! # What bounds it
//!
//! The rectangle it was handed, and nothing else. A landing outside it is
//! dropped rather than followed: the step rule refuses a landing off the *map*
//! by itself, but a footing with no map is open ground in every direction, and
//! a flood over one of those has no edge of its own to find.

use std::collections::VecDeque;

use openshard_protocol::world::Point;

use crate::footing::Footing;

/// Every tile one origin can walk to, and the height a body stands at there.
///
/// Dense over the rectangle it was flooded across — one `Option<i8>` per tile,
/// which is 59 MB for a facet and a few hundred bytes for a scene. A whole
/// facet is what the diagnostics ask for and they ask once.
#[derive(Debug)]
pub struct Reach {
    /// How wide the flooded rectangle is, in tiles. The row stride of
    /// [`stood`](Self::stood).
    width:  u32,
    /// How tall it is, in tiles.
    height: u32,
    /// The height a body stands at on each tile, or `None` where the flood
    /// never arrived. Row-major, `width` to a row.
    stood:  Vec<Option<i8>>,
    /// How many tiles were reached, counted as the flood marked them — because
    /// the alternative is a caller counting 29 million `Option`s to print one
    /// percentage.
    count:  usize,
}

impl Reach {
    /// Flood from `origin` over the step rule the world in `footing` implies.
    ///
    /// The origin is in the answer at its own height whether or not anything
    /// could have walked it there: a body is standing where it is standing, and
    /// how it arrived is [`MapTerrain::surface_at`](crate::MapTerrain)'s
    /// question rather than this one.
    ///
    /// # Panics
    ///
    /// If `origin` is outside the rectangle. A flood that starts off its own
    /// map has been handed either the wrong point or the wrong facet, and both
    /// are worth stopping for.
    #[must_use]
    pub fn of(footing: &Footing<'_>, origin: Point, width: u32, height: u32) -> Self {
        Self::by(origin, width, height, |at| crate::steps_out_of(footing, at))
    }

    /// The same flood with the expansion handed in: eight landings by
    /// [`Direction::to_bits`](openshard_protocol::direction::Direction::to_bits),
    /// which is what [`steps_out_of`](crate::steps_out_of) returns.
    ///
    /// For an oracle that must *not* go through the shipped rule. `span_check`
    /// compares two readings of the landing half and writes both out, because a
    /// flood through `step_allowed` would be the bake compared against itself —
    /// see that example. What it should not also write out is the traversal,
    /// which has nothing to do with either rule.
    ///
    /// # Panics
    ///
    /// As [`Reach::of`]: the origin stands inside the rectangle.
    #[must_use]
    pub fn by<F>(origin: Point, width: u32, height: u32, mut expand: F) -> Self
    where
        F: FnMut(Point) -> [Option<Point>; 8],
    {
        assert!(
            u32::from(origin.x) < width && u32::from(origin.y) < height,
            "a flood starts on its own map: ({}, {}) is outside {width}x{height}",
            origin.x,
            origin.y,
        );
        let cells = width as usize * height as usize;
        let mut stood = vec![None; cells];
        let mut queue = VecDeque::new();
        stood[origin.y as usize * width as usize + origin.x as usize] = Some(origin.z);
        let mut count = 1;
        queue.push_back(origin);
        while let Some(at) = queue.pop_front() {
            for next in expand(at).into_iter().flatten() {
                // The rectangle is the flood's only edge over open ground; over
                // a map the rule has already refused everything past it.
                if u32::from(next.x) >= width || u32::from(next.y) >= height {
                    continue;
                }
                let slot = next.y as usize * width as usize + next.x as usize;
                if stood[slot].is_some() {
                    continue;
                }
                stood[slot] = Some(next.z);
                count += 1;
                queue.push_back(next);
            }
        }
        Self {
            width,
            height,
            stood,
            count,
        }
    }

    /// The height a body stands at on this tile, or `None` where the flood
    /// never arrived — off the rectangle included.
    #[must_use]
    pub fn stands_at(&self, x: u16, y: u16) -> Option<i8> {
        if u32::from(x) >= self.width || u32::from(y) >= self.height {
            return None;
        }
        self.stood[y as usize * self.width as usize + x as usize]
    }

    /// Whether the flood reached this tile.
    #[must_use]
    pub fn holds(&self, x: u16, y: u16) -> bool {
        self.stands_at(x, y).is_some()
    }

    /// How many tiles it reached.
    #[must_use]
    pub const fn count(&self) -> usize {
        self.count
    }

    /// How many tiles it was flooded across, reached or not — the denominator
    /// of the only percentage anyone prints.
    #[must_use]
    pub const fn tiles(&self) -> usize {
        self.stood.len()
    }
}

#[cfg(test)]
mod tests {
    use openshard_map::overlay::{
        Doors,
        Overlay,
    };

    use super::*;
    use crate::scene::{
        SIDE,
        Scene,
    };

    /// The flood is the step rule's own reading: a wall across a scene is where
    /// it stops, and the half it started in is what it holds.
    #[test]
    fn a_wall_is_where_the_flood_stops() {
        let mut scene = Scene::flat(0);
        for x in 0..SIDE {
            scene.wall(x, 4, 0, 20);
        }
        let reach = Reach::of(
            &scene.footing(),
            Point::new(0, 0, 0),
            u32::from(scene.width()),
            u32::from(scene.height()),
        );
        assert_eq!(
            reach.count(),
            (SIDE * 4) as usize,
            "the north half, and nothing else"
        );
        assert!(reach.holds(7, 3), "up to the wall");
        assert!(!reach.holds(7, 5), "and nothing past it");
    }

    /// The rectangle is the edge over open ground, where the step rule has none
    /// of its own to offer: a footing with no map allows every step, and the
    /// flood still stops at the width and height it was handed.
    #[test]
    fn open_ground_is_flooded_no_further_than_the_rectangle() {
        let nothing = Overlay::default();
        let open = Footing::new(None, &nothing, Doors::AsTheyStand);
        let reach = Reach::of(&open, Point::new(2, 2, 0), 8, 6);
        assert_eq!(reach.count(), reach.tiles(), "open ground is reached entirely");
        assert_eq!(reach.tiles(), 48);
        assert!(!reach.holds(8, 0), "and the tile past the edge is not in it");
    }

    /// A handed-in expansion is walked instead of the shipped rule, which is
    /// what an oracle comparing two rules needs — here a rule that only ever
    /// goes east.
    #[test]
    fn a_handed_in_rule_is_the_one_flooded() {
        let east = |at: Point| {
            let mut landings = [None; 8];
            // `Direction::East` is bits 2, and a lane of one row is all this
            // rule can offer.
            landings[2] = (at.x + 1 < 8).then(|| Point::new(at.x + 1, at.y, at.z));
            landings
        };
        let reach = Reach::by(Point::new(0, 3, 0), 8, 8, east);
        assert_eq!(reach.count(), 8, "one lane, and the origin is one of its tiles");
        assert!(reach.holds(7, 3));
        assert!(!reach.holds(0, 4), "nothing left the row");
    }
}
