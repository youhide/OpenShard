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
//! Tiles are blocked whole, with no z-span: a door fills its doorway. The day
//! multi-storey interiors need a rug on one floor to not block the floor above,
//! an `Obstacle` can learn a z-range; nothing here forbids it.

use std::collections::HashMap;

use openshard_entities::EntityId;
use openshard_movement::{OpenWorld, Terrain};
use openshard_protocol::Point;

/// One entity blocking a tile.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Obstacle {
    /// The blocking entity.
    pub entity: EntityId,
    /// A closed door: a mobile that knows how may open it rather than walk
    /// around, so movement wants to know *what* blocked, not just that.
    pub door: bool,
}

/// The dynamic obstacles on one facet: tile → the entities blocking it.
#[derive(Default, Debug)]
pub struct Obstructions {
    tiles: HashMap<(u16, u16), Vec<Obstacle>>,
}

impl Obstructions {
    /// Mark `entity` as blocking `(x, y)`. Blocking twice is idempotent.
    pub fn block(&mut self, x: u16, y: u16, entity: EntityId, door: bool) {
        let tile = self.tiles.entry((x, y)).or_default();
        if let Some(existing) = tile.iter_mut().find(|o| o.entity == entity) {
            // Re-registering refines what the blocker is — a doorway placed as
            // plain impassable art and then given its `Door` stays one obstacle.
            existing.door = door;
        } else {
            tile.push(Obstacle { entity, door });
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

    /// The first thing blocking `(x, y)`, if anything is.
    #[must_use]
    pub fn blocker_at(&self, x: u16, y: u16) -> Option<Obstacle> {
        self.tiles.get(&(x, y)).and_then(|t| t.first().copied())
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
        through_doors: bool,
    ) -> Self {
        Self {
            map,
            obstructions,
            through_doors,
        }
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
            Some(map) => map.can_step(from, to)?,
            None => OpenWorld.can_step(from, to)?,
        };
        match self.obstructions.blocker_at(to.x, to.y) {
            Some(o) if o.door && self.through_doors => Some(landed),
            Some(_) => None,
            None => Some(landed),
        }
    }

    fn ground_z(&self, x: u16, y: u16) -> Option<i8> {
        self.map.and_then(|m| m.ground_z(x, y))
    }

    fn statics_at(&self, x: u16, y: u16, out: &mut Vec<(u16, i8)>) {
        if let Some(map) = self.map {
            map.statics_at(x, y, out);
        }
    }

    fn stand_z(&self, x: u16, y: u16, near_z: i32) -> Option<i32> {
        self.map.and_then(|m| m.stand_z(x, y, near_z))
    }

    fn can_fit(&self, x: u16, y: u16, z: i32, height: i32) -> bool {
        self.map.is_none_or(|m| m.can_fit(x, y, z, height)) && !self.obstructions.is_blocked(x, y)
    }

    fn sight_clear(&self, from: Point, to: Point) -> bool {
        if !self.map.is_none_or(|m| m.sight_clear(from, to)) {
            return false;
        }
        // A shut door is opaque; a crate is furniture, not a wall.
        openshard_movement::line_tiles((from.x, from.y), (to.x, to.y))
            .into_iter()
            .all(|(x, y)| self.obstructions.blocker_at(x, y).is_none_or(|o| !o.door))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openshard_entities::Registry;

    fn an_entity() -> EntityId {
        Registry::new().spawn()
    }

    #[test]
    fn a_blocked_tile_refuses_a_step_the_open_world_allows() {
        let mut obstructions = Obstructions::default();
        let door = an_entity();
        obstructions.block(10, 10, door, true);
        let live = LiveTerrain::new(None, &obstructions, false);
        assert!(live
            .can_step(Point::new(10, 9, 0), Point::new(10, 10, 0))
            .is_none());
        assert!(live
            .can_step(Point::new(10, 9, 0), Point::new(11, 9, 0))
            .is_some());
    }

    #[test]
    fn a_door_opener_plans_through_a_door_but_not_through_a_crate() {
        let mut obstructions = Obstructions::default();
        obstructions.block(10, 10, an_entity(), true);
        obstructions.block(12, 10, an_entity(), false);
        let planner = LiveTerrain::new(None, &obstructions, true);
        assert!(planner
            .can_step(Point::new(10, 9, 0), Point::new(10, 10, 0))
            .is_some());
        assert!(planner
            .can_step(Point::new(12, 9, 0), Point::new(12, 10, 0))
            .is_none());
    }

    #[test]
    fn a_shut_door_is_opaque_and_an_open_one_is_not() {
        let mut obstructions = Obstructions::default();
        let door = an_entity();
        obstructions.block(10, 10, door, true);
        let live = LiveTerrain::new(None, &obstructions, false);
        assert!(!live.sight_clear(Point::new(10, 8, 0), Point::new(10, 12, 0)));
        obstructions.unblock(10, 10, door);
        let live = LiveTerrain::new(None, &obstructions, false);
        assert!(live.sight_clear(Point::new(10, 8, 0), Point::new(10, 12, 0)));
    }

    #[test]
    fn unblocking_frees_the_tile_and_blocking_twice_is_one_obstacle() {
        let mut obstructions = Obstructions::default();
        let door = an_entity();
        obstructions.block(5, 5, door, true);
        obstructions.block(5, 5, door, true);
        obstructions.unblock(5, 5, door);
        assert!(!obstructions.is_blocked(5, 5));
    }
}
