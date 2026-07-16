//! Answering "what is near this point".
//!
//! # Why an index at all
//!
//! The naive answer is to walk every mobile and compare distances. At ten
//! players that is faster than anything clever. At five hundred it runs once per
//! mobile per step — a quarter of a million comparisons for one person walking
//! across Britain — and the shard dies under exactly the population that makes
//! it worth running.
//!
//! # Sectors, not a quadtree
//!
//! A flat grid of fixed-size buckets. Sphere uses 64-tile sectors
//! (`SECTORSIZE_DEFAULT 64 /* 8 x 8 */` — eight map blocks square) and so does
//! this.
//!
//! A quadtree or a BVH would adapt to clustering, and neither is worth it: a
//! sector lookup is two divisions and an index, a move is two `Vec` operations,
//! and the world is a fixed rectangle known at load. Britannia does not need a
//! tree to find the tile next door.
//!
//! # Distance in UO is a square
//!
//! Chebyshev — `max(|dx|, |dy|)` — from Sphere's `GetDistSightBase`. Not
//! Euclidean. That is not an approximation anyone chose for speed: the client
//! draws a *square* region, so a mobile at (18, 18) is exactly as visible as one
//! at (18, 0). Using a circle here would leave the corners of every screen
//! empty, and the bug looks like mobiles popping in and out at the edges.

use std::collections::HashMap;

use openshard_entities::EntityId;
use openshard_protocol::Point;

/// Tiles per sector, each way.
///
/// Sphere's `SECTORSIZE_DEFAULT`. Comfortably wider than [`VIEW_RANGE`], which
/// is what keeps a lookup to at most four sectors.
pub const SECTOR_SIZE: u32 = 64;

/// How far a client draws mobiles.
///
/// Sphere's `UO_MAP_VIEW_SIZE_DEFAULT`. Old clients are always 18; since
/// 7.0.55.27 the 2D client scales 18–24 with its window, and the enhanced client
/// goes to 24. Sending 18 to a client showing 24 leaves a ring of empty ground
/// it expects to be populated — a thing to fix when `0xC8` (view range) is read,
/// not by guessing high.
pub const VIEW_RANGE: u32 = 18;

/// The property that makes a lookup cheap: a view centred anywhere spans at most
/// two sectors each way, so [`Sectors::nearby`] scans at most four buckets. If
/// the sector ever shrinks below the view diameter a lookup starts touching nine
/// buckets and then sixteen, and this stops the build rather than the shard.
///
/// A `const` assertion and not a test: both sides are compile-time constants, so
/// a test of them can only ever assert `true` at runtime — the check belongs
/// where the constants are.
const _: () = assert!(
    VIEW_RANGE * 2 < SECTOR_SIZE,
    "the view diameter must fit inside one sector"
);

/// UO's distance: Chebyshev, because the client draws a square.
///
/// From Sphere's `GetDistSightBase`. A diagonal step covers the same distance as
/// a straight one, which is also why diagonal movement costs no extra time.
pub fn distance(a: Point, b: Point) -> u32 {
    let dx = u32::from(a.x.abs_diff(b.x));
    let dy = u32::from(a.y.abs_diff(b.y));
    dx.max(dy)
}

/// Whether `b` is within `range` of `a`.
pub fn in_range(a: Point, b: Point, range: u32) -> bool {
    distance(a, b) <= range
}

/// A flat grid of buckets over one facet.
///
/// # This duplicates `Position`, and that is what an index is
///
/// The grid stores each entity's point alongside its id, so a lookup can filter
/// exactly rather than handing back a whole sector for the caller to sift. That
/// is a second copy of something `Position` already holds, and the tick is what
/// keeps them in step — the same bargain as `Position` and `Movement`.
///
/// The alternative is a grid that returns candidates and makes every caller
/// re-read positions from the registry. That is not less duplication, it is the
/// same duplication with the correctness moved somewhere nobody tests.
#[derive(Debug)]
pub struct Sectors {
    /// Sectors across.
    across: u32,
    /// Sectors down.
    down: u32,
    /// Entities per sector, indexed `sector_x * down + sector_y`.
    ///
    /// Column-major to match the map's block order. Not required — nothing
    /// indexes both — but two different orders in one crate is a trap for
    /// whoever reads them next.
    buckets: Vec<Vec<(EntityId, Point)>>,
    /// Which bucket an entity is in, so a move does not scan.
    located: HashMap<EntityId, usize>,
}

impl Sectors {
    /// A grid covering a facet `width` by `height` tiles.
    pub fn new(width: u32, height: u32) -> Self {
        // Round up: a facet that is not a whole number of sectors still needs a
        // bucket for its last, partial one.
        let across = width.div_ceil(SECTOR_SIZE).max(1);
        let down = height.div_ceil(SECTOR_SIZE).max(1);
        Self {
            across,
            down,
            buckets: vec![Vec::new(); (across * down) as usize],
            located: HashMap::new(),
        }
    }

    /// How many entities are indexed.
    pub fn len(&self) -> usize {
        self.located.len()
    }

    /// Whether nothing is indexed.
    pub fn is_empty(&self) -> bool {
        self.located.is_empty()
    }

    /// How many buckets the grid holds.
    pub const fn bucket_count(&self) -> usize {
        (self.across * self.down) as usize
    }

    /// The bucket a point falls in.
    ///
    /// Clamped rather than optional: a point off the map is a bug upstream, and
    /// dropping it out of the index silently would make a mobile invisible
    /// rather than noisy.
    fn bucket_of(&self, point: Point) -> usize {
        let x = (u32::from(point.x) / SECTOR_SIZE).min(self.across - 1);
        let y = (u32::from(point.y) / SECTOR_SIZE).min(self.down - 1);
        (x * self.down + y) as usize
    }

    /// Put an entity in the index, or move it if it is already there.
    pub fn insert(&mut self, entity: EntityId, point: Point) {
        let bucket = self.bucket_of(point);
        if let Some(&current) = self.located.get(&entity) {
            if current == bucket {
                // Same sector: just update the point. The common case by far —
                // a step moves 64 tiles' worth of sector only once every 64
                // steps.
                if let Some(slot) = self.buckets[bucket]
                    .iter_mut()
                    .find(|(id, _)| *id == entity)
                {
                    slot.1 = point;
                }
                return;
            }
            self.remove_from(current, entity);
        }
        self.buckets[bucket].push((entity, point));
        self.located.insert(entity, bucket);
    }

    /// Take an entity out of the index.
    pub fn remove(&mut self, entity: EntityId) {
        if let Some(bucket) = self.located.remove(&entity) {
            self.remove_from(bucket, entity);
        }
    }

    fn remove_from(&mut self, bucket: usize, entity: EntityId) {
        // `swap_remove`: order within a bucket means nothing, and a `retain`
        // would be O(n) in the bucket for every step anyone takes.
        if let Some(index) = self.buckets[bucket]
            .iter()
            .position(|(id, _)| *id == entity)
        {
            self.buckets[bucket].swap_remove(index);
        }
    }

    /// Where the index thinks an entity is.
    pub fn position_of(&self, entity: EntityId) -> Option<Point> {
        let bucket = *self.located.get(&entity)?;
        self.buckets[bucket]
            .iter()
            .find(|(id, _)| *id == entity)
            .map(|(_, point)| *point)
    }

    /// Everything within `range` of `centre`, Chebyshev.
    ///
    /// Exact: the sectors overlapping the box are scanned and each entity is
    /// checked, so nothing outside `range` comes back.
    pub fn nearby(
        &self,
        centre: Point,
        range: u32,
    ) -> impl Iterator<Item = (EntityId, Point)> + '_ {
        // The box in sector coordinates. `saturating_sub` because a range that
        // reaches past the west or north edge is normal — a player standing at
        // x=5 is not a bug.
        let min_x = (u32::from(centre.x).saturating_sub(range)) / SECTOR_SIZE;
        let max_x = ((u32::from(centre.x) + range) / SECTOR_SIZE).min(self.across - 1);
        let min_y = (u32::from(centre.y).saturating_sub(range)) / SECTOR_SIZE;
        let max_y = ((u32::from(centre.y) + range) / SECTOR_SIZE).min(self.down - 1);

        let down = self.down;
        (min_x..=max_x)
            .flat_map(move |x| (min_y..=max_y).map(move |y| (x * down + y) as usize))
            .filter_map(move |bucket| self.buckets.get(bucket))
            .flatten()
            .filter(move |(_, point)| in_range(centre, *point, range))
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openshard_entities::Registry;

    /// A facet-sized grid.
    fn grid() -> Sectors {
        Sectors::new(7168, 4096)
    }

    fn entities(count: usize) -> (Registry, Vec<EntityId>) {
        let mut registry = Registry::new();
        let ids = (0..count).map(|_| registry.spawn()).collect();
        (registry, ids)
    }

    #[test]
    fn distance_is_chebyshev_not_euclidean() {
        // The client draws a square. A mobile at the corner of the screen is as
        // visible as one straight ahead, and a circle would leave the corners
        // empty — which looks like mobiles popping in and out at the edges.
        let origin = Point::new(100, 100, 0);
        assert_eq!(distance(origin, Point::new(118, 100, 0)), 18, "straight");
        assert_eq!(
            distance(origin, Point::new(118, 118, 0)),
            18,
            "diagonal, same"
        );

        // Euclidean would call the diagonal 25.5 and hide it.
        assert!(in_range(origin, Point::new(118, 118, 0), VIEW_RANGE));
    }

    #[test]
    fn distance_ignores_height() {
        // Two mobiles on different floors of a tower are the same distance
        // apart. Whether they can *see* each other is line of sight, which is a
        // different question and not this one.
        let a = Point::new(100, 100, 0);
        let b = Point::new(100, 100, 120);
        assert_eq!(distance(a, b), 0);
    }

    #[test]
    fn distance_is_symmetric_and_never_underflows() {
        // `abs_diff` rather than a subtraction: these are u16s and a mobile at
        // x=0 next to one at x=1 would wrap to 65535 and vanish.
        let west = Point::new(0, 0, 0);
        let east = Point::new(1, 1, 0);
        assert_eq!(distance(west, east), 1);
        assert_eq!(distance(east, west), 1);

        let far = Point::new(u16::MAX, u16::MAX, 0);
        assert_eq!(distance(west, far), u32::from(u16::MAX));
        assert_eq!(distance(far, west), u32::from(u16::MAX));
    }

    #[test]
    fn a_lookup_finds_what_is_near_and_nothing_else() {
        let (_, ids) = entities(3);
        let mut sectors = grid();
        let centre = Point::new(1000, 1000, 0);

        sectors.insert(ids[0], centre);
        sectors.insert(ids[1], Point::new(1010, 1000, 0)); // 10 away
        sectors.insert(ids[2], Point::new(1100, 1000, 0)); // 100 away

        let found: Vec<_> = sectors
            .nearby(centre, VIEW_RANGE)
            .map(|(id, _)| id)
            .collect();
        assert_eq!(found.len(), 2);
        assert!(found.contains(&ids[0]));
        assert!(found.contains(&ids[1]));
        assert!(!found.contains(&ids[2]), "100 tiles away is not on screen");
    }

    #[test]
    fn a_lookup_is_exact_at_its_boundary() {
        // Off by one here means a mobile that appears one step later than the
        // client expects, which is the sort of thing nobody reports and
        // everybody notices.
        let (_, ids) = entities(2);
        let mut sectors = grid();
        let centre = Point::new(1000, 1000, 0);

        sectors.insert(ids[0], Point::new(1000 + VIEW_RANGE as u16, 1000, 0));
        sectors.insert(ids[1], Point::new(1000 + VIEW_RANGE as u16 + 1, 1000, 0));

        let found: Vec<_> = sectors
            .nearby(centre, VIEW_RANGE)
            .map(|(id, _)| id)
            .collect();
        assert_eq!(found, vec![ids[0]], "the range is inclusive");
    }

    #[test]
    fn a_lookup_crosses_sector_boundaries() {
        // The whole reason a naive grid is wrong: two mobiles a step apart can
        // be in different sectors, and a lookup that only scanned its own bucket
        // would lose one of them.
        let (_, ids) = entities(2);
        let mut sectors = grid();

        // Straddle a sector edge: 64 is the first tile of the next sector.
        let west = Point::new(63, 1000, 0);
        let east = Point::new(64, 1000, 0);
        sectors.insert(ids[0], west);
        sectors.insert(ids[1], east);

        let found: Vec<_> = sectors.nearby(west, VIEW_RANGE).map(|(id, _)| id).collect();
        assert_eq!(found.len(), 2, "one step apart, different sectors");
    }

    #[test]
    fn a_lookup_at_every_offset_across_a_sector_edge_is_right() {
        // Sweep the whole neighbourhood rather than spot-check it: the bug this
        // catches is an off-by-one in the box arithmetic, and it only shows up
        // at particular offsets.
        let (_, ids) = entities(1);
        let mut sectors = grid();

        for target in 0..200u16 {
            sectors.insert(ids[0], Point::new(target, 1000, 0));
            for centre in 0..200u16 {
                let from = Point::new(centre, 1000, 0);
                let found = sectors.nearby(from, VIEW_RANGE).count();
                let expected = usize::from(centre.abs_diff(target) <= VIEW_RANGE as u16);
                assert_eq!(
                    found,
                    expected,
                    "a mobile at {target} seen from {centre}: distance {}",
                    centre.abs_diff(target)
                );
            }
        }
    }

    #[test]
    fn moving_within_a_sector_updates_the_point() {
        // The common case: 63 steps out of 64 do not change sector.
        let (_, ids) = entities(1);
        let mut sectors = grid();

        sectors.insert(ids[0], Point::new(1000, 1000, 0));
        sectors.insert(ids[0], Point::new(1001, 1000, 0));

        assert_eq!(sectors.len(), 1, "moved, not duplicated");
        assert_eq!(sectors.position_of(ids[0]), Some(Point::new(1001, 1000, 0)));
    }

    #[test]
    fn moving_between_sectors_does_not_duplicate() {
        // The bug an index invites: insert into the new bucket and forget the
        // old one, and the mobile is visible from two places at once forever.
        let (_, ids) = entities(1);
        let mut sectors = grid();

        sectors.insert(ids[0], Point::new(63, 1000, 0));
        sectors.insert(ids[0], Point::new(64, 1000, 0));

        assert_eq!(sectors.len(), 1);
        let total: usize = (0..sectors.bucket_count())
            .map(|bucket| sectors.buckets[bucket].len())
            .sum();
        assert_eq!(total, 1, "the old bucket still holds a ghost");
    }

    #[test]
    fn a_long_walk_never_leaves_a_ghost() {
        // Every sector boundary in a row, in both axes.
        let (_, ids) = entities(1);
        let mut sectors = grid();

        for step in 0..500u16 {
            sectors.insert(ids[0], Point::new(step, step, 0));
            assert_eq!(sectors.len(), 1, "after step {step}");
            let total: usize = sectors.buckets.iter().map(Vec::len).sum();
            assert_eq!(total, 1, "a ghost appeared at step {step}");
        }
        assert_eq!(sectors.position_of(ids[0]), Some(Point::new(499, 499, 0)));
    }

    #[test]
    fn removing_takes_it_out_of_everything() {
        let (_, ids) = entities(1);
        let mut sectors = grid();
        let point = Point::new(1000, 1000, 0);

        sectors.insert(ids[0], point);
        sectors.remove(ids[0]);

        assert!(sectors.is_empty());
        assert_eq!(sectors.position_of(ids[0]), None);
        assert_eq!(sectors.nearby(point, VIEW_RANGE).count(), 0);
    }

    #[test]
    fn removing_something_that_was_never_there_is_harmless() {
        let (_, ids) = entities(1);
        let mut sectors = grid();
        sectors.remove(ids[0]);
        assert!(sectors.is_empty());
    }

    #[test]
    fn a_lookup_at_the_world_edge_does_not_underflow() {
        // A player at x=5 has a view range that reaches past the west edge.
        // `saturating_sub` is what stops that becoming a scan of the far east.
        let (_, ids) = entities(1);
        let mut sectors = grid();
        let corner = Point::new(0, 0, 0);
        sectors.insert(ids[0], corner);

        assert_eq!(sectors.nearby(corner, VIEW_RANGE).count(), 1);
        assert_eq!(sectors.nearby(Point::new(5, 5, 0), VIEW_RANGE).count(), 1);
    }

    #[test]
    fn a_lookup_past_the_far_edge_is_clamped() {
        let (_, ids) = entities(1);
        let mut sectors = grid();
        let far = Point::new(7167, 4095, 0);
        sectors.insert(ids[0], far);
        assert_eq!(sectors.nearby(far, VIEW_RANGE).count(), 1);
    }

    #[test]
    fn a_point_off_the_map_is_clamped_rather_than_lost() {
        // A bug upstream, but an entity that silently vanishes from the index is
        // a mobile nobody can see and nothing reports.
        let (_, ids) = entities(1);
        let mut sectors = grid();
        sectors.insert(ids[0], Point::new(u16::MAX, u16::MAX, 0));
        assert_eq!(sectors.len(), 1);
    }

    #[test]
    fn a_lookup_scans_a_bounded_number_of_sectors() {
        // Five hundred mobiles spread across the facet: a lookup must touch a
        // handful, not all of them. This is the whole reason the index exists.
        let (_, ids) = entities(500);
        let mut sectors = grid();
        for (index, id) in ids.iter().enumerate() {
            let x = (index as u16 % 100) * 70;
            let y = (index as u16 / 100) * 70;
            sectors.insert(*id, Point::new(x, y, 0));
        }
        assert_eq!(sectors.len(), 500);

        // Spread 70 tiles apart with an 18-tile view, a lookup sees at most
        // itself.
        let found = sectors.nearby(Point::new(0, 0, 0), VIEW_RANGE).count();
        assert!(found <= 4, "{found} mobiles within view of a lone corner");
    }

    #[test]
    fn a_grid_smaller_than_one_sector_still_works() {
        // Ter Mur is 1280 wide; a test map might be 10. Neither should divide by
        // zero or index out of a one-bucket grid.
        let (_, ids) = entities(1);
        let mut sectors = Sectors::new(10, 10);
        assert_eq!(sectors.bucket_count(), 1);

        sectors.insert(ids[0], Point::new(5, 5, 0));
        assert_eq!(sectors.nearby(Point::new(0, 0, 0), VIEW_RANGE).count(), 1);
    }

    #[test]
    fn a_facet_that_is_not_a_whole_number_of_sectors_covers_its_last_tile() {
        // 1448 / 64 is 22.6. Rounding down would leave the last 40 tiles of
        // Tokuno with no bucket, and every mobile there clamped into the one
        // before it.
        let sectors = Sectors::new(1448, 1448);
        assert_eq!(sectors.across, 1448u32.div_ceil(SECTOR_SIZE));
        assert_eq!(
            sectors.bucket_of(Point::new(1447, 1447, 0)),
            sectors.bucket_count() - 1
        );
    }
}
