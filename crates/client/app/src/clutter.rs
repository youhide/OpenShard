//! What the shard has put on the ground, as something a step can be refused by.
//!
//! # Why the map is not enough
//!
//! `MapTerrain` reads the client's own files — land and static art — and nothing
//! else. A barrel is not in those files: it is an *entity* the shard placed and
//! described to us in a `0x1A`, so every terrain check this end makes looks
//! straight through it. The server does not: `openshard-state`'s `Obstructions`
//! indexes exactly these placements and `LiveTerrain` lays them over the same
//! map, which is what actually decides the step.
//!
//! Two ends deciding the same step by different rules is the whole defect. The
//! client walks its held direction into a crate, `Steering::detour` sees an open
//! tile and offers no way round, the `0x02` goes out, the shard refuses it with a
//! `0x21`, and the body shudders against the barrel for as long as the key is
//! held — the same shape of bug the corner rule had (see `steer.rs`), one layer
//! down. Walls worked only because a wall is static art and therefore in the
//! files.
//!
//! So this is the client's half of `Obstructions`, built from the one thing this
//! end has: the ground items in the [`WorldView`](openshard_client_net::view::WorldView),
//! and the same `tiledata.mul` the server read to decide they block. Same
//! predicate (`Terrain::item_blocks`), same z-span (base z, tiledata height), so
//! the two ends agree by construction rather than by resemblance — see
//! `world::tick::decor::place_decoration`, which is where the server's copy of
//! this decision is made.
//!
//! It is a *projection* of the view, like [`App::items`](crate::App::items) and
//! [`App::others`](crate::App::others): rebuilt whole whenever the view changes,
//! with nothing to keep in step by hand.
//!
//! # What is deliberately not here
//!
//! Mobiles. The shard does not block a step on another body either (nothing
//! registers one in `Obstructions`), and a client that did would refuse steps
//! the server allows — the mirror of this very bug, pointing the other way.

use std::collections::HashMap;

use openshard_client_render::items::GroundItem;
use openshard_movement::{MapTerrain, Terrain, Tile};
use openshard_protocol::world::Point;
use openshard_uofiles::map::Map;
use openshard_uofiles::tiledata::TileData;

/// A mobile's body height in z-units — how tall a span a blocker has to reach
/// into to be in the way. `openshard_movement::PLAYER_HEIGHT`, and the server's
/// `obstruct::MOBILE_HEIGHT`, which is the same number said in the same units on
/// the other end of the wire.
const MOBILE_HEIGHT: i32 = openshard_movement::PLAYER_HEIGHT;

/// One placed item blocking a tile: where its body starts and how tall it is.
///
/// No graphic and no serial: this end never has to say *what* stopped the step,
/// only that something did. The server's `Obstacle` carries an entity and a
/// `door` flag because it decides whether to open rather than walk around; a
/// client asks the shard to open a door by double-clicking it, so there is
/// nothing here for that flag to change.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Blocker {
    /// The base z the item sits at — its `0x1A` position's z.
    z: i8,
    /// Its tiledata height: its body spans `[z, z + height)`. Zero-height art
    /// still occupies its own tile, so the span is never empty — see
    /// [`Clutter::blocked_at`].
    height: u8,
}

/// The ground items that block, indexed by tile.
///
/// Built once per view update and read by every step decision, rather than
/// scanned per call: `find_path` asks about hundreds of tiles for one click, and
/// a linear pass over everything on screen for each of them is the same answer
/// bought a hundred times over.
#[derive(Default, Debug)]
pub struct Clutter {
    tiles: HashMap<(u16, u16), Vec<Blocker>>,
}

impl Clutter {
    /// Index whatever in `items` the tiledata calls impassable.
    ///
    /// The filter is `Terrain::item_blocks` — the flags, not the graphic — for
    /// the same reason the server uses it: a door's own graphic changes when it
    /// swings, so the shut leaf blocks and the open one does not without either
    /// end tracking a door's state at all.
    pub fn of(items: &[GroundItem], tiles: &TileData) -> Self {
        let mut blocked: HashMap<(u16, u16), Vec<Blocker>> = HashMap::new();
        for item in items {
            let tile = tiles.static_tile(item.graphic.0);
            if !tile.flags.is_blocking() {
                continue;
            }
            blocked.entry((item.at.x, item.at.y)).or_default().push(Blocker {
                z: item.at.z,
                height: tile.height,
            });
        }
        Self { tiles: blocked }
    }

    /// Whether anything here is in the way of a body standing at `stand_z` —
    /// its body `[z, z + height)` meeting the mobile's `[stand_z, stand_z +
    /// MOBILE_HEIGHT)`.
    ///
    /// The z-span and not the tile, so a crate on a building's upper floor
    /// leaves the ground floor beneath it open. `max(1)` on the height because
    /// tiledata gives plenty of impassable art a height of zero — a flat span
    /// would overlap nothing and block nowhere, which reads exactly like the bug
    /// this module exists to fix.
    fn blocked_at(&self, x: u16, y: u16, stand_z: i32) -> bool {
        self.tiles.get(&(x, y)).is_some_and(|blockers| {
            blockers.iter().any(|blocker| {
                let bottom = i32::from(blocker.z);
                let top = bottom + i32::from(blocker.height).max(1);
                bottom < stand_z + MOBILE_HEIGHT && stand_z < top
            })
        })
    }

    /// The map with this clutter laid over it — what every step decision on this
    /// end should actually ask.
    pub const fn over<'a>(
        &'a self,
        map: &'a Map,
        tiles: &'a TileData,
    ) -> Cluttered<'a, MapTerrain<&'a Map, &'a TileData>> {
        self.over_terrain(MapTerrain::new(map, tiles))
    }

    /// The same, over any terrain at all.
    ///
    /// What [`over`](Self::over) is written in terms of, and the reason the
    /// overlay is generic rather than nailed to `MapTerrain`: the behaviour this
    /// module exists for — a held direction *going around* a barrel instead of
    /// shuddering against it — is a property of the overlay and the detour rule
    /// together, and pinning it to a real facet would make the only test of it
    /// one that skips on every machine without a couple of gigabytes of client
    /// files. Over `OpenWorld` the ground is flat and infinite, so the one thing
    /// left that can refuse a step is the clutter itself.
    pub const fn over_terrain<M: Terrain>(&self, map: M) -> Cluttered<'_, M> {
        Cluttered { map, clutter: self }
    }
}

/// The client's map with the shard's placed items laid over it.
///
/// The client-side twin of `openshard-state`'s `LiveTerrain`, and deliberately
/// the same shape: a borrow, built per decision, delegating everything about the
/// map to the map and answering only for what the world has put on top.
#[derive(Debug)]
pub struct Cluttered<'a, M> {
    map: M,
    clutter: &'a Clutter,
}

impl<M: Terrain> Terrain for Cluttered<'_, M> {
    fn can_step(&self, from: Point, to: Point) -> Option<Point> {
        let landed = self.map.can_step(from, to)?;
        // At the height the body will stand at, not the height it asked for:
        // `to.z` is the caller's guess and `can_step` is what corrects it.
        match self.clutter.blocked_at(to.x, to.y, i32::from(landed.z)) {
            true => None,
            false => Some(landed),
        }
    }

    fn ground_z(&self, tile: Tile) -> Option<i8> {
        self.map.ground_z(tile)
    }

    fn land_tile(&self, tile: Tile) -> Option<u16> {
        self.map.land_tile(tile)
    }

    fn statics_at(&self, tile: Tile, out: &mut Vec<(u16, i8)>) {
        self.map.statics_at(tile, out);
    }

    fn stand_z(&self, tile: Tile, near_z: i32) -> Option<i32> {
        self.map.stand_z(tile, near_z)
    }

    fn spawn_z(&self, tile: Tile, near_z: i32) -> Option<i32> {
        self.map.spawn_z(tile, near_z)
    }

    fn can_fit(&self, tile: Tile, z: i32, height: i32) -> bool {
        self.map.can_fit(tile, z, height) && !self.clutter.blocked_at(tile.x, tile.y, z)
    }

    fn item_blocks(&self, graphic: openshard_protocol::wire::Graphic) -> bool {
        self.map.item_blocks(graphic)
    }

    fn item_height(&self, graphic: openshard_protocol::wire::Graphic) -> u8 {
        self.map.item_height(graphic)
    }

    fn item_name(&self, graphic: openshard_protocol::wire::Graphic) -> Option<&str> {
        self.map.item_name(graphic)
    }

    fn item_weight(&self, graphic: openshard_protocol::wire::Graphic) -> u8 {
        self.map.item_weight(graphic)
    }

    fn item_layer(&self, graphic: openshard_protocol::wire::Graphic) -> u8 {
        self.map.item_layer(graphic)
    }

    fn sight_clear(&self, from: Point, to: Point) -> bool {
        // The map's answer alone. A crate is furniture, not a wall — the same
        // line the server draws, which only treats a shut *door* as opaque, and
        // this end cannot tell a door from a barrel without the entity.
        self.map.sight_clear(from, to)
    }
}

#[cfg(test)]
impl Clutter {
    /// Index blockers by hand, with no tiledata to read flags from.
    ///
    /// For the tests that are about the *span* and the detour rather than about
    /// which art is impassable — the flag half is [`Clutter::of`]'s and is
    /// covered against the real `tiledata.mul` below. Everything downstream of
    /// here, `blocked_at` included, is the one the shipping path uses.
    fn placed(blockers: &[(Point, u8)]) -> Self {
        let mut tiles: HashMap<(u16, u16), Vec<Blocker>> = HashMap::new();
        for (at, height) in blockers {
            tiles.entry((at.x, at.y)).or_default().push(Blocker {
                z: at.z,
                height: *height,
            });
        }
        Self { tiles }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openshard_movement::{Around, Detour, Heading, Lean, Leeway, OpenWorld, Step, step_allowed};
    use openshard_protocol::direction::Direction;
    use openshard_protocol::wire::{Graphic, Hue};

    /// A barrel's tiledata height, so the span in these tests is a real one.
    const BARREL_HEIGHT: u8 = 12;

    /// Somewhere to stand, on flat open ground.
    const HERE: Point = Point::new(100, 100, 0);

    /// A held direction walked into a barrel is refused *here*, before it is
    /// ever sent — which is the whole point. Over `OpenWorld` nothing else can
    /// refuse a step, so this is the clutter and nothing but.
    #[test]
    fn a_step_into_a_barrel_is_refused_by_this_end() {
        let east = Point::new(HERE.x + 1, HERE.y, 0);
        let clutter = Clutter::placed(&[(east, BARREL_HEIGHT)]);
        let terrain = clutter.over_terrain(OpenWorld);
        assert!(
            step_allowed(&terrain, HERE, Direction::East).is_none(),
            "a barrel due east did not stop the step"
        );
        assert!(
            step_allowed(&terrain, HERE, Direction::West).is_some(),
            "a barrel due east stopped a step the other way"
        );
    }

    /// The bug as it was reported: a body walking at a barrel diagonally went
    /// straight at it every hold and was rolled back by the shard every hold,
    /// because this end could see no obstacle to go around. It goes around.
    #[test]
    fn a_diagonal_held_at_a_barrel_rounds_it() {
        let north_east = Point::new(HERE.x + 1, HERE.y - 1, 0);
        let clutter = Clutter::placed(&[(north_east, BARREL_HEIGHT)]);
        let terrain = clutter.over_terrain(OpenWorld);
        let intent = Heading {
            direction: Direction::NorthEast,
            lean: Lean::Centred,
        };
        assert!(
            step_allowed(&terrain, HERE, Direction::NorthEast).is_none(),
            "the barrel on the diagonal was not seen at all"
        );
        let around = Around::read(&terrain, HERE, intent);

        // An eighth of a turn — the default, and the smallest one that rounds
        // anything: the flanks of a blocked diagonal are North and East, both
        // open, so the body keeps moving along the barrel instead of standing.
        let step = Detour::default().step(&around, Leeway::Eighth);
        assert!(
            matches!(step, Step::Aside(Direction::North | Direction::East)),
            "a barrel on the diagonal gave {step:?} instead of a way round"
        );
    }

    /// The same barrel one storey up is not in the way — the reason a blocker
    /// carries a z-span at all. Without it, a crate on a balcony would seal the
    /// street under it, which is the mirror-image bug (the server's own, once).
    #[test]
    fn a_barrel_overhead_stops_nothing() {
        let east = Point::new(HERE.x + 1, HERE.y, 40);
        let clutter = Clutter::placed(&[(east, BARREL_HEIGHT)]);
        let terrain = clutter.over_terrain(OpenWorld);
        assert!(
            step_allowed(&terrain, HERE, Direction::East).is_some(),
            "a barrel two storeys up blocked the ground"
        );
    }

    /// A barrel: impassable in tiledata, and nothing to do with the map's own
    /// statics. Anything blocking would do — this is the one the report named.
    const BARREL: Graphic = Graphic(0x0FAE);

    fn item(x: u16, y: u16, z: i8, graphic: Graphic) -> GroundItem {
        GroundItem {
            at: Point::new(x, y, z),
            graphic,
            hue: Hue::NONE,
        }
    }

    /// The tiledata the real client ships, or nothing when no install is
    /// configured — the tests that need one skip, as everywhere else in this
    /// workspace. No client files live in this repository.
    fn client_tiledata() -> Option<TileData> {
        let dir = std::path::PathBuf::from(std::env::var_os("OPENSHARD_CLIENT")?);
        let file = dir.join("tiledata.mul");
        file.exists()
            .then(|| TileData::load(file).expect("tiledata should load"))
    }

    #[test]
    fn a_barrel_is_impassable_in_the_client_s_own_tiledata() {
        let Some(tiles) = client_tiledata() else {
            return;
        };
        // The premise the whole module rests on: the server decided this barrel
        // blocks by reading this flag, and so does the index below. Pinned here
        // so a tiledata reader that started answering differently is a failing
        // test rather than a body walking through crates again.
        assert!(
            tiles.static_tile(BARREL.0).flags.is_blocking(),
            "a barrel's tiledata does not call it impassable"
        );
    }

    /// The one that reads as a bug and is not.
    ///
    /// `water barrel` looks exactly like a barrel on screen and is walked
    /// straight through — on Britain's docks, where two of them stand a tile
    /// apart and only one stops anybody. Its tiledata carries a single flag,
    /// `0x4000`, which is `ArticleA`: it says the item's name takes "a", and
    /// nothing else. No `Impassable`, no `Surface`, height zero. Read straight
    /// out of the file with the reader out of the loop, and ServUO decides with
    /// the same predicate (`ImpassableSurface = Impassable | Surface`,
    /// `Scripts/Services/Pathing/Movement.cs`), so the reference walks through
    /// it too.
    ///
    /// Pinned so the next person to notice it finds this test instead of
    /// re-deriving it, and so that a shard which *wants* it solid knows it is
    /// changing gameplay rather than fixing a defect.
    #[test]
    fn a_water_barrel_is_walked_through_because_the_client_says_so() {
        let Some(tiles) = client_tiledata() else {
            return;
        };
        let water_barrel = Graphic(0x154D);
        let data = tiles.static_tile(water_barrel.0);
        assert_eq!(
            data.name.as_str(),
            "water barrel",
            "0x154D is not the tile this is about"
        );
        assert_eq!(
            data.flags.bits(),
            0x4000,
            "a water barrel's flags are no longer article-only"
        );
        assert!(!data.flags.is_blocking());
        let clutter = Clutter::of(&[item(100, 100, 0, water_barrel)], &tiles);
        assert!(
            !clutter.blocked_at(100, 100, 0),
            "a water barrel was made solid here and the shard would still allow the step"
        );
    }

    #[test]
    fn a_placed_barrel_blocks_its_tile_at_ground_level() {
        let Some(tiles) = client_tiledata() else {
            return;
        };
        let clutter = Clutter::of(&[item(100, 100, 0, BARREL)], &tiles);
        assert!(
            clutter.blocked_at(100, 100, 0),
            "a barrel underfoot is not in the way"
        );
        assert!(
            !clutter.blocked_at(101, 100, 0),
            "a barrel blocked a tile it is not on"
        );
    }

    #[test]
    fn a_barrel_two_storeys_up_leaves_the_ground_floor_open() {
        let Some(tiles) = client_tiledata() else {
            return;
        };
        let clutter = Clutter::of(&[item(100, 100, 40, BARREL)], &tiles);
        assert!(
            !clutter.blocked_at(100, 100, 0),
            "a crate on an upper floor sealed the floor beneath it"
        );
        assert!(
            clutter.blocked_at(100, 100, 40),
            "a crate did not block the floor it stands on"
        );
    }

    #[test]
    fn a_gold_coin_is_not_an_obstacle() {
        let Some(tiles) = client_tiledata() else {
            return;
        };
        // Gold on the floor is the case the impassable flag exists to tell
        // apart: a tile full of loot is still walked over.
        let gold = Graphic(0x0EED);
        assert!(
            !tiles.static_tile(gold.0).flags.is_blocking(),
            "gold's tiledata calls it impassable"
        );
        let clutter = Clutter::of(&[item(100, 100, 0, gold)], &tiles);
        assert!(
            !clutter.blocked_at(100, 100, 0),
            "a coin on the floor stopped a step"
        );
    }
}
