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

/// For a walkable static of `height` based at `base`, the point a mobile steps
/// *onto* and the point it *stands* at — `(reach, stand)`.
///
/// They differ only for a `climbable` bridge (a stair): you step onto its low
/// *base* — the near edge of the ramp — and standing on it lifts you half way up.
/// Checking the base rather than the top is what lets a staircase be climbed one
/// step at a time, where checking the top makes each riser a wall taller than a
/// step. A solid platform (a floor, a table) has no such trick: both are its top.
/// Mirrors ServUO's `Movement.Check` (`itemTop = itemZ`, `ourZ = itemZ +
/// CalcHeight` for a bridge).
const fn platform_surface(base: i32, height: i32, climbable: bool) -> (i32, i32) {
    if climbable {
        (base, base + height / 2)
    } else {
        let top = base + height;
        (top, top)
    }
}

/// The average of two corner heights, floored toward negative infinity — RunUO's
/// `FloorAverage`. A plain `(a + b) / 2` truncates toward zero, which rounds a
/// negative slope the wrong way and disagrees with the client by a unit.
const fn floor_average(a: i32, b: i32) -> i32 {
    let sum = a + b;
    if sum < 0 {
        (sum - 1) / 2
    } else {
        sum / 2
    }
}

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

    /// The height a mobile would stand at on `(x, y)`, reachable from `from_z`.
    ///
    /// A convenience over [`check`](Self::check) for callers that have only a
    /// single z and no picture of the tile they are standing on — the map's own
    /// walkability tests. It reaches from `from_z` as both the current z and the
    /// top of the surface underfoot, which is the flat-ground case. A *walking*
    /// mobile does not go through here: [`can_step`](Self::can_step) computes the
    /// real surface it stands on first, because on a slope the top of that
    /// surface is higher than its feet, and the reach starts from the top.
    pub fn surface_at(&self, x: u16, y: u16, from_z: i32) -> Option<i32> {
        self.check(x, y, from_z, from_z)
    }

    /// The surface a mobile at `(x, y, loc_z)` is standing *on*: its base z and
    /// its top. Ported from ServUO/RunUO's `MovementImpl.GetStartZ`.
    ///
    /// The client reaches its next step not from where its feet are but from the
    /// *top* of what it stands on — a sloped land tile's highest corner, a stair's
    /// full height. The server has to start from the same place or it refuses
    /// steps up a slope the client took: `start_top` is that place. Returns
    /// `(start_z, start_top)` — the base you stand on, and the top the next step
    /// reaches from.
    fn start_surface(&self, x: u16, y: u16, loc_z: i32) -> (i32, i32) {
        let (land_z, land_center, land_top) = self.land_heights(x, y);
        let mut z_low = loc_z;
        let mut z_top = loc_z;
        let mut z_center = 0;
        let mut is_set = false;

        // The ground, if you are at or above the height you would stand on it.
        if self.land_is_ground(x, y) && loc_z >= land_center {
            z_low = land_z;
            z_center = land_center;
            z_top = land_top;
            is_set = true;
        }

        // Then the tallest static surface at or below your feet: what you are
        // really standing on if you climbed onto something.
        for item in self.map.statics_at(x, y) {
            let tile = self.tiles.static_tile(item.tile);
            if !tile.flags.is_platform() {
                continue;
            }
            let base = i32::from(item.z);
            let height = i32::from(tile.height);
            let (_, calc_top) = platform_surface(base, height, tile.flags.is_climbable());
            if (!is_set || calc_top >= z_center) && loc_z >= calc_top {
                z_low = base;
                z_center = calc_top;
                let top = base + height;
                if !is_set || top > z_top {
                    z_top = top;
                }
                is_set = true;
            }
        }

        if !is_set {
            (loc_z, loc_z)
        } else {
            (z_low, z_top.max(loc_z))
        }
    }

    /// Whether a mobile standing on the surface described by `(start_z,
    /// start_top)` may step onto `(x, y)`, and the height it lands at.
    ///
    /// A blend of the two reference engines, because the shard serves the 2D
    /// client and each got one half right for it:
    ///
    /// - **Reach** is ServUO/RunUO's: a step reaches `start_top + 2` — the top of
    ///   the surface underfoot plus a step, not the feet. Starting from the feet
    ///   refuses steps up a slope the client took, which is what rubber-banded
    ///   every hillside before this.
    /// - **Selection** is Sphere's `GetFixPoint`: among the surfaces in reach,
    ///   stand on the **highest**, not the one nearest the current height. This is
    ///   how a staircase is climbed — a stair tile carries both the floor below
    ///   and the step above, and the client takes the step. ServUO's nearest-z
    ///   rule keeps you on the floor and the client, climbing, rubber-bands back
    ///   down. On bare ground the two rules agree — there is only one surface —
    ///   so this costs the slope fix nothing.
    pub fn check(&self, x: u16, y: u16, start_z: i32, start_top: i32) -> Option<i32> {
        // Off the map is not walkable — and reading a corner off the edge below
        // would fold a neighbour's height in as if it were real ground.
        self.map.land(x, y)?;
        let (land_z, land_center, _) = self.land_heights(x, y);
        // How high a step reaches, and the headroom a body needs above its feet.
        let step_top = start_top + MAX_STEP_UP;
        let check_top = start_z + PLAYER_HEIGHT;

        let mut new_z = 0;
        let mut move_ok = false;

        for item in self.map.statics_at(x, y) {
            let tile = self.tiles.static_tile(item.tile);
            if !tile.flags.is_platform() {
                continue;
            }
            let base = i32::from(item.z);
            let height = i32::from(tile.height);
            let climbable = tile.flags.is_climbable();
            // `item_top` is the edge a step must reach; `our_z` where you stand.
            let (item_top, our_z) = platform_surface(base, height, climbable);
            // Keep the highest surface in reach: the stair over the floor.
            if move_ok && our_z <= new_z {
                continue;
            }
            let test_top = check_top.max(our_z + PLAYER_HEIGHT);
            if step_top >= item_top {
                // A low static the ground pokes through is not something you climb
                // onto: the land under it wins. ServUO's `landCheck` guard.
                let land_check = base + MAX_STEP_UP.min(height);
                if self.land_is_ground(x, y)
                    && land_check < land_center
                    && land_center > our_z
                    && test_top > land_z
                {
                    continue;
                }
                if !self.is_obstructed(x, y, our_z) {
                    new_z = our_z;
                    move_ok = true;
                }
            }
        }

        // The ground itself: reachable if a step reaches its lowest corner, and
        // you stand at its centre — the average, never the raw corner. Taken only
        // if nothing higher already won.
        if self.land_is_ground(x, y)
            && step_top >= land_z
            && (!move_ok || land_center > new_z)
            && !self.is_obstructed(x, y, land_center)
        {
            new_z = land_center;
            move_ok = true;
        }

        move_ok.then_some(new_z)
    }

    /// Whether the land at `(x, y)` is something a mobile can stand on: not water
    /// it cannot swim in, and not flagged impassable.
    fn land_is_ground(&self, x: u16, y: u16) -> bool {
        let Some(land) = self.map.land(x, y) else {
            return false;
        };
        let flags = self.tiles.land(land.tile).flags;
        if flags.is_water() {
            self.swimming
        } else {
            !flags.is_blocking()
        }
    }

    /// The land tile's `(lowest corner, floor-average, highest corner)` — RunUO's
    /// `GetAverageZ`, which returns all three. The step check reaches the lowest,
    /// stands on the average, and never looks at the raw stored corner alone.
    fn land_heights(&self, x: u16, y: u16) -> (i32, i32, i32) {
        let own = self.map.land(x, y).map_or(0, |c| i32::from(c.z));
        // A missing neighbour (the map's edge) reads as this tile's own height, so
        // the edge is flat rather than a cliff into z = 0.
        let z = |nx: u16, ny: u16| self.map.land(nx, ny).map_or(own, |c| i32::from(c.z));
        let top = own;
        let left = z(x, y.wrapping_add(1));
        let right = z(x.wrapping_add(1), y);
        let bottom = z(x.wrapping_add(1), y.wrapping_add(1));
        let min = top.min(left).min(right).min(bottom);
        let max = top.max(left).max(right).max(bottom);
        // Average the pair spanning the *gentler* slope — the one whose corners
        // are closer — so a mobile stands level along the shallow axis.
        let avg = if (top - bottom).abs() > (left - right).abs() {
            floor_average(left, right)
        } else {
            floor_average(top, bottom)
        };
        (min, avg, max)
    }

    /// The height a mobile stands at on the land tile at `(x, y)` — the *average*
    /// of the tile's four corners, not the raw south-west corner the map stores.
    ///
    /// UO land tiles are sloped diamonds; you stand at the middle of one. The
    /// client (ClassicUO, and RunUO/ServUO before it) derives this `GetAverageZ`,
    /// and the server has to agree: the walk ack carries no z, so each side
    /// computes its own, and any mismatch on a slope rubber-bands every step — the
    /// terrain "blocks" for no visible reason. Ported from RunUO's `Map.GetAverageZ`.
    fn average_land_z(&self, x: u16, y: u16) -> i32 {
        self.land_heights(x, y).1
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
        // Reach the next tile from the top of what we stand on, not from our feet:
        // on a slope those differ, and starting from the feet refuses steps up the
        // slope the client took. `start_surface` is what the client reaches from.
        let (start_z, start_top) = self.start_surface(from.x, from.y, from_z);
        let landing = self.check(to.x, to.y, start_z, start_top)?;
        let z = i8::try_from(landing).ok()?;
        Some(Point {
            x: to.x,
            y: to.y,
            z,
        })
    }

    fn ground_z(&self, x: u16, y: u16) -> Option<i8> {
        // The average, not the raw corner, so a character spawns at the same
        // height the client will compute for the tile — see `average_land_z`.
        self.map().land(x, y)?;
        i8::try_from(self.average_land_z(x, y)).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openshard_protocol::Direction;

    #[test]
    fn a_stair_is_stepped_onto_at_its_base_not_its_top() {
        // The bug the ramps in a city hit: a ten-high stair based level with your
        // feet. You step onto its base (0 — within a step) and stand half way up
        // (5). Checking the *standing* height, 5, against the two-unit limit
        // refused it; the base is what makes the whole staircase climbable.
        assert_eq!(platform_surface(0, 10, true), (0, 5));
        // A solid platform of the same height is stepped onto at its top, which is
        // out of reach from the ground — you cannot step onto a tall table.
        assert_eq!(platform_surface(0, 10, false), (10, 10));
    }

    #[test]
    fn the_land_average_floors_toward_negative_infinity() {
        // RunUO's FloorAverage, the rule the client rounds a slope by: a plain
        // truncating divide would round -3 and -4 to -3 (toward zero); the client
        // and this floor both give -4.
        assert_eq!(floor_average(4, 6), 5);
        assert_eq!(floor_average(-3, -4), -4);
        assert_eq!(floor_average(0, -1), -1);
        assert_eq!(floor_average(-10, 10), 0);
    }

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
    fn a_stack_of_surfaces_is_climbed_to_the_highest_in_reach() {
        // The rule that lets a staircase be climbed, and the one ServUO gets wrong
        // for the 2D client: a stair tile carries the floor below *and* the step
        // above, and stepping onto it must land on the step, not the floor. Find
        // real tiles with two platform surfaces both within a generous reach and
        // assert `check` returns the higher one — Sphere's `GetFixPoint`.
        let Some(t) = real_terrain() else {
            return;
        };

        let mut checked = 0;
        for y in 1580..1610u16 {
            for x in 1490..1552u16 {
                // Collect the standing heights of the platform statics here.
                let mut stands: Vec<i32> = t
                    .map()
                    .statics_at(x, y)
                    .filter_map(|item| {
                        let tile = t.tiles().static_tile(item.tile);
                        tile.flags.is_platform().then(|| {
                            platform_surface(
                                i32::from(item.z),
                                i32::from(tile.height),
                                tile.flags.is_climbable(),
                            )
                            .1
                        })
                    })
                    .collect();
                if stands.len() < 2 {
                    continue;
                }
                stands.sort_unstable();
                let (&low, &high) = (stands.first().unwrap(), stands.last().unwrap());
                if low == high || t.is_obstructed(x, y, high) {
                    continue;
                }
                // Reach from a vantage that clears the highest surface, so both are
                // in reach and the choice is purely which one you stand on.
                let start_top = high;
                if let Some(landed) = t.check(x, y, high, start_top) {
                    assert_eq!(
                        landed, high,
                        "({x},{y}) has surfaces {stands:?}; a step onto it must climb to the top",
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 10, "only {checked} stacked-surface tiles tested");
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
    fn a_step_up_a_land_slope_and_back_agree_on_the_height() {
        // The invariant the ramp rubber-band broke, on the geometry it broke on:
        // bare sloped land, no statics stacked on it. A mobile reaches its next
        // tile from the *top* of the surface it stands on, not from its feet, so a
        // step up a slope and the same step back down are mutually consistent —
        // A→B→A lands you back at A's own standing height. The old check reached
        // from the feet and stood at the average, an asymmetry that put the server
        // a unit off the client on every hillside and snapped the walk back.
        //
        // Only pure land is a fair test: where statics stack (a stair), the height
        // you land at genuinely depends on the height you came from — the client
        // does the same — so reversibility there is not an invariant at all.
        let Some(terrain) = real_terrain() else {
            return;
        };
        let bare = |x: u16, y: u16| terrain.map().statics_at(x, y).next().is_none();

        let mut checked = 0;
        let mut slopes = 0; // steps that actually change height — the ones at risk
        for y in 1600..1900u16 {
            for x in 1350..1600u16 {
                if terrain.map().land(x, y).is_none() || !bare(x, y) {
                    continue;
                }
                let a = Point::new(x, y, terrain.average_land_z(x, y) as i8);
                for dir in Direction::ALL {
                    let (dx, dy) = dir.step();
                    let (bx, by) = ((i32::from(x) + dx) as u16, (i32::from(y) + dy) as u16);
                    if terrain.map().land(bx, by).is_none() || !bare(bx, by) {
                        continue;
                    }
                    let Some(b) = terrain.can_step(a, Point::new(bx, by, a.z)) else {
                        continue;
                    };
                    let Some(returned) = terrain.can_step(b, Point::new(a.x, a.y, b.z)) else {
                        continue;
                    };
                    assert_eq!(
                        returned.z, a.z,
                        "A={a:?} -{dir:?}-> B={b:?} -> back landed at z={}, not A's z={}",
                        returned.z, a.z
                    );
                    checked += 1;
                    if b.z != a.z {
                        slopes += 1;
                    }
                }
            }
        }
        assert!(checked > 1000, "only {checked} reversible land steps found");
        assert!(
            slopes > 20,
            "only {slopes} height-changing steps — no slopes tested"
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
                // `surface_at` is obstruction-aware now — it is the whole movement
                // check — so a walkable land tile with a wall standing on it is
                // rightly not standable. Skip those: this test is about *reach*
                // from your own height, not about walls. A tile where a body would
                // stand clear is the case it means to protect.
                let stand = terrain.average_land_z(x, y);
                if terrain.is_obstructed(x, y, stand) {
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
