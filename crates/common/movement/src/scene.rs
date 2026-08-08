//! A few tiles of ground with known geometry, to walk over.
//!
//! # Why this exists
//!
//! Every terrain rule in this crate was tested against one of two things: an
//! [`OpenWorld`](crate::OpenWorld), which has no heights at all, or a real client
//! install, which has every height there is and cannot be shaped to ask a
//! question. The gap between them is where three real bugs lived — a staircase
//! walked through, a staircase entered from the side, a wall with a hole in it —
//! and all three were found by a person walking Britain's castle and looking,
//! because nothing else could see them. The real-map tests that pin them now
//! skip wherever `OPENSHARD_CLIENT` is unset, CI included.
//!
//! A scene is the missing middle: ground at a height you chose, a stair you put
//! there, a band of wall you placed, in a map small enough to print. It ships no
//! client files and needs none, so it runs everywhere, and — this is the whole
//! point — it goes through the same [`MapTerrain`](crate::MapTerrain) the shard
//! and the client use. A fixture that reimplemented the rule would agree with
//! itself and prove nothing.
//!
//! ```
//! use openshard_movement::scene::{SIDE, Scene};
//! use openshard_protocol::world::Point;
//!
//! // Flat ground with a wall clean across it — a wall with a gap is a door,
//! // and a body walks around it.
//! let mut scene = Scene::flat(0);
//! for x in 0..SIDE {
//!     scene.wall(x, 2, 0, 20);
//! }
//! // From the north side, the south side is unreachable.
//! let walkable = scene.reachable(Point::new(1, 1, 0));
//! assert!(walkable.contains_key(&(7, 0)), "the north side is one room");
//! assert!(!walkable.contains_key(&(1, 3)), "and the wall is the end of it");
//! ```
//!
//! # What it is not
//!
//! Not a stand-in for the real map. A scene says what the rule does with a shape;
//! only Britannia says which shapes exist. Both are tests worth having, and the
//! real-file ones in `terrain.rs` are the ones that would notice a tiledata flag
//! being read out of the wrong byte.

use std::collections::BTreeMap;

use openshard_protocol::direction::Direction;
use openshard_protocol::world::Point;
use openshard_uofiles::map::{LandCell, Map, StaticItem};
use openshard_uofiles::tiledata::{StaticTile, TileData, TileFlags};

use crate::terrain::MapTerrain;

/// The side of the square a scene covers, in tiles.
///
/// One map block, which is the smallest [`Map::from_blocks`] can build. Big
/// enough for a staircase and the wall beside it, small enough that
/// [`Scene::picture`] fits on a terminal.
pub const SIDE: u16 = 8;

/// A small square of ground, and what stands on it.
///
/// Built with [`Scene::flat`] and shaped with [`Scene::ground`],
/// [`Scene::floor`], [`Scene::stair`] and [`Scene::wall`]; walked with
/// [`Scene::terrain`], [`Scene::reachable`] and [`Scene::picture`].
#[derive(Debug)]
pub struct Scene {
    map: Map,
    tiles: TileData,
    /// The next graphic id to hand out. Each distinct kind-and-height of static
    /// gets its own entry in the tiledata, because that is where a static's
    /// height and flags actually live — the same indirection the real files
    /// have, so a scene cannot accidentally test a shortcut the shard does not
    /// take.
    next_graphic: u16,
}

impl Scene {
    /// Flat ground at `z` across the whole square, with nothing on it.
    #[must_use]
    pub fn flat(z: i8) -> Self {
        // Land tile 0 with the default (empty) tiledata: not water, not
        // blocking, so it is ordinary walkable ground.
        let map = Map::from_blocks(1, 1, |_, _| LandCell { tile: 0, z });
        Self {
            map,
            tiles: TileData::empty(),
            next_graphic: 1,
        }
    }

    /// Move one tile's ground to `z`. Its neighbours are unchanged, so this is
    /// also how a scene gets a slope — a land tile's corners are its neighbours'
    /// heights, exactly as on the real map.
    pub fn ground(&mut self, x: u16, y: u16, z: i8) -> &mut Self {
        self.map.set_land(x, y, LandCell { tile: 0, z });
        self
    }

    /// A solid platform `height` tall based at `base`: a floor, a table, a pier.
    /// You stand on its top.
    pub fn floor(&mut self, x: u16, y: u16, base: i8, height: u8) -> &mut Self {
        self.place(x, y, base, height, TileFlags::PLATFORM)
    }

    /// A climbable platform — a stair, a ramp, tiledata's "bridge". You step onto
    /// its base and stand half way up it.
    pub fn stair(&mut self, x: u16, y: u16, base: i8, height: u8) -> &mut Self {
        self.place(x, y, base, height, TileFlags::PLATFORM | TileFlags::CLIMBABLE)
    }

    /// A wall: solid, and nothing stands on it.
    pub fn wall(&mut self, x: u16, y: u16, base: i8, height: u8) -> &mut Self {
        self.place(x, y, base, height, TileFlags::WALL | TileFlags::BLOCK)
    }

    fn place(&mut self, x: u16, y: u16, base: i8, height: u8, flags: u64) -> &mut Self {
        let graphic = self.next_graphic;
        self.next_graphic += 1;
        self.tiles.set_static_tile(
            graphic,
            StaticTile {
                flags: TileFlags::new(flags),
                height,
                ..StaticTile::default()
            },
        );
        self.map.place_static(StaticItem {
            tile: graphic,
            x,
            y,
            z: base,
            hue: 0,
        });
        self
    }

    /// The terrain to ask, borrowing this scene.
    #[must_use]
    pub fn terrain(&self) -> MapTerrain<&Map, &TileData> {
        MapTerrain::new(&self.map, &self.tiles)
    }

    /// The map, for a test that wants to read the scene back rather than ask the
    /// rule about it — which is what an independent oracle has to do.
    #[must_use]
    pub const fn map(&self) -> &Map {
        &self.map
    }

    /// The tile definitions, for the same reason.
    #[must_use]
    pub const fn tiles(&self) -> &TileData {
        &self.tiles
    }

    /// Every tile a body at `from` can walk to, and the height it stands at
    /// there — a flood fill over [`step_allowed`](crate::step_allowed), so it is
    /// the *whole* step rule including the corner rule a diagonal has to pass.
    ///
    /// `from` itself is always in the answer, at its own `z`: a body is standing
    /// where it is standing. Whether it *could* have got there is a different
    /// question, and one [`Scene::terrain`]'s `surface_at` answers.
    #[must_use]
    pub fn reachable(&self, from: Point) -> BTreeMap<(u16, u16), i8> {
        let terrain = self.terrain();
        let mut found = BTreeMap::new();
        found.insert((from.x, from.y), from.z);
        let mut queue = vec![from];
        while let Some(here) = queue.pop() {
            for direction in Direction::ALL {
                let Some(next) = crate::step_allowed(&terrain, here, direction) else {
                    continue;
                };
                if found.insert((next.x, next.y), next.z).is_none() {
                    queue.push(next);
                }
            }
        }
        found
    }

    /// [`Scene::reachable`], drawn: one field per tile, the height a body stands
    /// at there, `##` where it cannot go and `@` on the tile it started from.
    ///
    /// For reading a failure rather than for asserting on — though a scene is
    /// small enough that a picture is a perfectly good assertion, and one a
    /// person can check by eye, which a list of coordinates is not.
    #[must_use]
    pub fn picture(&self, from: Point) -> String {
        let walkable = self.reachable(from);
        let mut out = String::new();
        for y in 0..SIDE {
            for x in 0..SIDE {
                let field = match walkable.get(&(x, y)) {
                    _ if (x, y) == (from.x, from.y) => "@".to_owned(),
                    Some(z) => z.to_string(),
                    None => "##".to_owned(),
                };
                out.push_str(&format!("{field:>3}"));
            }
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Terrain;

    /// The scene machinery itself: ground at the height it was asked for, and a
    /// static whose flags and height came back out of the tiledata.
    #[test]
    fn a_flat_scene_is_flat_and_a_static_is_what_it_was_placed_as() {
        let mut scene = Scene::flat(-7);
        scene.stair(3, 3, -7, 10);
        assert_eq!(scene.map().land(3, 3).unwrap().z, -7);
        let item = scene.map().statics_at(3, 3).next().expect("the stair is there");
        assert_eq!(item.z, -7);
        let tile = scene.tiles().static_tile(item.tile);
        assert_eq!(tile.height, 10);
        assert!(tile.flags.is_platform() && tile.flags.is_climbable());
        // And it is walkable ground rather than a hole: standing on the flat.
        assert_eq!(scene.terrain().surface_at(0, 0, -7), Some(-7));
    }

    /// A wall across the middle cuts the square in two, and the flood fill says
    /// so — which is the assertion every scene test is a variation of.
    #[test]
    fn a_wall_across_a_flat_scene_is_a_wall() {
        let mut scene = Scene::flat(0);
        for x in 0..SIDE {
            scene.wall(x, 4, 0, 20);
        }
        let walkable = scene.reachable(Point::new(0, 0, 0));
        assert_eq!(
            walkable.len(),
            (SIDE * 4) as usize,
            "the north half, and nothing else"
        );
        assert!(walkable.keys().all(|&(_, y)| y < 4));
    }

    /// A staircase climbed one riser at a time, which is what a `climbable`
    /// static is for: five units of stair per tile, stepped onto at its base and
    /// stood on half way up.
    #[test]
    fn a_flight_of_stairs_is_climbed_tile_by_tile() {
        let mut scene = Scene::flat(0);
        for (i, x) in (1..5u16).enumerate() {
            let base = (i as i8) * 5;
            scene.stair(x, 4, base, 5);
        }
        let terrain = scene.terrain();
        let mut at = Point::new(0, 4, 0);
        for x in 1..5u16 {
            let landed = terrain
                .can_step(at, Point::new(x, 4, at.z))
                .unwrap_or_else(|| panic!("the step onto ({x},4) from {at:?} was refused"));
            assert_eq!(
                landed.z,
                (x as i8 - 1) * 5 + 2,
                "a stair based at {} is stood on half way up",
                (x as i8 - 1) * 5,
            );
            at = landed;
        }
    }

    #[test]
    fn a_picture_of_an_empty_scene_is_the_whole_square() {
        let scene = Scene::flat(0);
        let picture = scene.picture(Point::new(0, 0, 0));
        assert_eq!(picture.lines().count(), SIDE as usize);
        assert!(!picture.contains("##"), "nothing blocks a flat scene:\n{picture}");
        assert!(picture.starts_with("  @"), "the start is marked:\n{picture}");
    }
}
