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

use crate::boat::Boats;
use openshard_entities::EntityId;
use openshard_map::grid::Tile;
use openshard_map::overlay::{Body, Cover, Overlay};

/// A mobile's body height in z-units, for deciding what overlaps it. Matches the
/// step check's `PLAYER_HEIGHT` in `world::terrain`.
const MOBILE_HEIGHT: i32 = 16;

/// The height a door (or a plain wall-style obstacle) blocks through when the
/// placer has no tiledata height to hand. A classic UO wall/door is 20 tall.
pub const DOOR_HEIGHT: u8 = 20;

/// One entity's one entry over a tile: what it does there, and who put it there.
///
/// **The cover is the whole of what it does.** This used to spell the span and
/// the door flag out again — a `z`, a `height` and a `door: bool` beside the
/// entity — and then convert them to a [`Cover`] on the way out, which is one
/// idea written twice and a conversion that could drift from it. Now the
/// identity is the only thing this adds: the overlay says a door is in the way,
/// and this says *which* door.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Obstacle {
    /// The entity that put this here.
    pub entity: EntityId,
    /// What it does to a body standing on the tile — the span, and which of the
    /// two things it is. The z-span is what lets a wall on an upper floor block
    /// that floor and *not* the ground beneath it: without it, a placed
    /// multi-storey building sealed every floor below its highest impassable
    /// piece.
    pub cover: Cover,
}

impl Obstacle {
    /// Whether this is a shut door somebody could open.
    ///
    /// A mobile that knows how may open one rather than walk around, so movement
    /// wants to know *what* blocked and not just that something did.
    #[must_use]
    pub const fn door(&self) -> bool {
        self.cover.is_door()
    }
}

/// The dynamic obstacles on one facet: tile → the entities blocking it.
#[derive(Default, Debug)]
pub struct Obstructions {
    tiles: HashMap<(u16, u16), Vec<Obstacle>>,
}

impl Obstructions {
    /// Put `entity`'s `cover` on `(x, y)`. Registering the same one twice is
    /// idempotent.
    ///
    /// # One entity may cover one tile several times over
    ///
    /// The identity is the entity, **the z, and which arm of the cover it is** —
    /// not the entity alone.
    ///
    /// - A door is one thing at one height and re-registering it refines it,
    ///   which is what the entity half is for.
    /// - A *house* is one entity whose components stand on top of each other: a
    ///   wall on the ground floor and a wall directly above it on the second are
    ///   two entries at one tile, and keying by the entity alone would let the
    ///   second overwrite the first — sealing the upper floor and opening the
    ///   lower one, in whichever order they happened to be registered.
    /// - A *platform* is two entries at one z from one entity: a stair tread is
    ///   something a body beside it walks into and somewhere a body on top of it
    ///   stands, and the two are not refinements of each other. That is the
    ///   third part of the key.
    pub fn block(&mut self, x: u16, y: u16, entity: EntityId, cover: Cover) {
        let tile = self.tiles.entry((x, y)).or_default();
        let same = |o: &&mut Obstacle| {
            o.entity == entity && o.cover.z == cover.z && o.cover.is_surface() == cover.is_surface()
        };
        if let Some(existing) = tile.iter_mut().find(same) {
            // Re-registering refines what the cover is — a doorway placed as
            // plain impassable art and then given its `Door` stays one entry.
            existing.cover = cover;
        } else {
            tile.push(Obstacle { entity, cover });
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
    ///
    /// A *surface* is not something blocking: a house's floor is registered here
    /// too, and a body looking through the tile it lies on is not looking
    /// through a wall.
    #[must_use]
    pub fn blocker_at(&self, x: u16, y: u16) -> Option<Obstacle> {
        self.tiles
            .get(&(x, y))
            .and_then(|t| t.iter().find(|o| o.cover.is_blocker()).copied())
    }

    /// The first thing blocking `(x, y)` in the vertical span a mobile standing
    /// at `stand_z` occupies — the cover's body meeting the mobile's
    /// `[stand_z, stand_z + MOBILE_HEIGHT)`. This is what movement asks, so an
    /// upper-floor blocker leaves the ground floor open.
    #[must_use]
    pub fn blocker_at_z(&self, x: u16, y: u16, stand_z: i32) -> Option<Obstacle> {
        let body = Body::new(stand_z, MOBILE_HEIGHT);
        self.tiles.get(&(x, y)).and_then(|tile| {
            tile.iter()
                .find(|o| o.cover.is_blocker() && o.cover.meets(body))
                .copied()
        })
    }

    /// Whether anything at all is registered on `(x, y)` — **a surface
    /// included**, so this is not the question a step asks.
    ///
    /// It was called `is_blocked` while nothing but blockers could be in here.
    /// A house's floor is a surface and goes in the same index, so the old name
    /// would now answer "yes, blocked" about an open room. Use
    /// [`blocker_at`](Self::blocker_at) for what is in the way.
    #[must_use]
    pub fn holds_anything(&self, x: u16, y: u16) -> bool {
        self.tiles.contains_key(&(x, y))
    }

    /// Every tile this index holds anything on.
    pub fn tiles(&self) -> impl Iterator<Item = (u16, u16)> + '_ {
        self.tiles.keys().copied()
    }

    /// Everything registered on `(x, y)`.
    ///
    /// What [`FacetState`](crate::FacetState) projects into the overlay after
    /// every mutation of this index — see its `refresh`. Read outwards rather
    /// than kept private because this is the *identity* half: the overlay says
    /// a door is in the way, and only this says which door, which is what a
    /// townsperson about to open one needs.
    #[must_use]
    pub fn at(&self, x: u16, y: u16) -> &[Obstacle] {
        self.tiles.get(&(x, y)).map_or(&[], Vec::as_slice)
    }
}

/// Everything the two live indexes put on one tile, as the overlay states it.
///
/// **The projection, in one place.** Both sources at once rather than each
/// index maintaining its own slice of the overlay: a crate lashed to a deck is
/// one tile with entries from both, and a per-source rule for merging them is a
/// rule that can be wrong. This has nothing to merge.
#[must_use]
pub fn covers_at(obstructions: &Obstructions, boats: &Boats, x: u16, y: u16) -> Vec<Cover> {
    obstructions
        .at(x, y)
        .iter()
        .map(|obstacle| obstacle.cover)
        // `flat_map` and not `map`: a piece of a ship lays what its art lays,
        // which for a deck plank is two covers — the floor and the planking
        // under it — and for a rope is none at all.
        .chain(boats.at(x, y).iter().flat_map(|plank| plank.covers()))
        .collect()
}

/// Both live indexes projected into one overlay, whole.
///
/// What a caller holding the indexes and no facet builds. A facet does not go
/// through here — it keeps its overlay in step one tile at a time, which is the
/// point of [`FacetState::block`](crate::FacetState::block) and its three
/// siblings.
#[must_use]
pub fn project(obstructions: &Obstructions, boats: &Boats) -> Overlay {
    let mut overlay = Overlay::default();
    for (x, y) in obstructions.tiles().chain(boats.tiles()) {
        overlay.set(Tile::new(x, y), covers_at(obstructions, boats, x, y));
    }
    overlay
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
        obstructions.block(10, 10, house, Cover::blocking(0, 20));
        obstructions.block(10, 10, house, Cover::blocking(20, 20));

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
        obstructions.block(10, 10, house, Cover::door(0, 20));
        assert!(
            obstructions.blocker_at_z(10, 10, 0).is_some_and(|o| o.door()),
            "the refinement did not land"
        );
    }

    use super::*;
    use crate::boat::{Boats, Plank};
    use openshard_entities::Registry;
    use openshard_map::overlay::Doors;
    use openshard_movement::Footing;
    use openshard_movement::scene::Scene;
    use openshard_protocol::direction::Direction;
    use openshard_protocol::world::Point;
    use openshard_tiles::TileFlags;

    /// The land id a scene paves with, which these fixtures declare to be water.
    ///
    /// `Scene::flat_holding` lays id `0` over every cell, so making *that* the
    /// sea costs no pass at all and only the shore has to be written.
    const OPEN_WATER: u16 = 0;

    /// The land id of the strip a body can actually stand on.
    const SHORE: u16 = 0x0003;

    /// A deck plank: a platform two tall, so a body stands on it at z 2.
    const DECK: u16 = 0x3E4A;
    /// A hull plank: impassable and ten tall.
    const HULL: u16 = 0x3E4E;

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
        obstructions.block(10, 10, door, Cover::door(0, DOOR_HEIGHT));
        let live_overlay = project(&obstructions, &NO_BOATS);
        let live = Footing::new(None, &live_overlay, Doors::AsTheyStand);
        assert!(openshard_movement::can_step(&live, Point::new(10, 9, 0), Point::new(10, 10, 0)).is_none());
        assert!(openshard_movement::can_step(&live, Point::new(10, 9, 0), Point::new(11, 9, 0)).is_some());
    }

    #[test]
    fn a_door_opener_plans_through_a_door_but_not_through_a_crate() {
        let mut obstructions = Obstructions::default();
        obstructions.block(10, 10, an_entity(), Cover::door(0, DOOR_HEIGHT));
        obstructions.block(12, 10, an_entity(), Cover::blocking(0, DOOR_HEIGHT));
        let planner_overlay = project(&obstructions, &NO_BOATS);
        let planner = Footing::new(None, &planner_overlay, Doors::AllOpen);
        assert!(
            openshard_movement::can_step(&planner, Point::new(10, 9, 0), Point::new(10, 10, 0)).is_some()
        );
        assert!(
            openshard_movement::can_step(&planner, Point::new(12, 9, 0), Point::new(12, 10, 0)).is_none()
        );
    }

    #[test]
    fn a_shut_door_is_opaque_and_an_open_one_is_not() {
        let mut obstructions = Obstructions::default();
        let door = an_entity();
        obstructions.block(10, 10, door, Cover::door(0, DOOR_HEIGHT));
        let live_overlay = project(&obstructions, &NO_BOATS);
        let live = Footing::new(None, &live_overlay, Doors::AsTheyStand);
        assert!(!openshard_movement::sight_clear(
            &live,
            Point::new(10, 8, 0),
            Point::new(10, 12, 0)
        ));
        obstructions.unblock(10, 10, door);
        let live_overlay = project(&obstructions, &NO_BOATS);
        let live = Footing::new(None, &live_overlay, Doors::AsTheyStand);
        assert!(openshard_movement::sight_clear(
            &live,
            Point::new(10, 8, 0),
            Point::new(10, 12, 0)
        ));
    }

    /// The corner rule is [`step_allowed`](openshard_movement::step_allowed)'s
    /// and not [`can_step`](openshard_movement::can_step)'s.
    ///
    /// `can_step` is one landing: it answers whether a body may *stand* where
    /// the step ends, and a diagonal's two flanks are not that question. The
    /// rule lives one layer up, in `steps_out_of`, which resolves all eight
    /// neighbours together precisely so a diagonal can read the flanks it needs
    /// — see `docs/map/navigation_spans.md`'s N3. These two used to ask
    /// `can_step` and had been failing since the rule moved.
    #[test]
    fn a_diagonal_passes_an_open_corner() {
        let obstructions = Obstructions::default();
        let live_overlay = project(&obstructions, &NO_BOATS);
        let live = Footing::new(None, &live_overlay, Doors::AsTheyStand);
        assert!(
            openshard_movement::step_allowed(&live, Point::new(10, 10, 0), Direction::SouthEast).is_some(),
            "nothing flanks the diagonal, so it is not cutting a corner"
        );
    }

    #[test]
    fn a_diagonal_is_refused_when_either_flank_is_blocked() {
        // One crate east of the mover is enough: the diagonal into (11,11) would
        // slip past its corner, which the rule forbids even with the other flank
        // wide open. This is the case a server-driven creature used to exploit.
        let mut obstructions = Obstructions::default();
        obstructions.block(11, 10, an_entity(), Cover::blocking(0, DOOR_HEIGHT));
        let live_overlay = project(&obstructions, &NO_BOATS);
        let live = Footing::new(None, &live_overlay, Doors::AsTheyStand);
        assert!(
            openshard_movement::step_allowed(&live, Point::new(10, 10, 0), Direction::SouthEast).is_none(),
            "a single blocked flank forbids the corner cut"
        );
        // The orthogonal step onto the open tile beside it is still fine.
        assert!(
            openshard_movement::step_allowed(&live, Point::new(10, 10, 0), Direction::South).is_some(),
            "the cardinal step is unaffected"
        );
    }

    #[test]
    fn unblocking_frees_the_tile_and_blocking_twice_is_one_obstacle() {
        let mut obstructions = Obstructions::default();
        let door = an_entity();
        obstructions.block(5, 5, door, Cover::door(0, DOOR_HEIGHT));
        obstructions.block(5, 5, door, Cover::door(0, DOOR_HEIGHT));
        obstructions.unblock(5, 5, door);
        assert!(!obstructions.holds_anything(5, 5));
    }

    #[test]
    fn an_upper_floor_blocker_leaves_the_ground_floor_open() {
        // The Britain-library bug: a placed impassable static on an upper floor
        // (z 20, a wall 20 tall) must not seal the ground beneath it, but one at
        // ground level must still block. The mobile steps at z 0.
        let mut obstructions = Obstructions::default();
        obstructions.block(10, 10, an_entity(), Cover::blocking(20, 20));
        let live_overlay = project(&obstructions, &NO_BOATS);
        let live = Footing::new(None, &live_overlay, Doors::AsTheyStand);
        assert!(
            openshard_movement::can_step(&live, Point::new(10, 9, 0), Point::new(10, 10, 0)).is_some(),
            "an upper-floor wall does not block the floor below"
        );

        obstructions.block(11, 10, an_entity(), Cover::blocking(0, 20));
        let live_overlay = project(&obstructions, &NO_BOATS);
        let live = Footing::new(None, &live_overlay, Doors::AsTheyStand);
        assert!(
            openshard_movement::can_step(&live, Point::new(11, 9, 0), Point::new(11, 10, 0)).is_none(),
            "but a ground-level wall still blocks"
        );
    }

    /// A map with a coastline down `x = 100`, so `land_is_water` has a real
    /// answer to give and a missing forward reads as the trait's default rather
    /// than as a plausible answer.
    ///
    /// Water is a *flag on a land row*, which is why this is a scene and not a
    /// double: a fixture that overrode `land_is_water` would be agreeing with
    /// itself about the one thing the test is checking gets through.
    fn charted() -> Scene {
        let mut scene = Scene::flat_holding(110, 8, 0);
        scene.land_art(OPEN_WATER, TileFlags::WATER);
        scene.land_art(SHORE, 0);
        for y in 0..scene.height() {
            for x in 0..100 {
                scene.land(x, y, SHORE);
            }
        }
        scene
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
        let charted = charted();
        let live_overlay = project(&obstructions, &NO_BOATS);
        let live = Footing::new(Some(charted.terrain()), &live_overlay, Doors::AsTheyStand);

        assert!(live.map.unwrap().land_is_water(Tile::new(100, 5)), "the sea");
        assert!(
            !live.map.unwrap().land_is_water(Tile::new(99, 5)),
            "and the shore"
        );
    }

    /// And with no map there is no sea, so a shard running without client files
    /// has nowhere to moor a boat.
    ///
    /// It used to ask a `MapTerrain` that had been made an `Option` under it and
    /// unwrapped the `None`, so it had not passed since. There is nothing to
    /// ask: water is the map's word and a footing with no map has no word. What
    /// is left worth asserting is the consequence — the mooring rule reads the
    /// same footing, and it refuses.
    #[test]
    fn a_live_terrain_with_no_map_reports_no_water() {
        let obstructions = Obstructions::default();
        let live_overlay = project(&obstructions, &NO_BOATS);
        let live = Footing::new(None, &live_overlay, Doors::AsTheyStand);
        assert!(live.map.is_none(), "a shard with no client files has no map");
        assert!(
            openshard_movement::step_allowed(&live, Point::new(100, 5, 0), Direction::North).is_some(),
            "and the tile the sea would be on is walked like any other"
        );
    }

    /// A sea with one strip of shore along `y = 0`, and nothing else to stand on.
    ///
    /// Wide enough for [`boat_step_cost`] to walk a thousand tiles of it. Every
    /// refusal here is the map's own rule about water rather than a fixture's
    /// opinion, which is the point: what these tests check is that a moored ship
    /// overturns a refusal the *real* rule made.
    fn sea() -> Scene {
        let mut scene = Scene::flat_holding(1001, 4, 0);
        scene.land_art(OPEN_WATER, TileFlags::WATER);
        scene.land_art(SHORE, 0);
        for x in 0..scene.width() {
            scene.land(x, 0, SHORE);
        }
        // What a ship is made of is a fact about the install, so it lives in the
        // tile table beside the water rather than in the plank fixture.
        scene.art(DECK, TileFlags::PLATFORM, 2);
        scene.art(HULL, TileFlags::WALL | TileFlags::BLOCK, 10);
        scene
    }

    /// Two deck tiles whose surface is z 2, and a hull tile beside them.
    ///
    /// **The art is the scene's**, and read through [`Plank::of_art`] — the one
    /// reading. A fixture that assembled a plank out of a height and a flag
    /// would be asserting against a rule the shard no longer has: a component
    /// is a floor because its tiledata row says `PLATFORM`, not because it
    /// fails to say `BLOCK`.
    ///
    /// **The deck stands two above the shore, and that is the most it may be.**
    /// A walk climbs at most `MAX_STEP_UP`, and these tests assert a body
    /// *walking* aboard from the shore at z 0. The fixture used to put the
    /// surface at 5 and pass, because `walk::aboard` applied no climb limit at
    /// all; it applies the same one `climbed` does now. See `walk.rs`'s
    /// `boarding_from_open_water_obeys_the_climb_limit`, and `docs/boats.md`
    /// for how a UO player really boards — over the plank, which teleports
    /// rather than steps.
    fn a_ship_at(scene: &Scene, boat: EntityId, x: u16, y: u16) -> Boats {
        let deck = scene.tiles().static_tile(DECK);
        let hull = scene.tiles().static_tile(HULL);
        let mut boats = Boats::default();
        boats.moor(
            boat,
            [
                ((x, y), Plank::of_art(boat, deck, 0)),
                ((x, y + 1), Plank::of_art(boat, deck, 0)),
                ((x + 1, y), Plank::of_art(boat, hull, 2)),
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
        let sea = sea();
        let boats = a_ship_at(&sea, an_entity(), 10, 1);
        let live_overlay = project(&obstructions, &boats);
        let live = Footing::new(Some(sea.terrain()), &live_overlay, Doors::AsTheyStand);

        assert!(
            openshard_movement::can_step(&live, Point::new(20, 0, 0), Point::new(20, 1, 0)).is_none(),
            "open water with no ship on it is still not walkable",
        );
        assert_eq!(
            openshard_movement::can_step(&live, Point::new(10, 0, 0), Point::new(10, 1, 0)),
            Some(Point::new(10, 1, 2)),
            "stepping aboard from the shore lands on the deck, not in the water",
        );
        assert_eq!(
            openshard_movement::can_step(&live, Point::new(10, 1, 2), Point::new(10, 2, 2)),
            Some(Point::new(10, 2, 2)),
            "and walking along the deck stays on it",
        );
    }

    /// The hull is a wall that is not in the obstruction index, so it is asked
    /// separately — and asked at the landing height, because a gunwale seals the
    /// deck and not the sea beneath the ship.
    #[test]
    fn a_hull_refuses_the_step_a_deck_would_have_allowed() {
        let obstructions = Obstructions::default();
        let sea = sea();
        let boats = a_ship_at(&sea, an_entity(), 10, 1);
        let live_overlay = project(&obstructions, &boats);
        let live = Footing::new(Some(sea.terrain()), &live_overlay, Doors::AsTheyStand);

        assert!(
            openshard_movement::can_step(&live, Point::new(10, 1, 2), Point::new(11, 1, 2)).is_none(),
            "walked straight through the hull",
        );
    }

    /// With no ships anywhere the answers are exactly what they were before
    /// boats existed. The regression that matters most, because every other
    /// facet on every shard is this one.
    #[test]
    fn an_empty_harbour_changes_no_answer() {
        let obstructions = Obstructions::default();
        let sea = sea();
        let live_overlay = project(&obstructions, &NO_BOATS);
        let live = Footing::new(Some(sea.terrain()), &live_overlay, Doors::AsTheyStand);

        assert!(openshard_movement::can_step(&live, Point::new(10, 0, 0), Point::new(10, 1, 0)).is_none());
        assert_eq!(
            openshard_movement::can_step(&live, Point::new(10, 0, 0), Point::new(11, 0, 0)),
            Some(Point::new(11, 0, 0)),
        );
        assert!(!openshard_movement::can_fit(&live, Tile::new(10, 1), 5, 16));
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
    /// # What it measured, 2026-08-22
    ///
    /// Release, 100,000 steps: **11.0ms with no boats, 12.3ms with one moored**
    /// — 110ns against 123ns a step, so the moored ship costs **13ns and 12%**.
    ///
    /// The empty case is the one that matters, because it is every facet on
    /// every shard that has no ships, and it is the `is_empty` length check
    /// doing its job.
    ///
    /// # The earlier reading, and why it was replaced
    ///
    /// 2026-08-16 recorded **1.5ms against 5.5ms** — 15ns and 55ns a step, a
    /// 3.6× — and said in the same breath why that framing was the least
    /// flattering available: the `Sea` it walked was a test double whose
    /// `can_step` was one integer comparison, so the boat lookup was very
    /// nearly the whole of the measured work. It predicted its own correction:
    /// *"a real `MapTerrain::can_step` reads map blocks, walks the statics on
    /// the tile and computes surfaces; against that baseline the same absolute
    /// cost is a small fraction rather than a multiple."*
    ///
    /// `docs/map/terrain_seam.md`'s node D replaced that double with a `Scene`
    /// building a real `MapTerrain`, which made the prediction testable and the
    /// old number stale. It came out as predicted: 12% of a real step rather
    /// than 267% of a synthetic one. Both readings are kept because the pair is
    /// the point — the same probe against two baselines, and only one of them
    /// was a world.
    ///
    /// If it ever needs to be smaller, the obvious move is a bounding box per
    /// facet — one integer range test to skip the probe for tiles nowhere near
    /// any ship. Not done, and now less justified than it was: 13ns does not
    /// buy a second structure to keep in step.
    #[test]
    #[ignore = "a measurement, not an assertion — see the doc comment"]
    fn boat_step_cost() {
        const STEPS: u32 = 100_000;
        let obstructions = Obstructions::default();
        let sea = sea();
        let busy = a_ship_at(&sea, an_entity(), 500, 1);

        let walk = |boats: &Boats| {
            let live_overlay = project(&obstructions, boats);
            let live = Footing::new(Some(sea.terrain()), &live_overlay, Doors::AsTheyStand);
            let start = std::time::Instant::now();
            let mut allowed = 0u32;
            for step in 0..STEPS {
                let x = (step % 1000) as u16;
                if openshard_movement::can_step(
                    &live,
                    Point::new(x, 0, 0),
                    Point::new(x.wrapping_add(1), 0, 0),
                )
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
