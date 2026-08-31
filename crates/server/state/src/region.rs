//! Named areas of a facet: towns, dungeons, guarded zones.
//!
//! A region is a name, a set of rectangles, and a few facts that hold inside
//! them — whether guards answer a call, how dark it is, what music plays. Both
//! references have the concept (ServUO's `Region`, Sphere's `REGION` blocks) and
//! several rules want it: the guard is the classic answer to a criminal flag,
//! `housing` needs somewhere to place a house, and a dungeon is dark because it
//! says so, not because the sun set.
//!
//! # Flat list, not a tree
//!
//! ServUO nests regions in XML and walks parents at lookup time. Here the
//! nesting is *flattened where the data is written* — a child becomes a region of
//! its own with a higher [`priority`](Region::priority), inheriting whatever it
//! did not override — so the engine holds a plain list and a number, and a lookup
//! is "the highest priority rectangle containing this point". Nothing walks a
//! parent chain, and nothing can build a cycle.
//!
//! # The grid is an accelerator, not the truth
//!
//! Regions are few (~150 on a converted Felucca) but a lookup runs per player per
//! tick, so [`Regions`] keeps a coarse bucket grid of candidate ids, on the same
//! sector size the interest index uses. The fine test is always
//! rectangle-containment: a wrong bucket can only cost time, never an answer.

use openshard_protocol::world::Point;

use crate::sectors::SECTOR_SIZE;

/// A region's facet-local index.
///
/// This is distinct from the many other `u16` values in world state: it only
/// addresses a [`Regions`] collection for one facet.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct RegionId(pub u16);

/// A facet named more regions than its facet-local id can represent.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TooManyRegions {
    /// How many regions were supplied.
    pub found:   usize,
    /// How many distinct ids exist.
    pub maximum: usize,
}

impl std::fmt::Display for TooManyRegions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} regions were supplied, but a facet can identify at most {}",
            self.found, self.maximum
        )
    }
}

impl std::error::Error for TooManyRegions {
}

const MAX_REGIONS: usize = u16::MAX as usize + 1;

/// One box of a region, in tiles, with the height band it applies to.
///
/// A region is a *set* of these — a town is rarely one rectangle, and a dungeon
/// level sits inside a z band so the surface above it stays open sky.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RegionRect {
    /// West edge.
    pub x:      u16,
    /// North edge.
    pub y:      u16,
    /// How far east it reaches.
    pub width:  u16,
    /// How far south it reaches.
    pub height: u16,
    /// The lowest height it covers. `i8::MIN` covers everything below.
    pub z_min:  i8,
    /// The highest height it covers. `i8::MAX` covers everything above.
    pub z_max:  i8,
}

impl RegionRect {
    /// A rectangle covering every height — the common case, and what a `<rect>`
    /// with no `zrange` means.
    #[must_use]
    pub const fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Self {
            x,
            y,
            width,
            height,
            z_min: i8::MIN,
            z_max: i8::MAX,
        }
    }

    /// The same with a height band.
    #[must_use]
    pub const fn with_z(mut self, z_min: i8, z_max: i8) -> Self {
        self.z_min = z_min;
        self.z_max = z_max;
        self
    }

    /// Whether `point` falls inside, height included.
    #[must_use]
    pub fn contains(&self, point: Point) -> bool {
        point.x >= self.x
            && point.y >= self.y
            && u32::from(point.x) < u32::from(self.x) + u32::from(self.width)
            && u32::from(point.y) < u32::from(self.y) + u32::from(self.height)
            && point.z >= self.z_min
            && point.z <= self.z_max
    }
}

/// What holds inside a region — the rules an area changes, as opposed to the
/// scenery it names.
///
/// [`none`](Self::none) leaves every rule off, so an area that declares nothing
/// behaves exactly as the world did before regions existed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RegionFlags {
    /// Guards answer a call here, and hunt a murderer who walks in — ServUO's
    /// `GuardedRegion`.
    pub guarded:     bool,
    /// No teleporting in, out or within — the staff `.tele` and the Teleport
    /// spell both refuse.
    pub no_teleport: bool,
    /// No Recall or Gate — `magic::travel::may_travel` refuses both, and marking
    /// a rune inside one too.
    pub no_recall:   bool,
    /// No house may be placed — `housing::place`'s sixth rule, asked over every
    /// tile the house would cover. Twenty-one of the shipped regions set it.
    pub no_housing:  bool,
    /// A safe zone — no player may harm another. Waiting for its consumer too.
    pub safe:        bool,
}

impl RegionFlags {
    /// A region that changes no rule from the shard-wide baseline.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            guarded:     false,
            no_teleport: false,
            no_recall:   false,
            no_housing:  false,
            safe:        false,
        }
    }
}

/// A named area of one facet.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Region {
    /// Its index in the facet's list, which is also its wire and save id.
    pub id:       RegionId,
    /// What it is called — "Britain", "Covetous".
    pub name:     String,
    /// Which region wins where two overlap: the higher number. A nested area is
    /// written with a higher priority than the one that contains it.
    pub priority: u8,
    /// The boxes it covers.
    pub rects:    Vec<RegionRect>,
    /// The rules that hold inside it.
    pub flags:    RegionFlags,
    /// The track the client plays here, as a `MusicName` index (ServUO's enum
    /// order). `None` leaves whatever was playing alone.
    pub music:    Option<u16>,
    /// The light level inside, overriding the time of day — a dungeon is dark at
    /// noon. `None` takes the ambient.
    pub light:    Option<u8>,
}

impl Region {
    /// Whether `point` is inside any of its boxes.
    #[must_use]
    pub fn contains(&self, point: Point) -> bool {
        self.rects.iter().any(|rect| rect.contains(point))
    }
}

/// One facet's regions, and the admin verb that lays them.
///
/// The verb rides with the data because that is where it stays honest. Regions
/// are the first dataset the tree ships that is *not* registered at boot — an
/// operator lays and clears them from the staff menu — so something has to say
/// which button means this set. A `match` in the server would be a second list
/// to keep level with `world::admin`'s `ROWS`; a field is one.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RegionSet {
    /// What the staff menu's button sends: `regions:felucca`.
    pub verb:    String,
    /// Which facet these belong to.
    pub facet:   openshard_protocol::world::Facet,
    /// The areas, in the order [`Regions::set`] will number them.
    pub regions: Vec<Region>,
}

/// Every region on one facet, with a coarse grid to find them by.
///
/// Lives on the facet's state beside the interest grid and the obstruction index,
/// for the same reason those do: two facets never share one, so nothing has to
/// remember to check which facet a region belongs to.
#[derive(Debug)]
pub struct Regions {
    /// The regions, in registration order. An id is an index into this.
    regions: Vec<Region>,
    /// Candidate region ids per bucket, indexed `bucket_x * down + bucket_y`.
    /// Column-major, matching [`Sectors`](crate::Sectors).
    grid:    Vec<Vec<RegionId>>,
    /// Buckets across.
    across:  u32,
    /// Buckets down.
    down:    u32,
}

impl Default for Regions {
    /// A one-bucket index, for a facet whose size is not known yet. Correct, not
    /// fast: every lookup clamps into the single bucket and falls through to the
    /// rectangle test. Deriving this would leave *zero* buckets, and a lookup
    /// with nowhere to clamp to panics.
    fn default() -> Self {
        Self::new(1, 1)
    }
}

impl Regions {
    /// An empty index sized for a facet `width` by `height` tiles.
    ///
    /// [`Default`] gives a one-bucket grid instead, which is still *correct* —
    /// every lookup clamps into that bucket and falls through to the rectangle
    /// test — just unaccelerated. That is what makes a facet built before its map
    /// is known safe to fill in later.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        let across = width.div_ceil(SECTOR_SIZE).max(1);
        let down = height.div_ceil(SECTOR_SIZE).max(1);
        Self {
            regions: Vec::new(),
            grid: vec![Vec::new(); (across * down) as usize],
            across,
            down,
        }
    }

    /// Replace every region with `regions`, renumbering their ids to their
    /// position and rebuilding the grid.
    ///
    /// Replace-all rather than add-one, like the decoration and spawner sweeps:
    /// a registration carries the whole set, so registering twice cannot leave a stale
    /// half behind.
    #[track_caller]
    pub fn set(&mut self, regions: Vec<Region>) {
        self.try_set(regions)
            .expect("an in-tree region set fits the facet-local RegionId");
    }

    /// Replace every region when each can receive a distinct [`RegionId`].
    ///
    /// Refuses the new set before changing the old one. Reusing `u16::MAX` for
    /// every excess region would make unrelated guarded and travel rules share
    /// an identity and make the bucket grid point at the wrong area.
    ///
    /// # Errors
    ///
    /// [`TooManyRegions`] when `regions` has more entries than `RegionId` can
    /// distinguish.
    pub fn try_set(&mut self, mut regions: Vec<Region>) -> Result<(), TooManyRegions> {
        if regions.len() > MAX_REGIONS {
            return Err(TooManyRegions {
                found:   regions.len(),
                maximum: MAX_REGIONS,
            });
        }
        for (index, region) in regions.iter_mut().enumerate() {
            region.id = RegionId(u16::try_from(index).expect("the region count was checked above"));
        }
        self.regions = regions;
        self.reindex();
        Ok(())
    }

    /// Forget every region.
    pub fn clear(&mut self) {
        self.regions.clear();
        for bucket in &mut self.grid {
            bucket.clear();
        }
    }

    /// The region at `point`: the one with the highest priority whose boxes
    /// contain it, or the last registered of those tied.
    #[must_use]
    pub fn at(&self, point: Point) -> Option<&Region> {
        let bucket = &self.grid[self.bucket_of(point.x, point.y)];
        let mut best: Option<&Region> = None;
        for &id in bucket {
            let region = &self.regions[usize::from(id.0)];
            if !region.contains(point) {
                continue;
            }
            // `>=` so a later registration wins a tie: the flattened child is
            // written after the area that contains it.
            if best.is_none_or(|current| region.priority >= current.priority) {
                best = Some(region);
            }
        }
        best
    }

    /// A region by id, for turning a remembered id back into a name.
    #[must_use]
    pub fn get(&self, id: RegionId) -> Option<&Region> {
        self.regions.get(usize::from(id.0))
    }

    /// Every region, in id order — the save sweep's view.
    pub fn iter(&self) -> impl Iterator<Item = &Region> {
        self.regions.iter()
    }

    /// How many regions there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.regions.len()
    }

    /// Whether the facet has no regions at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    /// Fill the grid from the current regions. Every bucket a rectangle touches
    /// gets its id, so a lookup only ever tests rectangles that could match.
    fn reindex(&mut self) {
        for bucket in &mut self.grid {
            bucket.clear();
        }
        for index in 0..self.regions.len() {
            let id = RegionId(u16::try_from(index).expect("Regions::try_set bounded every stored index"));
            // Collected first: `bucket_of` borrows self immutably.
            let mut buckets = Vec::new();
            for rect in &self.regions[index].rects {
                let last_x = u32::from(rect.x) + u32::from(rect.width).saturating_sub(1);
                let last_y = u32::from(rect.y) + u32::from(rect.height).saturating_sub(1);
                // Both ends clamp, not just the far one: on an unsized index
                // every tile belongs to bucket zero, and a range starting past
                // the end would be empty rather than clamped.
                let first_x = (u32::from(rect.x) / SECTOR_SIZE).min(self.across - 1);
                let first_y = (u32::from(rect.y) / SECTOR_SIZE).min(self.down - 1);
                let last_x = (last_x / SECTOR_SIZE).min(self.across - 1);
                let last_y = (last_y / SECTOR_SIZE).min(self.down - 1);
                for bx in first_x..=last_x {
                    for by in first_y..=last_y {
                        buckets.push((bx * self.down + by) as usize);
                    }
                }
            }
            for bucket in buckets {
                if !self.grid[bucket].contains(&id) {
                    self.grid[bucket].push(id);
                }
            }
        }
    }

    /// The bucket a tile falls in, clamped — a point off the map is a bug
    /// upstream, and silently having no bucket would answer "no region" for a
    /// place that has one.
    fn bucket_of(&self, x: u16, y: u16) -> usize {
        let bx = (u32::from(x) / SECTOR_SIZE).min(self.across - 1);
        let by = (u32::from(y) / SECTOR_SIZE).min(self.down - 1);
        (bx * self.down + by) as usize
    }
}

include!(concat!(env!("OUT_DIR"), "/regions.rs"));

#[cfg(test)]
mod tests {
    use super::*;

    fn region(id: u16, name: &str, priority: u8, rects: Vec<RegionRect>) -> Region {
        Region {
            id: RegionId(id),
            name: name.to_owned(),
            priority,
            rects,
            flags: RegionFlags::none(),
            music: None,
            light: None,
        }
    }

    fn at(regions: &Regions, x: u16, y: u16) -> Option<&str> {
        regions.at(Point::new(x, y, 0)).map(|r| r.name.as_str())
    }

    #[test]
    fn a_point_inside_a_rectangle_finds_its_region() {
        let mut regions = Regions::new(1024, 1024);
        regions.set(vec![region(
            0,
            "Britain",
            50,
            vec![RegionRect::new(100, 100, 50, 50)],
        )]);

        assert_eq!(at(&regions, 100, 100), Some("Britain"));
        assert_eq!(at(&regions, 149, 149), Some("Britain"));
    }

    #[test]
    fn a_point_outside_every_rectangle_is_in_no_region() {
        let mut regions = Regions::new(1024, 1024);
        regions.set(vec![region(
            0,
            "Britain",
            50,
            vec![RegionRect::new(100, 100, 50, 50)],
        )]);

        // One past each edge: width and height are extents, not inclusive bounds.
        assert_eq!(at(&regions, 150, 120), None);
        assert_eq!(at(&regions, 120, 150), None);
        assert_eq!(at(&regions, 99, 99), None);
    }

    #[test]
    fn a_region_is_the_union_of_its_rectangles() {
        let mut regions = Regions::new(1024, 1024);
        regions.set(vec![region(
            0,
            "Britain",
            50,
            vec![
                RegionRect::new(100, 100, 10, 10),
                RegionRect::new(400, 400, 10, 10),
            ],
        )]);

        assert_eq!(at(&regions, 105, 105), Some("Britain"));
        assert_eq!(at(&regions, 405, 405), Some("Britain"));
        assert_eq!(at(&regions, 250, 250), None);
    }

    #[test]
    fn the_higher_priority_wins_where_two_overlap() {
        let mut regions = Regions::new(1024, 1024);
        regions.set(vec![
            region(0, "Britain", 50, vec![RegionRect::new(100, 100, 100, 100)]),
            region(0, "The Bank", 51, vec![RegionRect::new(120, 120, 10, 10)]),
        ]);

        assert_eq!(at(&regions, 125, 125), Some("The Bank"));
        assert_eq!(at(&regions, 160, 160), Some("Britain"));
    }

    #[test]
    fn a_nested_region_registered_later_wins_an_equal_priority() {
        // The converter flattens nesting by raising the child's priority, but a
        // hand-written pack may leave both equal; the later registration is the
        // inner one, so it takes the tie.
        let mut regions = Regions::new(1024, 1024);
        regions.set(vec![
            region(0, "Outside", 50, vec![RegionRect::new(0, 0, 100, 100)]),
            region(0, "Inside", 50, vec![RegionRect::new(10, 10, 10, 10)]),
        ]);

        assert_eq!(at(&regions, 15, 15), Some("Inside"));
    }

    #[test]
    fn a_height_band_keeps_the_surface_out_of_the_dungeon() {
        let mut regions = Regions::new(1024, 1024);
        regions.set(vec![region(
            0,
            "Covetous",
            50,
            vec![RegionRect::new(100, 100, 50, 50).with_z(-128, -20)],
        )]);

        assert_eq!(
            regions.at(Point::new(120, 120, -40)).map(|r| r.id),
            Some(RegionId(0))
        );
        assert!(regions.at(Point::new(120, 120, 0)).is_none());
    }

    #[test]
    fn set_renumbers_ids_to_their_position() {
        let mut regions = Regions::new(1024, 1024);
        regions.set(vec![
            region(77, "First", 1, vec![RegionRect::new(0, 0, 10, 10)]),
            region(77, "Second", 1, vec![RegionRect::new(20, 20, 10, 10)]),
        ]);

        assert_eq!(regions.get(RegionId(0)).map(|r| r.name.as_str()), Some("First"));
        assert_eq!(regions.get(RegionId(1)).map(|r| r.name.as_str()), Some("Second"));
        assert_eq!(at(&regions, 25, 25), Some("Second"));
    }

    #[test]
    fn too_many_regions_are_refused_without_replacing_the_live_set() {
        let mut regions = Regions::new(1024, 1024);
        regions.set(vec![region(
            0,
            "Still here",
            50,
            vec![RegionRect::new(10, 10, 10, 10)],
        )]);
        let excess = vec![region(0, "Excess", 1, Vec::new()); MAX_REGIONS + 1];

        let error = regions
            .try_set(excess)
            .expect_err("RegionId cannot name this set");
        assert_eq!(
            error,
            TooManyRegions {
                found:   MAX_REGIONS + 1,
                maximum: MAX_REGIONS,
            }
        );
        assert_eq!(
            regions.len(),
            1,
            "the rejected set did not partially replace the old one"
        );
        assert_eq!(at(&regions, 15, 15), Some("Still here"));
    }

    #[test]
    fn registering_again_replaces_rather_than_stacks() {
        let mut regions = Regions::new(1024, 1024);
        regions.set(vec![region(
            0,
            "Old",
            50,
            vec![RegionRect::new(100, 100, 50, 50)],
        )]);
        regions.set(vec![region(
            0,
            "New",
            50,
            vec![RegionRect::new(100, 100, 50, 50)],
        )]);

        assert_eq!(regions.len(), 1);
        assert_eq!(at(&regions, 120, 120), Some("New"));
    }

    #[test]
    fn clearing_leaves_nothing_to_find() {
        let mut regions = Regions::new(1024, 1024);
        regions.set(vec![region(
            0,
            "Britain",
            50,
            vec![RegionRect::new(100, 100, 50, 50)],
        )]);
        regions.clear();

        assert!(regions.is_empty());
        assert_eq!(at(&regions, 120, 120), None);
    }

    #[test]
    fn a_region_spanning_many_buckets_is_found_in_all_of_them() {
        // The grid is an accelerator: a rectangle wider than a sector has to be
        // registered in every bucket it touches, or a lookup in the middle of a
        // town would miss it.
        let mut regions = Regions::new(4096, 4096);
        regions.set(vec![region(
            0,
            "Wide",
            50,
            vec![RegionRect::new(0, 0, 1000, 1000)],
        )]);

        for tile in [(1, 1), (300, 300), (700, 120), (999, 999)] {
            assert_eq!(at(&regions, tile.0, tile.1), Some("Wide"), "at {tile:?}");
        }
    }

    #[test]
    fn an_unsized_index_still_answers_correctly() {
        // `Default` is a one-bucket grid. Everything clamps into it, so lookups
        // are unaccelerated but never wrong — which is what makes a facet built
        // before its map is loaded safe.
        let mut regions = Regions::default();
        regions.set(vec![region(
            0,
            "Britain",
            50,
            vec![RegionRect::new(5000, 3000, 50, 50)],
        )]);

        assert_eq!(at(&regions, 5010, 3010), Some("Britain"));
        assert_eq!(at(&regions, 10, 10), None);
    }
}
