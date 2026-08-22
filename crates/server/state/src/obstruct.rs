//! Dynamic obstacles: entities that block a tile the map calls open.
//!
//! # Why the map is not enough
//!
//! `MapTerrain` reads the client's files — land and static art — and nothing
//! else. A door, though, is an *entity*: the doorway it stands in is an open
//! gap in the statics by construction (that is how it was chosen), and the
//! door itself lives in the registry, invisible to every terrain check. Without
//! this index a closed door stops nobody — player or NPC — and the bug reads as
//! "NPCs walk through doors" only because a player politely double-clicks
//! before walking.
//!
//! So placing a blocking entity registers it here, and movement asks both: the
//! map for the ground, this index for what the world has put on top. The index
//! is a second copy of a fact the registry already holds (a closed `Door` at a
//! tile), and the code that flips the door is what keeps the copy honest —
//! the same bargain the sector grid makes with `Position`.
//!
//! An obstacle carries a z-span, so a wall on an upper floor blocks that floor
//! and not the ground beneath it — and one entity may hold several, because a
//! house is a single entity whose walls stand on top of each other. See
//! [`Obstructions::block`], where the identity is the entity *and* the z.

use std::collections::HashMap;

use openshard_entities::EntityId;
use openshard_movement::{LandTile, OpenWorld, Terrain, Tile};
use openshard_protocol::wire::Graphic;
use openshard_protocol::world::Point;

/// A mobile's body height in z-units, for deciding what overlaps it. Matches the
/// step check's `PLAYER_HEIGHT` in `world::terrain`.
const MOBILE_HEIGHT: i32 = 16;

/// The height a door (or a plain wall-style obstacle) blocks through when the
/// placer has no tiledata height to hand. A classic UO wall/door is 20 tall.
pub const DOOR_HEIGHT: u8 = 20;

/// One entity blocking a tile.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Obstacle {
    /// The blocking entity.
    pub entity: EntityId,
    /// A closed door: a mobile that knows how may open it rather than walk
    /// around, so movement wants to know *what* blocked, not just that.
    pub door: bool,
    /// The base z the blocker sits at.
    pub z: i8,
    /// How tall it is: its body spans `[z, z + height)`. This is the z-span that
    /// lets a wall on an upper floor block that floor and *not* the ground
    /// beneath it — without it, a placed multi-storey building sealed every floor
    /// below its highest impassable piece.
    pub height: u8,
}

/// The dynamic obstacles on one facet: tile → the entities blocking it.
#[derive(Default, Debug)]
pub struct Obstructions {
    tiles: HashMap<(u16, u16), Vec<Obstacle>>,
}

impl Obstructions {
    /// Mark `entity` as blocking `(x, y)` through the z-span `[z, z + height)`.
    /// Blocking twice at the same height is idempotent.
    ///
    /// # One entity may block one tile at several heights
    ///
    /// The identity is the entity **and the z**, not the entity alone. A door is
    /// one thing at one height and re-registering it refines it, which is what
    /// the entity half is for. A *house* is one entity whose components stand on
    /// top of each other: a wall on the ground floor and a wall directly above it
    /// on the second are two obstacles at one tile, and keying by the entity alone
    /// would let the second overwrite the first — sealing the upper floor and
    /// opening the lower one, in whichever order they happened to be registered.
    pub fn block(&mut self, x: u16, y: u16, entity: EntityId, door: bool, z: i8, height: u8) {
        let tile = self.tiles.entry((x, y)).or_default();
        if let Some(existing) = tile.iter_mut().find(|o| o.entity == entity && o.z == z) {
            // Re-registering refines what the blocker is — a doorway placed as
            // plain impassable art and then given its `Door` stays one obstacle.
            existing.door = door;
            existing.height = height;
        } else {
            tile.push(Obstacle {
                entity,
                door,
                z,
                height,
            });
        }
    }

    /// Remove `entity`'s block on `(x, y)`, if it holds one.
    pub fn unblock(&mut self, x: u16, y: u16, entity: EntityId) {
        if let Some(tile) = self.tiles.get_mut(&(x, y)) {
            tile.retain(|o| o.entity != entity);
            if tile.is_empty() {
                self.tiles.remove(&(x, y));
            }
        }
    }

    /// The first thing blocking `(x, y)` at any height, if anything is. Used for
    /// door detection and sight, where a door is a full-height wall and its z
    /// does not matter.
    #[must_use]
    pub fn blocker_at(&self, x: u16, y: u16) -> Option<Obstacle> {
        self.tiles.get(&(x, y)).and_then(|t| t.first().copied())
    }

    /// The first thing blocking `(x, y)` in the vertical span a mobile standing
    /// at `stand_z` occupies — its body `[z, z + height)` meeting the mobile's
    /// `[stand_z, stand_z + MOBILE_HEIGHT)`. This is what movement asks, so an
    /// upper-floor blocker leaves the ground floor open.
    #[must_use]
    pub fn blocker_at_z(&self, x: u16, y: u16, stand_z: i32) -> Option<Obstacle> {
        self.tiles.get(&(x, y)).and_then(|tile| {
            tile.iter()
                .find(|o| {
                    let bottom = i32::from(o.z);
                    let top = bottom + i32::from(o.height).max(1);
                    bottom < stand_z + MOBILE_HEIGHT && stand_z < top
                })
                .copied()
        })
    }

    /// Whether anything blocks `(x, y)`.
    #[must_use]
    pub fn is_blocked(&self, x: u16, y: u16) -> bool {
        self.tiles.contains_key(&(x, y))
    }
}

/// The map's terrain with the live world's obstacles laid over it.
///
/// What every movement decision — a player's walk, an NPC's step, a chase's
/// A* — actually checks. Built fresh from a [`FacetState`](crate::FacetState)
/// each time; a borrow, not a copy.
#[derive(Clone, Copy)]
pub struct LiveTerrain<'a> {
    map: Option<&'a (dyn Terrain + Send + Sync)>,
    obstructions: &'a Obstructions,
    /// The ships. A third source beside the map and the obstruction index, and
    /// the only one that can *add* somewhere to stand — see [`crate::boat`].
    boats: &'a crate::boat::Boats,
    /// Plan as a door-opener: a closed door does not block, because the mobile
    /// walking this route will open it on arrival. Pathfinding for a creature
    /// that opens doors sets this; the actual step never does.
    through_doors: bool,
}

impl std::fmt::Debug for LiveTerrain<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveTerrain")
            .field("has_map", &self.map.is_some())
            .field("through_doors", &self.through_doors)
            .finish()
    }
}

impl<'a> LiveTerrain<'a> {
    pub(crate) fn new(
        map: Option<&'a (dyn Terrain + Send + Sync)>,
        obstructions: &'a Obstructions,
        boats: &'a crate::boat::Boats,
        through_doors: bool,
    ) -> Self {
        Self {
            map,
            obstructions,
            boats,
            through_doors,
        }
    }

    /// Where a body coming from `from` would land on a deck at `to`, if a boat
    /// covers it.
    ///
    /// **The one thing that can overrule the map's refusal.** Open water is not
    /// ground, so `can_step` onto it answers `None` — correctly, because there
    /// is nothing there. A ship makes there be something there, and no index
    /// that only subtracts could say so.
    fn aboard(&self, from: Point, to: Point) -> Option<Point> {
        if self.boats.is_empty() {
            return None;
        }
        let deck = self.boats.deck_at(to.x, to.y, i32::from(from.z))?;
        Some(Point::new(to.x, to.y, i8::try_from(deck).ok()?))
    }

    /// What blocks `(x, y)`, if anything — so a caller can tell a door from a
    /// crate before deciding to open, path around, or give up.
    #[must_use]
    pub fn blocker_at(&self, x: u16, y: u16) -> Option<Obstacle> {
        self.obstructions.blocker_at(x, y)
    }
}

impl Terrain for LiveTerrain<'_> {
    fn can_step(&self, from: Point, to: Point) -> Option<Point> {
        let landed = match self.map {
            Some(map) => match map.can_step(from, to) {
                Some(landed) => landed,
                // The map says there is nothing to stand on, which over open
                // water is true right up until a ship is moored there.
                None => self.aboard(from, to)?,
            },
            None => OpenWorld.can_step(from, to)?,
        };
        // A hull is a wall that is not in the obstruction index, so it is asked
        // separately — and asked after the landing is known, because a gunwale
        // seals the deck's height and not the water under the ship.
        if !self.boats.is_empty() && self.boats.hull_blocks(to.x, to.y, i32::from(landed.z)) {
            return None;
        }
        // A live obstacle in the mobile's own vertical span on the destination
        // tile: a shut door yields only to a planner told it will be opened,
        // anything else stops the step. Checked at the z the mobile will stand at,
        // so an upper-floor wall does not block the floor below.
        match self.obstructions.blocker_at_z(to.x, to.y, i32::from(landed.z)) {
            Some(o) if o.door && self.through_doors => {}
            Some(_) => return None,
            None => {}
        }
        // The diagonal corner rule: a diagonal step may not slip through the
        // corner where two blockers meet — both cardinal tiles flanking it must
        // themselves be steppable. The A* planner already refuses such corners
        // (`movement::path::corner_open`); enforcing it here in the shared
        // validator means a *server-driven* creature taking a naive, wandering
        // or kiting step cannot squeeze between two walls the way only a planned
        // route was ever stopped from doing. The flank calls are orthogonal, so
        // they do not re-enter this branch.
        //
        // Restated here rather than delegated to `movement::step_allowed`, which
        // is the same rule for everyone deciding a step *before* asking a
        // terrain: that one calls `can_step`, so this is the call it would
        // re-enter. Every other caller — `find_path`, and the client's own
        // held-direction detour — asks `step_allowed` instead. A client that
        // asks a bare `Terrain` is the one that gets this wrong, and ours did:
        // `MapTerrain` has no corner rule, so the client thought a corner-cutting
        // diagonal was walkable, sent it, and was refused here every hold — a
        // body stuck on a building corner. See docs/client.md.
        let dx = i32::from(to.x) - i32::from(from.x);
        let dy = i32::from(to.y) - i32::from(from.y);
        if dx != 0 && dy != 0 {
            let flank_x = Point::new((i32::from(from.x) + dx.signum()) as u16, from.y, from.z);
            let flank_y = Point::new(from.x, (i32::from(from.y) + dy.signum()) as u16, from.z);
            if self.can_step(from, flank_x).is_none() || self.can_step(from, flank_y).is_none() {
                return None;
            }
        }
        Some(landed)
    }

    fn ground_z(&self, tile: Tile) -> Option<i8> {
        self.map.and_then(|m| m.ground_z(tile))
    }

    fn land_tile(&self, tile: Tile) -> Option<LandTile> {
        self.map.and_then(|m| m.land_tile(tile))
    }

    fn statics_at(&self, tile: Tile, out: &mut Vec<(Graphic, i8)>) {
        if let Some(map) = self.map {
            map.statics_at(tile, out);
        }
    }

    fn stand_z(&self, tile: Tile, near_z: i32) -> Option<i32> {
        self.map.and_then(|m| m.stand_z(tile, near_z))
    }

    fn can_fit(&self, tile: Tile, z: i32, height: i32) -> bool {
        if !self.boats.is_empty() {
            if self.boats.hull_blocks(tile.x, tile.y, z) {
                return false;
            }
            // A deck at exactly this height is a surface the map does not have,
            // so it answers for the map rather than alongside it.
            if self.boats.deck_at(tile.x, tile.y, z) == Some(z) {
                return self.obstructions.blocker_at_z(tile.x, tile.y, z).is_none();
            }
        }
        self.map.is_none_or(|m| m.can_fit(tile, z, height))
            && self.obstructions.blocker_at_z(tile.x, tile.y, z).is_none()
    }

    fn spawn_z(&self, tile: Tile, near_z: i32) -> Option<i32> {
        self.map
            .map_or_else(|| self.stand_z(tile, near_z), |m| m.spawn_z(tile, near_z))
    }

    // A *coordinate* question, so it stays: the map is what says which land tile
    // is at `tile`. Its six neighbours — what a graphic weighs, how tall it is,
    // which hand it is held in — took no coordinate and could not be changed by
    // anything this overlay holds, so they are asked of `WorldState::tiles`
    // directly now. Every one of them used to fall through to the trait's
    // default here, which is the defect a decorator that forwards by hand always
    // has in it; there is nothing left to forget.
    fn land_is_water(&self, tile: Tile) -> bool {
        self.map.is_some_and(|m| m.land_is_water(tile))
    }

    fn sight_clear(&self, from: Point, to: Point) -> bool {
        if !self.map.is_none_or(|m| m.sight_clear(from, to)) {
            return false;
        }
        // A shut door is opaque; a crate is furniture, not a wall.
        openshard_movement::line_tiles(Tile::new(from.x, from.y), Tile::new(to.x, to.y))
            .into_iter()
            .all(|tile| {
                self.obstructions
                    .blocker_at(tile.x, tile.y)
                    .is_none_or(|o| !o.door)
            })
    }
}

#[cfg(test)]
mod tests {
    /// A house is one entity with walls above walls, and both must block.
    ///
    /// The key is the entity *and* the z. Keyed by the entity alone, the second
    /// registration overwrote the first — which does not read as "one wall is
    /// missing" but as the wrong floor being sealed, since which of the two
    /// survived depended on the order the components happened to come out of the
    /// file in.
    #[test]
    fn one_entity_blocks_one_tile_at_two_heights() {
        let house = an_entity();
        let mut obstructions = Obstructions::default();
        obstructions.block(10, 10, house, false, 0, 20);
        obstructions.block(10, 10, house, false, 20, 20);

        assert!(
            obstructions.blocker_at_z(10, 10, 0).is_some(),
            "the ground floor wall is gone"
        );
        assert!(
            obstructions.blocker_at_z(10, 10, 25).is_some(),
            "the upper floor wall is gone"
        );
        // And the storey above both is still open sky.
        assert!(obstructions.blocker_at_z(10, 10, 60).is_none());

        // Re-registering at a height already held still refines rather than adds:
        // that is what the entity half of the key is for.
        obstructions.block(10, 10, house, true, 0, 20);
        assert!(
            obstructions.blocker_at_z(10, 10, 0).is_some_and(|o| o.door),
            "the refinement did not land"
        );
    }

    use super::*;
    use crate::boat::{Boats, Plank};
    use openshard_entities::Registry;

    /// A harbour with no ships in it. Most of these tests predate boats and want
    /// exactly that; the ones that do not build their own.
    static NO_BOATS: std::sync::LazyLock<Boats> = std::sync::LazyLock::new(Boats::default);

    fn an_entity() -> EntityId {
        Registry::new().spawn()
    }

    #[test]
    fn a_blocked_tile_refuses_a_step_the_open_world_allows() {
        let mut obstructions = Obstructions::default();
        let door = an_entity();
        obstructions.block(10, 10, door, true, 0, DOOR_HEIGHT);
        let live = LiveTerrain::new(None, &obstructions, &NO_BOATS, false);
        assert!(
            live.can_step(Point::new(10, 9, 0), Point::new(10, 10, 0))
                .is_none()
        );
        assert!(
            live.can_step(Point::new(10, 9, 0), Point::new(11, 9, 0))
                .is_some()
        );
    }

    #[test]
    fn a_door_opener_plans_through_a_door_but_not_through_a_crate() {
        let mut obstructions = Obstructions::default();
        obstructions.block(10, 10, an_entity(), true, 0, DOOR_HEIGHT);
        obstructions.block(12, 10, an_entity(), false, 0, DOOR_HEIGHT);
        let planner = LiveTerrain::new(None, &obstructions, &NO_BOATS, true);
        assert!(
            planner
                .can_step(Point::new(10, 9, 0), Point::new(10, 10, 0))
                .is_some()
        );
        assert!(
            planner
                .can_step(Point::new(12, 9, 0), Point::new(12, 10, 0))
                .is_none()
        );
    }

    #[test]
    fn a_shut_door_is_opaque_and_an_open_one_is_not() {
        let mut obstructions = Obstructions::default();
        let door = an_entity();
        obstructions.block(10, 10, door, true, 0, DOOR_HEIGHT);
        let live = LiveTerrain::new(None, &obstructions, &NO_BOATS, false);
        assert!(!live.sight_clear(Point::new(10, 8, 0), Point::new(10, 12, 0)));
        obstructions.unblock(10, 10, door);
        let live = LiveTerrain::new(None, &obstructions, &NO_BOATS, false);
        assert!(live.sight_clear(Point::new(10, 8, 0), Point::new(10, 12, 0)));
    }

    #[test]
    fn a_diagonal_passes_an_open_corner() {
        let obstructions = Obstructions::default();
        let live = LiveTerrain::new(None, &obstructions, &NO_BOATS, false);
        assert!(
            live.can_step(Point::new(10, 10, 0), Point::new(11, 11, 0))
                .is_some(),
            "nothing flanks the diagonal, so it is not cutting a corner"
        );
    }

    #[test]
    fn a_diagonal_is_refused_when_either_flank_is_blocked() {
        // One crate east of the mover is enough: the diagonal into (11,11) would
        // slip past its corner, which the rule forbids even with the other flank
        // wide open. This is the case a server-driven creature used to exploit.
        let mut obstructions = Obstructions::default();
        obstructions.block(11, 10, an_entity(), false, 0, DOOR_HEIGHT);
        let live = LiveTerrain::new(None, &obstructions, &NO_BOATS, false);
        assert!(
            live.can_step(Point::new(10, 10, 0), Point::new(11, 11, 0))
                .is_none(),
            "a single blocked flank forbids the corner cut"
        );
        // The orthogonal step onto the open tile beside it is still fine.
        assert!(
            live.can_step(Point::new(10, 10, 0), Point::new(10, 11, 0))
                .is_some(),
            "the cardinal step is unaffected"
        );
    }

    #[test]
    fn unblocking_frees_the_tile_and_blocking_twice_is_one_obstacle() {
        let mut obstructions = Obstructions::default();
        let door = an_entity();
        obstructions.block(5, 5, door, true, 0, DOOR_HEIGHT);
        obstructions.block(5, 5, door, true, 0, DOOR_HEIGHT);
        obstructions.unblock(5, 5, door);
        assert!(!obstructions.is_blocked(5, 5));
    }

    #[test]
    fn an_upper_floor_blocker_leaves_the_ground_floor_open() {
        // The Britain-library bug: a placed impassable static on an upper floor
        // (z 20, a wall 20 tall) must not seal the ground beneath it, but one at
        // ground level must still block. The mobile steps at z 0.
        let mut obstructions = Obstructions::default();
        obstructions.block(10, 10, an_entity(), false, 20, 20);
        let live = LiveTerrain::new(None, &obstructions, &NO_BOATS, false);
        assert!(
            live.can_step(Point::new(10, 9, 0), Point::new(10, 10, 0))
                .is_some(),
            "an upper-floor wall does not block the floor below"
        );

        obstructions.block(11, 10, an_entity(), false, 0, 20);
        let live = LiveTerrain::new(None, &obstructions, &NO_BOATS, false);
        assert!(
            live.can_step(Point::new(11, 9, 0), Point::new(11, 10, 0))
                .is_none(),
            "but a ground-level wall still blocks"
        );
    }

    /// A terrain that answers a map question with something distinguishable from
    /// the trait's default, so a forward that is missing reads as the default
    /// rather than as a plausible answer.
    struct Charted;

    impl Terrain for Charted {
        fn can_step(&self, _from: Point, to: Point) -> Option<Point> {
            Some(to)
        }
        fn land_is_water(&self, tile: Tile) -> bool {
            tile.x >= 100
        }
    }

    /// **The forward, because a missing one is silent.**
    ///
    /// This used to cover seven questions, six of which have since left the
    /// trait for `WorldState::tiles` — they took a graphic and no coordinate, so
    /// no overlay could change their answer and nothing had to forward them. The
    /// one left is a map question and still can be forgotten: it used to reach
    /// the trait's default through `LiveTerrain`, which says a shard has no sea
    /// and so nowhere to moor a boat. That is the right answer for a shard with
    /// no map and the wrong one for a shard that has one and wrapped it, which is
    /// every running shard.
    #[test]
    fn the_live_terrain_answers_the_map_and_not_the_trait_default() {
        let obstructions = Obstructions::default();
        let charted = Charted;
        let live = LiveTerrain::new(Some(&charted), &obstructions, &NO_BOATS, false);

        assert!(live.land_is_water(Tile::new(100, 5)), "the sea");
        assert!(!live.land_is_water(Tile::new(99, 5)), "and the shore");
    }

    /// And with no map the default is what is wanted: a shard running without
    /// client files has no sea, so it has nowhere to moor a boat.
    #[test]
    fn a_live_terrain_with_no_map_reports_no_water() {
        let obstructions = Obstructions::default();
        let live = LiveTerrain::new(None, &obstructions, &NO_BOATS, false);
        assert!(!live.land_is_water(Tile::new(100, 5)));
    }

    /// A sea with a rock in it: nothing is walkable except one strip of shore.
    struct Sea;

    impl Terrain for Sea {
        fn can_step(&self, _from: Point, to: Point) -> Option<Point> {
            // The shore runs along y = 0. Everything else is open water, which
            // the map correctly says is nowhere to stand.
            (to.y == 0).then_some(to)
        }
        fn land_is_water(&self, tile: Tile) -> bool {
            tile.y != 0
        }
        fn can_fit(&self, tile: Tile, _z: i32, _height: i32) -> bool {
            tile.y == 0
        }
    }

    fn a_ship_at(boat: EntityId, x: u16, y: u16) -> Boats {
        let mut boats = Boats::default();
        boats.moor(
            boat,
            [
                // Two deck tiles at z 2, and a hull tile beside them.
                (
                    (x, y),
                    Plank {
                        boat,
                        z: 2,
                        height: 3,
                        blocks: false,
                    },
                ),
                (
                    (x, y + 1),
                    Plank {
                        boat,
                        z: 2,
                        height: 3,
                        blocks: false,
                    },
                ),
                (
                    (x + 1, y),
                    Plank {
                        boat,
                        z: 2,
                        height: 10,
                        blocks: true,
                    },
                ),
            ],
        );
        boats
    }

    /// **The positive half, and the reason boats are not in `Obstructions`.**
    ///
    /// The map refuses a step onto open water because there is nothing there,
    /// which is true until a ship is moored on it. No index that can only
    /// subtract could overturn that refusal.
    #[test]
    fn a_deck_makes_a_step_onto_open_water_legal() {
        let obstructions = Obstructions::default();
        let boats = a_ship_at(an_entity(), 10, 1);
        let live = LiveTerrain::new(Some(&Sea), &obstructions, &boats, false);

        assert!(
            live.can_step(Point::new(20, 0, 0), Point::new(20, 1, 0))
                .is_none(),
            "open water with no ship on it is still not walkable",
        );
        assert_eq!(
            live.can_step(Point::new(10, 0, 0), Point::new(10, 1, 0)),
            Some(Point::new(10, 1, 5)),
            "stepping aboard from the shore lands on the deck, not in the water",
        );
        assert_eq!(
            live.can_step(Point::new(10, 1, 5), Point::new(10, 2, 5)),
            Some(Point::new(10, 2, 5)),
            "and walking along the deck stays on it",
        );
    }

    /// The hull is a wall that is not in the obstruction index, so it is asked
    /// separately — and asked at the landing height, because a gunwale seals the
    /// deck and not the sea beneath the ship.
    #[test]
    fn a_hull_refuses_the_step_a_deck_would_have_allowed() {
        let obstructions = Obstructions::default();
        let boats = a_ship_at(an_entity(), 10, 1);
        let live = LiveTerrain::new(Some(&Sea), &obstructions, &boats, false);

        assert!(
            live.can_step(Point::new(10, 1, 5), Point::new(11, 1, 5))
                .is_none(),
            "walked straight through the hull",
        );
    }

    /// With no ships anywhere the answers are exactly what they were before
    /// boats existed. The regression that matters most, because every other
    /// facet on every shard is this one.
    #[test]
    fn an_empty_harbour_changes_no_answer() {
        let obstructions = Obstructions::default();
        let live = LiveTerrain::new(Some(&Sea), &obstructions, &NO_BOATS, false);

        assert!(
            live.can_step(Point::new(10, 0, 0), Point::new(10, 1, 0))
                .is_none()
        );
        assert_eq!(
            live.can_step(Point::new(10, 0, 0), Point::new(11, 0, 0)),
            Some(Point::new(11, 0, 0)),
        );
        assert!(!live.can_fit(Tile::new(10, 1), 5, 16));
    }

    /// **B3's owed measurement, not an assurance.**
    ///
    /// The boat consultation runs on every step by every mobile, and the
    /// diagonal rule re-enters it twice more. `docs/boats.md` asked the phase
    /// that landed it to measure rather than promise, so this walks the same
    /// hundred thousand steps over an empty harbour and over one with a ship in
    /// it and prints both.
    ///
    /// Deliberately **not** asserted. A wall-clock threshold in a test suite is
    /// a flake generator on shared CI; what is worth having is the number, on
    /// demand:
    ///
    /// ```sh
    /// cargo test --release -p openshard-state boat_step_cost -- --nocapture --ignored
    /// ```
    ///
    /// # What it measured, 2026-08-16
    ///
    /// Release, 100,000 steps: **1.5ms with no boats, 5.5ms with one moored** —
    /// 15ns against 55ns a step.
    ///
    /// The empty case is the one that matters, because it is every facet on
    /// every shard that has no ships, and it is the `is_empty` length check
    /// doing its job.
    ///
    /// **The 3.6x is the least flattering framing available and is stated that
    /// way on purpose.** `Sea::can_step` below is a single integer comparison,
    /// so the boat lookup is very nearly the whole of the measured work. A real
    /// `MapTerrain::can_step` reads map blocks, walks the statics on the tile
    /// and computes surfaces; against that baseline the same absolute 40ns is a
    /// small fraction rather than a multiple. What the number establishes is the
    /// **absolute** cost of a hash probe per step, which is the thing that was
    /// worth knowing.
    ///
    /// If it ever needs to be smaller, the obvious move is a bounding box per
    /// facet — one integer range test to skip the probe for tiles nowhere near
    /// any ship. Not done, because 40ns did not justify a second structure to
    /// keep in step.
    #[test]
    #[ignore = "a measurement, not an assertion — see the doc comment"]
    fn boat_step_cost() {
        const STEPS: u32 = 100_000;
        let obstructions = Obstructions::default();
        let busy = a_ship_at(an_entity(), 500, 1);

        let walk = |boats: &Boats| {
            let live = LiveTerrain::new(Some(&Sea), &obstructions, boats, false);
            let start = std::time::Instant::now();
            let mut allowed = 0u32;
            for step in 0..STEPS {
                let x = (step % 1000) as u16;
                if live
                    .can_step(Point::new(x, 0, 0), Point::new(x.wrapping_add(1), 0, 0))
                    .is_some()
                {
                    allowed += 1;
                }
            }
            (start.elapsed(), allowed)
        };

        let (empty, _) = walk(&NO_BOATS);
        let (with_ship, _) = walk(&busy);
        println!("{STEPS} steps, empty harbour: {empty:?}");
        println!("{STEPS} steps, one ship moored: {with_ship:?}");
    }
}
