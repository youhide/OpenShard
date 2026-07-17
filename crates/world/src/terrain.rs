//! Whether a mobile can actually stand somewhere.

use openshard_movement::Terrain;
use openshard_protocol::Point;

use crate::map::Map;
use crate::tiledata::TileData;

/// How far a walking human can step up.
///
/// Sphere: `if (blockingState->m_Bottom.m_z > ptDest->m_z + m_zClimbHeight + 2)`
/// — "too high to climb". A normal human has a climb height of zero, so two
/// units is the whole allowance. Anything taller needs stairs.
pub const MAX_STEP_UP: i32 = 2;

/// How much room a human needs to fit under something.
///
/// Sphere's `PLAYER_HEIGHT`. Its own comment says this should vary by creature
/// and does not.
pub const PLAYER_HEIGHT: i32 = 16;

/// A drop longer than this is a cliff, not a step.
///
/// Not a Sphere constant — Sphere lets you fall and takes the damage out of you,
/// which needs a combat system to be worth anything. Until then, refusing the
/// step keeps a player out of geometry they cannot climb back out of.
pub const MAX_STEP_DOWN: i32 = 20;

/// The real world: ground heights, walls, water.
///
/// Owns its map and tile data rather than borrowing, so the thing handed to
/// `Walker::request` has no lifetime and can live in a struct field.
#[derive(Debug)]
pub struct MapTerrain {
    map: Map,
    tiles: TileData,
    /// Whether water counts as ground. A boat or a fish says yes.
    swimming: bool,
}

impl MapTerrain {
    /// Wrap a loaded map.
    pub const fn new(map: Map, tiles: TileData) -> Self {
        Self {
            map,
            tiles,
            swimming: false,
        }
    }

    /// Let this terrain treat water as standable.
    pub const fn swimming(mut self, swimming: bool) -> Self {
        self.swimming = swimming;
        self
    }

    /// The map.
    pub const fn map(&self) -> &Map {
        &self.map
    }

    /// The tile definitions.
    pub const fn tiles(&self) -> &TileData {
        &self.tiles
    }

    /// The height a mobile would stand at, and whether it can stand there at all.
    ///
    /// Walks every surface on the tile — the ground and each static — and takes
    /// the highest one that is reachable from `from_z`. That is what makes a
    /// bridge walkable while the water under it is not.
    pub fn surface_at(&self, x: u16, y: u16, from_z: i32) -> Option<i32> {
        let land = self.map.land(x, y)?;
        let land_flags = self.tiles.land(land.tile).flags;

        let mut best: Option<i32> = None;
        let mut consider = |z: i32| {
            // Reachable means: not more than two units up, and not off a cliff.
            if z > from_z + MAX_STEP_UP || z < from_z - MAX_STEP_DOWN {
                return;
            }
            if best.is_none_or(|current| z > current) {
                best = Some(z);
            }
        };

        // The ground, unless it is water we cannot swim in or is flagged
        // impassable outright.
        let land_is_ground = if land_flags.is_water() {
            self.swimming
        } else {
            !land_flags.is_blocking()
        };
        if land_is_ground {
            consider(i32::from(land.z));
        }

        for item in self.map.statics_at(x, y) {
            let tile = self.tiles.static_tile(item.tile);
            if !tile.flags.is_platform() {
                continue;
            }
            // Sphere halves the height of stairs: you end up standing half way
            // up a step, not on top of it. Without this every staircase is a
            // wall of two-unit risers you cannot climb.
            let height = if tile.flags.is_climbable() {
                i32::from(tile.height) / 2
            } else {
                i32::from(tile.height)
            };
            consider(i32::from(item.z) + height);
        }

        best
    }

    /// Whether anything on this tile would be in a mobile's way at `z`.
    ///
    /// A static blocks if its body overlaps the space the mobile occupies —
    /// `z` to `z + PLAYER_HEIGHT`. A wall whose top is below your feet is a step,
    /// not an obstacle, and one whose base is above your head is a ceiling.
    fn is_obstructed(&self, x: u16, y: u16, z: i32) -> bool {
        self.map.statics_at(x, y).any(|item| {
            let tile = self.tiles.static_tile(item.tile);
            if !tile.flags.is_blocking() {
                return false;
            }
            // Something you can stand on is not in your way once you are on it.
            if tile.flags.is_platform() {
                return false;
            }
            // An arch or doorway is a hole in a wall, not a wall.
            if tile.flags.has(crate::tiledata::TileFlags::WINDOW) {
                return false;
            }
            let bottom = i32::from(item.z);
            let top = bottom + i32::from(tile.height).max(1);
            // Overlap between [bottom, top) and [z, z + PLAYER_HEIGHT).
            bottom < z + PLAYER_HEIGHT && z < top
        })
    }
}

impl Terrain for MapTerrain {
    fn can_step(&self, from: Point, to: Point) -> Option<Point> {
        let from_z = i32::from(from.z);
        let landing = self.surface_at(to.x, to.y, from_z)?;
        if self.is_obstructed(to.x, to.y, landing) {
            return None;
        }
        // `surface_at` already refused anything out of reach, so this cannot
        // truncate a legal step: the range is `from_z - 20 ..= from_z + 2`.
        let z = i8::try_from(landing).ok()?;
        Some(Point {
            x: to.x,
            y: to.y,
            z,
        })
    }

    fn ground_z(&self, x: u16, y: u16) -> Option<i8> {
        self.map().land(x, y).map(|cell| cell.z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Point `OPENSHARD_CLIENT` at a UO client install to run these.
    ///
    /// They skip when it is unset. A synthetic map cannot tell you the parser is
    /// right — only a real facet can — but a test that fails on any machine
    /// without a couple of gigabytes of client files is worse than no test, and
    /// there is no path that is correct for two people.
    ///
    /// Read at runtime rather than compile time so that setting the variable does
    /// not need a rebuild.
    fn client_dir() -> Option<std::path::PathBuf> {
        let dir = std::path::PathBuf::from(std::env::var_os("OPENSHARD_CLIENT")?);
        dir.join("tiledata.mul").exists().then_some(dir)
    }

    fn load_client(swimming: bool) -> Option<MapTerrain> {
        let dir = client_dir()?;
        let map = Map::load_facet(&dir, 0).expect("the client's map0 should load");
        let tiles = TileData::load(dir.join("tiledata.mul")).expect("tiledata should load");
        Some(MapTerrain::new(map, tiles).swimming(swimming))
    }

    fn real_terrain() -> Option<MapTerrain> {
        load_client(false)
    }

    #[test]
    fn sphere_constants_are_what_sphere_says() {
        assert_eq!(MAX_STEP_UP, 2, "CCharStatus.cpp: `+ m_zClimbHeight + 2`");
        assert_eq!(PLAYER_HEIGHT, 16, "uofiles_macros.h");
    }

    #[test]
    fn most_of_britain_is_walkable() {
        // Not a fixed coordinate. Facets differ: (1475, 1774) is the classic
        // Britain centre and on some maps it is open water, with a water static
        // sitting on blocking ground. Hard-coding a landmark is how you write a
        // test that only passes against one particular set of files.
        //
        // The property that actually holds for any Britannia: a city is mostly
        // ground you can stand on. Neither an all-blocking map (a bad tiledata
        // read) nor an all-open one (an `OpenWorld` in disguise) passes this.
        let Some(terrain) = real_terrain() else {
            return;
        };

        let mut walkable = 0;
        let mut total = 0;
        for y in 1600..1900u16 {
            for x in 1350..1600u16 {
                let Some(cell) = terrain.map().land(x, y) else {
                    continue;
                };
                total += 1;
                if terrain.surface_at(x, y, i32::from(cell.z)).is_some() {
                    walkable += 1;
                }
            }
        }
        let percent = 100 * walkable / total;
        assert!(
            (40..95).contains(&percent),
            "{percent}% of the Britain box is walkable; \
             under 40 means the map is not loading, over 95 means nothing blocks"
        );
    }

    #[test]
    fn standing_on_the_ground_you_are_on_is_always_allowed() {
        // The z you ask from matters: `surface_at(x, y, 0)` on ground at z=10 is
        // correctly None, because ten is more than a two-unit step up. Asking
        // from the ground's own height is the question that should always work.
        let Some(terrain) = real_terrain() else {
            return;
        };

        let mut checked = 0;
        for y in (1600..1900u16).step_by(7) {
            for x in (1350..1600u16).step_by(7) {
                let cell = terrain.map().land(x, y).unwrap();
                let flags = terrain.tiles().land(cell.tile).flags;
                if flags.is_blocking() || flags.is_water() {
                    continue;
                }
                assert!(
                    terrain.surface_at(x, y, i32::from(cell.z)).is_some(),
                    "({x},{y}) is plain ground at z={} and cannot be stood on",
                    cell.z
                );
                checked += 1;
            }
        }
        assert!(checked > 100, "only {checked} plain-ground tiles found");
    }

    #[test]
    fn the_map_is_the_facet_the_arithmetic_predicted() {
        let Some(terrain) = real_terrain() else {
            return;
        };
        assert_eq!(
            (terrain.map().width(), terrain.map().height()),
            (7168, 4096)
        );
        assert_eq!(terrain.map().facet_name(), "Felucca/Trammel (post-ML)");
    }

    #[test]
    fn a_walking_human_cannot_stand_on_the_ocean() {
        let Some(terrain) = real_terrain() else {
            return;
        };
        // Deep ocean west of Britannia. Water is BLOCK|WATER in tiledata, so a
        // walker gets nothing and a swimmer gets a surface.
        let mut wet = 0;
        let mut dry = 0;
        for x in 60..160u16 {
            for y in 60..160u16 {
                let cell = terrain.map().land(x, y).unwrap();
                if terrain.tiles().land(cell.tile).flags.is_water() {
                    wet += 1;
                    assert_eq!(
                        terrain.surface_at(x, y, i32::from(cell.z)),
                        None,
                        "({x},{y}) is water and a walker stood on it"
                    );
                } else {
                    dry += 1;
                }
            }
        }
        assert!(wet > 0, "expected some ocean in the far west; found none");
        let _ = dry;
    }

    #[test]
    fn a_swimmer_can_stand_where_a_walker_cannot() {
        let Some(terrain) = real_terrain() else {
            return;
        };
        let swimming = load_client(true).unwrap();

        let mut found = false;
        for x in 60..160u16 {
            for y in 60..160u16 {
                let cell = terrain.map().land(x, y).unwrap();
                if !terrain.tiles().land(cell.tile).flags.is_water() {
                    continue;
                }
                let z = i32::from(cell.z);
                assert_eq!(terrain.surface_at(x, y, z), None, "a walker");
                assert_eq!(swimming.surface_at(x, y, z), Some(z), "a swimmer");
                found = true;
            }
        }
        assert!(found, "expected some ocean; found none");
    }

    #[test]
    fn the_map_is_not_degenerate() {
        // This exists because the smoothness test below passed against a map0.mul
        // that was 90MB of zeroes. All-zero terrain is perfectly smooth, so the
        // test proved nothing at all while looking green.
        //
        // Any statistical check on real data needs a companion that says the data
        // is real. This is that companion.
        let Some(terrain) = real_terrain() else {
            return;
        };

        let mut tiles = std::collections::HashSet::new();
        let mut heights = std::collections::HashSet::new();
        for y in (0..4096u16).step_by(64) {
            for x in (0..7168u16).step_by(64) {
                let cell = terrain.map().land(x, y).unwrap();
                tiles.insert(cell.tile);
                heights.insert(cell.z);
            }
        }
        assert!(
            tiles.len() > 20,
            "only {} distinct land tiles across the whole facet; the map is a stub",
            tiles.len()
        );
        assert!(
            heights.len() > 5,
            "only {} distinct heights across the whole facet; the map is flat",
            heights.len()
        );
    }

    #[test]
    fn the_ground_is_smooth_which_proves_the_block_order() {
        // The real check on the column-major indexing. Terrain is continuous:
        // neighbouring tiles are within a few units of each other. If the block
        // order were transposed the file would still parse, every read would
        // still land in bounds, and this is what would catch it — the heights
        // would be scattered noise.
        //
        // Only meaningful alongside `the_map_is_not_degenerate`: a flat map is
        // smooth no matter how you index it.
        let Some(terrain) = real_terrain() else {
            return;
        };

        let mut steps = 0u32;
        let mut jumps = 0u32;
        for y in 1500..1600u16 {
            for x in 1400..1500u16 {
                let here = terrain.map().land(x, y).unwrap().z;
                let east = terrain.map().land(x + 1, y).unwrap().z;
                let south = terrain.map().land(x, y + 1).unwrap().z;
                for neighbour in [east, south] {
                    steps += 1;
                    if (i32::from(here) - i32::from(neighbour)).abs() > 10 {
                        jumps += 1;
                    }
                }
            }
        }
        let percent = 100.0 * f64::from(jumps) / f64::from(steps);
        assert!(
            percent < 2.0,
            "{jumps}/{steps} neighbouring tiles jump more than 10z ({percent:.1}%); \
             the map is probably transposed"
        );
    }

    #[test]
    fn britain_has_walls_you_cannot_walk_through() {
        // Not a fixed coordinate: statics move between client versions. Sweep
        // the city and assert that *something* blocks, which is the property
        // that matters — an OpenWorld would find nothing.
        let Some(terrain) = real_terrain() else {
            return;
        };

        let mut blocked = 0;
        for y in 1700..1850u16 {
            for x in 1400..1550u16 {
                let Some(ground) = terrain.map().land(x, y) else {
                    continue;
                };
                if terrain.is_obstructed(x, y, i32::from(ground.z)) {
                    blocked += 1;
                }
            }
        }
        assert!(
            blocked > 100,
            "only {blocked} blocked tiles in Britain; the statics are not loading"
        );
    }

    #[test]
    fn britain_has_statics_at_all() {
        let Some(terrain) = real_terrain() else {
            return;
        };
        assert!(
            terrain.map().static_count() > 1_000_000,
            "Felucca should hold millions of statics, found {}",
            terrain.map().static_count()
        );
        // The starting tile is inside the city; something should be near it.
        let nearby: usize = (1470..1480)
            .flat_map(|x| (1770..1780).map(move |y| (x, y)))
            .map(|(x, y)| terrain.map().statics_at(x, y).count())
            .sum();
        assert!(nearby > 0, "no statics anywhere near Britain's centre");
    }

    #[test]
    fn a_step_up_is_limited_to_two_units() {
        let Some(terrain) = real_terrain() else {
            return;
        };
        // Find a tile whose ground is well above its neighbour, and prove the
        // walker cannot climb it from below.
        for y in 1500..1700u16 {
            for x in 1400..1600u16 {
                let here = i32::from(terrain.map().land(x, y).unwrap().z);
                let there = i32::from(terrain.map().land(x + 1, y).unwrap().z);
                if there > here + MAX_STEP_UP && there < here + MAX_STEP_UP + 10 {
                    assert_eq!(
                        terrain.surface_at(x + 1, y, here),
                        None,
                        "({x},{y}) z={here} climbed to z={there}, more than {MAX_STEP_UP}"
                    );
                    return;
                }
            }
        }
    }

    #[test]
    fn off_the_map_is_not_standable() {
        let Some(terrain) = real_terrain() else {
            return;
        };
        assert_eq!(terrain.surface_at(7168, 0, 0), None, "past the east edge");
        assert_eq!(terrain.surface_at(0, 4096, 0), None, "past the south edge");
        assert_eq!(terrain.surface_at(u16::MAX, u16::MAX, 0), None);
    }
}
