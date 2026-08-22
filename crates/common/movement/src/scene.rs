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
use std::sync::Arc;

use openshard_protocol::direction::Direction;
use openshard_protocol::wire::{Graphic, Hue};
use openshard_protocol::world::Point;
use openshard_uofiles::grid::BlockExtent;
use openshard_uofiles::map::{LandCell, LandTile, Map, StaticItem};
use openshard_uofiles::tiledata::{StaticTile, TileData, TileFlags};

use crate::terrain::MapTerrain;

/// The side of the square [`Scene::flat`] covers, in tiles.
///
/// One map block, which is the smallest [`Map::from_blocks`] can build. Big
/// enough for a staircase and the wall beside it, small enough that
/// [`Scene::picture`] fits on a terminal. A scene that has to reach further asks
/// for the size it needs — see [`Scene::flat_holding`].
pub const SIDE: u16 = 8;

/// A square of ground, and what stands on it.
///
/// Built with [`Scene::flat`] or [`Scene::flat_holding`] and shaped with
/// [`Scene::ground`], [`Scene::land`], [`Scene::floor`], [`Scene::stair`],
/// [`Scene::wall`] and the pair [`Scene::art`]/[`Scene::put`]; walked with
/// [`Scene::terrain`], [`Scene::reachable`] and [`Scene::picture`], or handed to
/// a shard with [`Scene::into_shard`].
#[derive(Debug)]
pub struct Scene {
    map: Map,
    tiles: TileData,
    /// The next graphic id to hand out. Each distinct kind-and-height of static
    /// gets its own entry in the tiledata, because that is where a static's
    /// height and flags actually live — the same indirection the real files
    /// have, so a scene cannot accidentally test a shortcut the shard does not
    /// take.
    ///
    /// Ids named by the caller ([`Scene::art`]) are written straight into the
    /// table and do not move this: a fixture that has to be *this* graphic —
    /// a forge, a door frame, an ore vein — is matching a domain table against
    /// the id, and an id chosen by a counter would match nothing.
    next_graphic: u16,
}

impl Scene {
    /// Flat ground at `z` across one block, [`SIDE`] tiles square.
    #[must_use]
    pub fn flat(z: i8) -> Self {
        Self::flat_over(BlockExtent { wide: 1, down: 1 }, z)
    }

    /// Flat ground at `z` across `extent` blocks.
    ///
    /// The map is built in blocks because [`Map::from_blocks`] is — a facet is a
    /// whole number of them, and a scene that pretended otherwise would be a
    /// shape no map can have.
    #[must_use]
    pub fn flat_over(extent: BlockExtent, z: i8) -> Self {
        // Land tile 0 with the default (empty) tiledata: not water, not
        // blocking, so it is ordinary walkable ground.
        let map = Map::from_blocks(extent, |_, _| LandCell { tile: LandTile(0), z });
        Self {
            map,
            tiles: TileData::empty(),
            next_graphic: 1,
        }
    }

    /// Flat ground at `z` on a scene big enough that `(x, y)` is on it.
    ///
    /// The size a fixture actually knows is the coordinate it wants to use —
    /// (10, 10) for a house, (102, 100) for a door frame — so the rounding up to
    /// whole blocks is done here rather than at every caller.
    #[must_use]
    pub fn flat_holding(x: u16, y: u16, z: i8) -> Self {
        let blocks = |tile: u16| u32::from(tile / SIDE + 1);
        Self::flat_over(
            BlockExtent {
                wide: blocks(x),
                down: blocks(y),
            },
            z,
        )
    }

    /// How wide this scene is, in tiles. [`SIDE`] for a [`Scene::flat`].
    #[must_use]
    pub fn width(&self) -> u16 {
        self.map.width() as u16
    }

    /// How tall this scene is, in tiles.
    #[must_use]
    pub fn height(&self) -> u16 {
        self.map.height() as u16
    }

    /// Move one tile's ground to `z`. Its neighbours are unchanged, so this is
    /// also how a scene gets a slope — a land tile's corners are its neighbours'
    /// heights, exactly as on the real map.
    ///
    /// The tile's land id is left alone: height and identity are two facts about
    /// the same cell, and setting one is not a statement about the other.
    pub fn ground(&mut self, x: u16, y: u16, z: i8) -> &mut Self {
        let cell = self.cell(x, y);
        self.map.set_land(x, y, LandCell { z, ..cell });
        self
    }

    /// Say which land tile one cell *is* — a road, a furrow, water — leaving its
    /// height alone.
    ///
    /// The id is the whole answer to `land_tile`, which is what a domain table
    /// matches against: housing's road ranges, harvesting's sand and mountain
    /// lists. What the id can *do* is a second question, and [`Scene::land_art`]
    /// is where it is answered.
    pub fn land(&mut self, x: u16, y: u16, tile: u16) -> &mut Self {
        let cell = self.cell(x, y);
        self.map.set_land(
            x,
            y,
            LandCell {
                tile: LandTile(tile),
                ..cell
            },
        );
        self
    }

    /// The same, everywhere: the whole scene becomes road, or sea, or grass.
    ///
    /// Most fixtures want one land id under the entire square — they are asking
    /// what a rule does about a *kind* of ground, not about a border between two
    /// kinds.
    pub fn land_everywhere(&mut self, tile: u16) -> &mut Self {
        for y in 0..self.height() {
            for x in 0..self.width() {
                self.land(x, y, tile);
            }
        }
        self
    }

    /// What a land id can do, in the tiledata: [`TileFlags::WATER`] makes it sea,
    /// [`TileFlags::BLOCK`] makes it ground nothing stands on.
    ///
    /// Without this a scene can only *name* its ground; with it the scene can
    /// make water that `land_is_water` reports and `stand_surfaces` refuses to
    /// stand on, through the real rule rather than through a fixture that
    /// overrode the rule and agreed with itself.
    pub fn land_art(&mut self, tile: u16, flags: u64) -> &mut Self {
        self.tiles.set_land_tile(
            tile,
            openshard_uofiles::tiledata::LandTile {
                flags: TileFlags::new(flags),
                ..openshard_uofiles::tiledata::LandTile::default()
            },
        );
        self
    }

    /// What a *named* static graphic is: its flags and its height.
    ///
    /// [`Scene::floor`] and friends mint an id per shape, which is right when the
    /// test is about geometry and wrong when it is about identity — a forge, an
    /// anvil, a door frame and an ore vein are all matched by id against a table
    /// the shard ships. Declare the graphic here and put copies of it down with
    /// [`Scene::put`].
    pub fn art(&mut self, graphic: u16, flags: u64, height: u8) -> &mut Self {
        self.tiles.set_static_tile(
            graphic,
            StaticTile {
                flags: TileFlags::new(flags),
                height,
                ..StaticTile::default()
            },
        );
        self
    }

    /// Put a copy of graphic `graphic` on `(x, y)`, based at `base`.
    ///
    /// The graphic is expected to have been declared with [`Scene::art`] first —
    /// an undeclared one is the empty tiledata's answer, which is a static with
    /// no flags and no height: drawn, in the way of nothing.
    pub fn put(&mut self, x: u16, y: u16, base: i8, graphic: u16) -> &mut Self {
        self.map.place_static(StaticItem {
            tile: Graphic(graphic),
            x,
            y,
            z: base,
            hue: Hue(0),
        });
        self
    }

    /// One cell as it stands, so a setter can change one of its two facts
    /// without inventing the other. A tile off the scene reads as land 0 at 0,
    /// which is what `set_land` will ignore anyway.
    fn cell(&self, x: u16, y: u16) -> LandCell {
        self.map.land(x, y).unwrap_or(LandCell {
            tile: LandTile(0),
            z: 0,
        })
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
            tile: Graphic(graphic),
            x,
            y,
            z: base,
            hue: Hue(0),
        });
        self
    }

    /// The terrain to ask, borrowing this scene.
    #[must_use]
    pub fn terrain(&self) -> MapTerrain<&Map, &TileData> {
        MapTerrain::new(&self.map, &self.tiles)
    }

    /// The terrain to ask, owning it: a scene consumed into something that has
    /// no lifetime and can therefore live in a struct field.
    ///
    /// [`Scene::terrain`] borrows, which is what a test in this crate wants and
    /// what a shard cannot use — `FacetState::terrain` is boxed and outlives
    /// every local. See [`Scene::into_shard`] for the shard's whole answer.
    #[must_use]
    pub fn into_terrain(self) -> MapTerrain<Map, Arc<TileData>> {
        self.into_shard().0
    }

    /// Everything a shard needs from a scene: the facet's ground, and the tile
    /// table.
    ///
    /// **One table, held twice.** The shard's `WorldState.tiles` and the terrain
    /// under it both answer for what a graphic is — how tall a wall stands, what
    /// a component weighs — and a fixture that built them separately could put a
    /// house on a wall the ground had never heard of. Sharing the `Arc` makes
    /// that disagreement unrepresentable rather than merely unlikely.
    #[must_use]
    pub fn into_shard(self) -> (MapTerrain<Map, Arc<TileData>>, Arc<TileData>) {
        let tiles = Arc::new(self.tiles);
        (MapTerrain::new(self.map, Arc::clone(&tiles)), tiles)
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
    /// For reading a failure rather than for asserting on — though a one-block
    /// scene is small enough that a picture is a perfectly good assertion, and
    /// one a person can check by eye, which a list of coordinates is not. A
    /// scene sized for far-off coordinates draws all of itself, and is a picture
    /// for a file rather than for a terminal.
    #[must_use]
    pub fn picture(&self, from: Point) -> String {
        let walkable = self.reachable(from);
        let mut out = String::new();
        for y in 0..self.height() {
            for x in 0..self.width() {
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
    use crate::terrain::PLAYER_HEIGHT;
    use crate::{Terrain, Tile};

    /// The scene machinery itself: ground at the height it was asked for, and a
    /// static whose flags and height came back out of the tiledata.
    #[test]
    fn a_flat_scene_is_flat_and_a_static_is_what_it_was_placed_as() {
        let mut scene = Scene::flat(-7);
        scene.stair(3, 3, -7, 10);
        assert_eq!(scene.map().land(3, 3).unwrap().z, -7);
        let item = scene.map().statics_at(3, 3).next().expect("the stair is there");
        assert_eq!(item.z, -7);
        let tile = scene.tiles().static_tile(item.tile.0);
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

    /// A scene sized by the coordinate a fixture wants to use, rather than by
    /// the one block `flat` builds: (102, 100) is on the map, and the tile past
    /// the far corner is not.
    #[test]
    fn a_scene_is_as_big_as_the_coordinate_it_was_asked_to_hold() {
        let scene = Scene::flat_holding(102, 100, 0);
        assert_eq!(scene.width(), 104, "thirteen blocks across");
        assert_eq!(scene.height(), 104);
        assert_eq!(scene.terrain().ground_z(Tile::new(102, 100)), Some(0));
        assert_eq!(
            scene.terrain().ground_z(Tile::new(104, 100)),
            None,
            "off the scene is off the map, not silently clamped"
        );
    }

    /// A static placed by id keeps it. Every fixture that matches a domain table
    /// — a forge, a door frame, an ore vein — needs *this* graphic and not the
    /// next one a counter hands out.
    #[test]
    fn a_named_graphic_comes_back_under_its_own_id() {
        const FRAME: u16 = 0x0007;
        let mut scene = Scene::flat(0);
        scene.art(FRAME, TileFlags::WALL | TileFlags::BLOCK, 20);
        scene.put(2, 2, 0, FRAME);

        let mut out = Vec::new();
        scene.terrain().statics_at(Tile::new(2, 2), &mut out);
        assert_eq!(out, vec![(Graphic(FRAME), 0)]);
        // And the id carries the tiledata the caller declared for it, which is
        // what makes the frame a wall rather than decoration.
        assert!(scene.tiles().static_tile(FRAME).flags.is_blocking());
        assert!(
            !scene.terrain().can_fit(Tile::new(2, 2), 0, PLAYER_HEIGHT),
            "a body does not fit where the wall stands"
        );
    }

    /// The land id is the whole of `land_tile`'s answer, and a road id is what
    /// makes a plot a street to the table that refuses houses on one.
    #[test]
    fn ground_can_be_told_which_land_tile_it_is() {
        const ROAD: u16 = 0x0071;
        let mut scene = Scene::flat(0);
        scene.land_everywhere(ROAD);
        scene.land(3, 3, 0x0003);
        let terrain = scene.terrain();
        assert_eq!(terrain.land_tile(Tile::new(0, 0)), Some(LandTile(ROAD)));
        assert_eq!(terrain.land_tile(Tile::new(3, 3)), Some(LandTile(0x0003)));
        // Naming a tile did not move it: the heights are still the flat scene's.
        assert_eq!(terrain.ground_z(Tile::new(3, 3)), Some(0));
    }

    /// Water through the real rule: the flag is on the land's tiledata row, and
    /// both `land_is_water` and the standing check read it from there.
    #[test]
    fn water_is_a_flag_on_the_land_and_only_a_swimmer_stands_on_it() {
        const SEA: u16 = 0x00A8;
        let mut scene = Scene::flat(-5);
        scene.land_art(SEA, TileFlags::WATER);
        scene.land_everywhere(SEA);
        scene.land(1, 0, 0);

        let terrain = scene.terrain();
        assert!(terrain.land_is_water(Tile::new(4, 4)));
        assert!(!terrain.land_is_water(Tile::new(1, 0)), "the shore is not sea");
        assert_eq!(
            terrain.surface_at(4, 4, -5),
            None,
            "a walker does not stand on water"
        );

        let swimmer = scene.terrain().swimming(true);
        assert_eq!(swimmer.surface_at(4, 4, -5), Some(-5), "a boat or a fish does");
    }

    /// Ground flagged impassable is ground nobody stands on, so a floor above it
    /// is the only surface there is — and one out of a step's reach, which is
    /// the difference between walking somewhere and being placed there.
    #[test]
    fn a_raised_floor_is_out_of_a_step_but_not_out_of_a_placement() {
        const VOID: u16 = 0x0002;
        let mut scene = Scene::flat(0);
        scene.land_art(VOID, TileFlags::BLOCK);
        scene.land_everywhere(VOID);
        scene.floor(2, 2, 0, 7);

        let terrain = scene.terrain();
        assert_eq!(
            terrain.stand_z(Tile::new(2, 2), 0),
            None,
            "seven is more than a step up from nothing"
        );
        assert_eq!(
            terrain.spawn_z(Tile::new(2, 2), 0),
            Some(7),
            "a placement reaches the floor a step cannot"
        );
    }

    /// The shard's table and the facet's ground are one table, so what a graphic
    /// is cannot depend on which of the two was asked.
    #[test]
    fn a_scene_hands_the_shard_the_same_table_its_ground_reads() {
        const WALL: u16 = 0x1234;
        let mut scene = Scene::flat(0);
        scene.art(WALL, TileFlags::WALL | TileFlags::BLOCK, 20);
        scene.put(1, 1, 0, WALL);

        let (terrain, tiles) = scene.into_shard();
        assert_eq!(tiles.static_tile(WALL).height, 20);
        assert_eq!(terrain.tiles().static_tile(WALL).height, 20);
        assert!(
            !terrain.can_fit(Tile::new(1, 1), 0, PLAYER_HEIGHT),
            "the wall the shard knows about is the wall standing on the ground"
        );
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
