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
//!
//! # A bucket is two lists, because a bucket is mostly furniture
//!
//! Insert, move and remove have been O(1) since the row index went in. The
//! *read* was not: a lookup walked every entry of up to four buckets, which was
//! cheap while a bucket held mobiles and stopped being cheap the day a house
//! could be decorated. Housing's own caps put about four thousand locked-down
//! items inside a castle, and at 64 tiles a side that castle sits in one or two
//! buckets — so an NPC that happened to share a sector with somebody's keep paid
//! four thousand comparisons per glance, and, since the step's crowd began
//! reading this index too, per step as well.
//!
//! Almost every reader wants **mobiles**: sight, chat, guards, pets, area
//! spells, a sector waking up, and the bodies a step has to get past. So a
//! bucket keeps its mobiles and its items apart and the caller says which it
//! means — [`mobiles_near`](Sectors::mobiles_near),
//! [`items_near`](Sectors::items_near),
//! [`everything_near`](Sectors::everything_near). The castle stops being in the
//! way of the questions that were never about it.

use std::collections::HashMap;

use openshard_entities::EntityId;
use openshard_protocol::world::Point;

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
/// The measurement itself is [`Point::distance`], where both ends of the wire
/// can reach it — the client's sight overlay draws the line a reach check is
/// decided along, and a second copy of the arithmetic is a second answer. This
/// name stays because the whole server counts tiles through it.
pub fn distance(a: Point, b: Point) -> u32 {
    a.distance(b)
}

/// Whether `b` is within `range` of `a`.
pub fn in_range(a: Point, b: Point, range: u32) -> bool {
    distance(a, b) <= range
}

/// Which of a bucket's two lists an entity is filed in.
///
/// **The inserter says, and the grid never works it out by looking.** The fact
/// exists in the registry — a mobile carries a
/// [`Body`](crate::components::Body) and a thing on the ground a
/// [`Drawn`](crate::components::Drawn), one or the other and never both — but
/// reading it here would mean handing this index the registry at every insert,
/// and the answer would then depend on whether the component went on before the
/// index did. Every caller already knows what it is placing: a spawn knows, a
/// step knows, a corpse knows. Saying so costs a word and cannot go stale.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Occupant {
    /// A mobile: a player, an NPC, a creature — anything with a body.
    Mobile,
    /// Anything else standing on the ground: an item, a corpse, a door, a house,
    /// a ship, a moongate.
    Item,
}

/// One sector's contents, in the two lists the module doc explains.
#[derive(Clone, Debug)]
struct Bucket {
    /// The bodies in this sector.
    mobiles: Vec<(EntityId, Point)>,
    /// Everything else in it, which in a decorated town is nearly all of it.
    items: Vec<(EntityId, Point)>,
}

impl Bucket {
    /// A sector with nothing in it.
    const fn empty() -> Self {
        Self {
            mobiles: Vec::new(),
            items: Vec::new(),
        }
    }

    /// The list one kind of occupant is filed in.
    fn list(&self, occupant: Occupant) -> &Vec<(EntityId, Point)> {
        match occupant {
            Occupant::Mobile => &self.mobiles,
            Occupant::Item => &self.items,
        }
    }

    /// The same, to write.
    fn list_mut(&mut self, occupant: Occupant) -> &mut Vec<(EntityId, Point)> {
        match occupant {
            Occupant::Mobile => &mut self.mobiles,
            Occupant::Item => &mut self.items,
        }
    }
}

/// Where the grid has filed one entity.
///
/// All three parts are needed to find its row again without scanning: which
/// bucket, which of that bucket's two lists, and where in that list.
#[derive(Clone, Copy, Debug)]
struct Row {
    /// Index into [`Sectors::buckets`].
    bucket: usize,
    /// Which list of that bucket.
    occupant: Occupant,
    /// Where in that list.
    slot: usize,
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
    buckets: Vec<Bucket>,
    /// Which bucket an entity is in, which list of it, *and* where in that list,
    /// so neither a move nor a removal scans.
    ///
    /// The slot half is not an optimisation of an optimisation. A bucket is 64
    /// tiles square and holds every mobile, ground item and piece of decoration
    /// in it; in a decorated town that is thousands of entries, and finding an
    /// entity's own row in it by scanning was paid on *every step by anyone*.
    /// Keeping the row index costs one `usize` and a repair when `swap_remove`
    /// moves another entity's row — see [`remove_from`](Sectors::remove_from).
    ///
    /// **One row per entity, wherever it is filed.** This map is what makes that
    /// true: an entity handed to [`insert`](Sectors::insert) under a different
    /// [`Occupant`] than it was filed under is *moved* between the two lists,
    /// never copied into the second — which is the same guarantee, and the same
    /// mechanism, as a mobile changing sector.
    located: HashMap<EntityId, Row>,
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
            buckets: vec![Bucket::empty(); (across * down) as usize],
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

    /// Put an entity in the index as `occupant`, or move it if it is already
    /// there.
    ///
    /// `occupant` is the caller's to say — see [`Occupant`]. Handing the same
    /// entity a different one moves it between the bucket's two lists; it is
    /// never in both.
    pub fn insert(&mut self, entity: EntityId, point: Point, occupant: Occupant) {
        let bucket = self.bucket_of(point);
        if let Some(&row) = self.located.get(&entity) {
            if row.bucket == bucket && row.occupant == occupant {
                // Same sector, same list: just update the point. The common case
                // by far — a step moves 64 tiles' worth of sector only once
                // every 64 steps, and nothing changes what kind of thing it is.
                self.buckets[bucket].list_mut(occupant)[row.slot].1 = point;
                return;
            }
            self.remove_from(row);
        }
        let list = self.buckets[bucket].list_mut(occupant);
        let slot = list.len();
        list.push((entity, point));
        self.located.insert(
            entity,
            Row {
                bucket,
                occupant,
                slot,
            },
        );
    }

    /// Take an entity out of the index.
    pub fn remove(&mut self, entity: EntityId) {
        if let Some(row) = self.located.remove(&entity) {
            self.remove_from(row);
        }
    }

    /// Drop one row, repairing whoever `swap_remove` moves into its place.
    fn remove_from(&mut self, row: Row) {
        // `swap_remove`: order within a list means nothing, and a `retain`
        // would be O(n) in the bucket for every step anyone takes.
        let list = self.buckets[row.bucket].list_mut(row.occupant);
        list.swap_remove(row.slot);
        // The last row moved into `slot` — unless the removed row *was* the last.
        if let Some(&(moved, _)) = list.get(row.slot) {
            self.located.insert(
                moved,
                Row {
                    bucket: row.bucket,
                    occupant: row.occupant,
                    slot: row.slot,
                },
            );
        }
    }

    /// Where the index thinks an entity is.
    pub fn position_of(&self, entity: EntityId) -> Option<Point> {
        let &row = self.located.get(&entity)?;
        self.buckets[row.bucket]
            .list(row.occupant)
            .get(row.slot)
            .map(|(_, point)| *point)
    }

    /// The mobiles within `range` of `centre`, Chebyshev.
    ///
    /// Exact: the sectors overlapping the box are scanned and each entity is
    /// checked, so nothing outside `range` comes back — a caller that filters by
    /// distance again is asking a question this already answered.
    ///
    /// What almost every caller wants, and the reason a bucket is two lists. A
    /// castle's four thousand lockdowns are not walked to find the two people
    /// standing in its doorway.
    pub fn mobiles_near(&self, centre: Point, range: u32) -> impl Iterator<Item = (EntityId, Point)> + '_ {
        self.buckets_over(centre, range)
            .flat_map(|bucket| bucket.mobiles.iter())
            .filter(move |(_, point)| in_range(centre, *point, range))
            .copied()
    }

    /// The items within `range` of `centre`, Chebyshev. Exact, as
    /// [`mobiles_near`](Self::mobiles_near) is.
    ///
    /// The other half, and asked far less often: what is on the ground here — a
    /// forge to craft at, a fire to cook on.
    pub fn items_near(&self, centre: Point, range: u32) -> impl Iterator<Item = (EntityId, Point)> + '_ {
        self.buckets_over(centre, range)
            .flat_map(|bucket| bucket.items.iter())
            .filter(move |(_, point)| in_range(centre, *point, range))
            .copied()
    }

    /// Both lists within `range` of `centre`, Chebyshev. Exact, as
    /// [`mobiles_near`](Self::mobiles_near) is.
    ///
    /// For the one question that really is about everything near: what a client
    /// should have on its screen. A player is shown the people *and* the
    /// furniture, so drawing the neighbourhood cannot be either lookup alone.
    pub fn everything_near(&self, centre: Point, range: u32) -> impl Iterator<Item = (EntityId, Point)> + '_ {
        self.buckets_over(centre, range)
            .flat_map(|bucket| bucket.mobiles.iter().chain(bucket.items.iter()))
            .filter(move |(_, point)| in_range(centre, *point, range))
            .copied()
    }

    /// The buckets a Chebyshev box of `range` around `centre` overlaps.
    ///
    /// The half every lookup shares: which sectors to look in, before anything
    /// decides which of their two lists it wants.
    fn buckets_over(&self, centre: Point, range: u32) -> impl Iterator<Item = &Bucket> + '_ {
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
    }

    /// Which sector a point falls in. The unit a crossing is diffed against.
    #[must_use]
    pub fn sector_of(&self, point: Point) -> usize {
        self.bucket_of(point)
    }

    /// The mobiles in the sector `centre` falls in, and in its eight neighbours.
    ///
    /// Sphere's sector-wake unit: `CSector::_CanSleep` takes the block, not the
    /// tile (`fCheckAdjacents`), so a sector is already alive before a player
    /// crosses into it. Deliberately not a radius — the point is to cover the
    /// player's whole sector wherever in it they happen to stand, which a radius
    /// centred on them does not, and to stay cheap enough that the caller need
    /// only run it on the tick someone actually crosses a boundary.
    ///
    /// Mobiles only, because waking is something only a mobile does: an item has
    /// no beat to pull forward, and nine whole sectors of furniture is the most
    /// expensive sweep in this file to walk for nothing.
    pub fn mobiles_in_block(&self, centre: Point) -> impl Iterator<Item = (EntityId, Point)> + '_ {
        let x = (u32::from(centre.x) / SECTOR_SIZE).min(self.across - 1);
        let y = (u32::from(centre.y) / SECTOR_SIZE).min(self.down - 1);
        let (min_x, max_x) = (x.saturating_sub(1), (x + 1).min(self.across - 1));
        let (min_y, max_y) = (y.saturating_sub(1), (y + 1).min(self.down - 1));
        let down = self.down;
        (min_x..=max_x)
            .flat_map(move |block_x| (min_y..=max_y).map(move |block_y| (block_x * down + block_y) as usize))
            .filter_map(move |bucket| self.buckets.get(bucket))
            .flat_map(|bucket| bucket.mobiles.iter())
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

    /// Every row the grid holds, both lists of every bucket. What a duplicate
    /// shows up in: `len` counts the `located` map, which cannot see a row the
    /// index has lost track of.
    fn rows(sectors: &Sectors) -> usize {
        sectors
            .buckets
            .iter()
            .map(|bucket| bucket.mobiles.len() + bucket.items.len())
            .sum()
    }

    #[test]
    fn distance_is_chebyshev_not_euclidean() {
        // The client draws a square. A mobile at the corner of the screen is as
        // visible as one straight ahead, and a circle would leave the corners
        // empty — which looks like mobiles popping in and out at the edges.
        let origin = Point::new(100, 100, 0);
        assert_eq!(distance(origin, Point::new(118, 100, 0)), 18, "straight");
        assert_eq!(distance(origin, Point::new(118, 118, 0)), 18, "diagonal, same");

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

        sectors.insert(ids[0], centre, Occupant::Mobile);
        sectors.insert(ids[1], Point::new(1010, 1000, 0), Occupant::Mobile); // 10 away
        sectors.insert(ids[2], Point::new(1100, 1000, 0), Occupant::Mobile); // 100 away

        let found: Vec<_> = sectors
            .mobiles_near(centre, VIEW_RANGE)
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

        sectors.insert(
            ids[0],
            Point::new(1000 + VIEW_RANGE as u16, 1000, 0),
            Occupant::Mobile,
        );
        sectors.insert(
            ids[1],
            Point::new(1000 + VIEW_RANGE as u16 + 1, 1000, 0),
            Occupant::Mobile,
        );

        let found: Vec<_> = sectors
            .mobiles_near(centre, VIEW_RANGE)
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
        sectors.insert(ids[0], west, Occupant::Mobile);
        sectors.insert(ids[1], east, Occupant::Mobile);

        let found: Vec<_> = sectors.mobiles_near(west, VIEW_RANGE).map(|(id, _)| id).collect();
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
            sectors.insert(ids[0], Point::new(target, 1000, 0), Occupant::Mobile);
            for centre in 0..200u16 {
                let from = Point::new(centre, 1000, 0);
                let found = sectors.mobiles_near(from, VIEW_RANGE).count();
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

        sectors.insert(ids[0], Point::new(1000, 1000, 0), Occupant::Mobile);
        sectors.insert(ids[0], Point::new(1001, 1000, 0), Occupant::Mobile);

        assert_eq!(sectors.len(), 1, "moved, not duplicated");
        assert_eq!(sectors.position_of(ids[0]), Some(Point::new(1001, 1000, 0)));
    }

    #[test]
    fn moving_between_sectors_does_not_duplicate() {
        // The bug an index invites: insert into the new bucket and forget the
        // old one, and the mobile is visible from two places at once forever.
        let (_, ids) = entities(1);
        let mut sectors = grid();

        sectors.insert(ids[0], Point::new(63, 1000, 0), Occupant::Mobile);
        sectors.insert(ids[0], Point::new(64, 1000, 0), Occupant::Mobile);

        assert_eq!(sectors.len(), 1);
        assert_eq!(rows(&sectors), 1, "the old bucket still holds a ghost");
    }

    #[test]
    fn changing_which_list_an_entity_is_in_moves_it_rather_than_copying_it() {
        // The same bug one field over, and the one the split invented: file an
        // entity as an item and then as a mobile without taking it out first,
        // and a grid that only compared buckets would leave it in both lists of
        // the same bucket — found twice by `everything_near`, and never removed
        // by anything that removes it once.
        let (_, ids) = entities(1);
        let mut sectors = grid();
        let at = Point::new(1000, 1000, 0);

        sectors.insert(ids[0], at, Occupant::Item);
        sectors.insert(ids[0], at, Occupant::Mobile);

        assert_eq!(sectors.len(), 1);
        assert_eq!(rows(&sectors), 1, "one row, whichever list it is in");
        assert_eq!(sectors.mobiles_near(at, VIEW_RANGE).count(), 1);
        assert_eq!(sectors.items_near(at, VIEW_RANGE).count(), 0);
        assert_eq!(sectors.everything_near(at, VIEW_RANGE).count(), 1);
    }

    #[test]
    fn a_long_walk_never_leaves_a_ghost() {
        // Every sector boundary in a row, in both axes.
        let (_, ids) = entities(1);
        let mut sectors = grid();

        for step in 0..500u16 {
            sectors.insert(ids[0], Point::new(step, step, 0), Occupant::Mobile);
            assert_eq!(sectors.len(), 1, "after step {step}");
            assert_eq!(rows(&sectors), 1, "a ghost appeared at step {step}");
        }
        assert_eq!(sectors.position_of(ids[0]), Some(Point::new(499, 499, 0)));
    }

    #[test]
    fn removing_takes_it_out_of_everything() {
        let (_, ids) = entities(1);
        let mut sectors = grid();
        let point = Point::new(1000, 1000, 0);

        sectors.insert(ids[0], point, Occupant::Mobile);
        sectors.remove(ids[0]);

        assert!(sectors.is_empty());
        assert_eq!(sectors.position_of(ids[0]), None);
        assert_eq!(sectors.everything_near(point, VIEW_RANGE).count(), 0);
    }

    #[test]
    fn removing_an_item_repairs_the_item_it_moved() {
        // `swap_remove` moves the last row into the hole, and the row index of
        // whoever was moved has to be repaired *in its own list*. Repairing it
        // against the bucket's mobiles instead would leave an item pointing at a
        // row that is somebody else's, or at no row at all.
        let (_, ids) = entities(3);
        let mut sectors = grid();
        let at = Point::new(1000, 1000, 0);
        for id in &ids {
            sectors.insert(*id, at, Occupant::Item);
        }

        sectors.remove(ids[0]);

        assert_eq!(
            sectors.position_of(ids[2]),
            Some(at),
            "the moved row still resolves"
        );
        sectors.remove(ids[2]);
        assert_eq!(sectors.len(), 1);
        assert_eq!(rows(&sectors), 1);
        assert_eq!(sectors.items_near(at, VIEW_RANGE).count(), 1);
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
        sectors.insert(ids[0], corner, Occupant::Mobile);

        assert_eq!(sectors.mobiles_near(corner, VIEW_RANGE).count(), 1);
        assert_eq!(sectors.mobiles_near(Point::new(5, 5, 0), VIEW_RANGE).count(), 1);
    }

    #[test]
    fn a_lookup_past_the_far_edge_is_clamped() {
        let (_, ids) = entities(1);
        let mut sectors = grid();
        let far = Point::new(7167, 4095, 0);
        sectors.insert(ids[0], far, Occupant::Mobile);
        assert_eq!(sectors.mobiles_near(far, VIEW_RANGE).count(), 1);
    }

    #[test]
    fn a_point_off_the_map_is_clamped_rather_than_lost() {
        // A bug upstream, but an entity that silently vanishes from the index is
        // a mobile nobody can see and nothing reports.
        let (_, ids) = entities(1);
        let mut sectors = grid();
        sectors.insert(ids[0], Point::new(u16::MAX, u16::MAX, 0), Occupant::Mobile);
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
            sectors.insert(*id, Point::new(x, y, 0), Occupant::Mobile);
        }
        assert_eq!(sectors.len(), 500);

        // Spread 70 tiles apart with an 18-tile view, a lookup sees at most
        // itself.
        let found = sectors.mobiles_near(Point::new(0, 0, 0), VIEW_RANGE).count();
        assert!(found <= 4, "{found} mobiles within view of a lone corner");
    }

    #[test]
    fn a_decorated_house_is_not_in_the_way_of_a_mobile_lookup() {
        // The whole reason for the split, as an assertion about *what is
        // walked* rather than about how long it takes: housing's caps put about
        // four thousand lockdowns in a castle, and at 64 tiles a side they share
        // a bucket with everyone standing in the street outside.
        let (_, ids) = entities(4_002);
        let mut sectors = grid();
        let doorway = Point::new(1000, 1000, 0);
        for id in &ids[..4_000] {
            sectors.insert(*id, doorway, Occupant::Item);
        }
        sectors.insert(ids[4_000], doorway, Occupant::Mobile);
        sectors.insert(ids[4_001], Point::new(1001, 1000, 0), Occupant::Mobile);

        let mobiles: Vec<_> = sectors.mobiles_near(doorway, VIEW_RANGE).collect();
        assert_eq!(mobiles.len(), 2, "the two people, and not the furniture");
        assert_eq!(sectors.mobiles_in_block(doorway).count(), 2);
        assert_eq!(sectors.items_near(doorway, VIEW_RANGE).count(), 4_000);
        assert_eq!(sectors.everything_near(doorway, VIEW_RANGE).count(), 4_002);
    }

    #[test]
    fn a_grid_smaller_than_one_sector_still_works() {
        // Ter Mur is 1280 wide; a test map might be 10. Neither should divide by
        // zero or index out of a one-bucket grid.
        let (_, ids) = entities(1);
        let mut sectors = Sectors::new(10, 10);
        assert_eq!(sectors.bucket_count(), 1);

        sectors.insert(ids[0], Point::new(5, 5, 0), Occupant::Mobile);
        assert_eq!(sectors.mobiles_near(Point::new(0, 0, 0), VIEW_RANGE).count(), 1);
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
