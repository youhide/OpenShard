//! Turning a walk request into a step, or a refusal.

use std::time::Instant;

use openshard_protocol::direction::{Direction, Facing};
use openshard_protocol::wire::Graphic;
use openshard_protocol::world::{Point, WalkRequest};

use crate::pace::{Pace, WalkPace};
use crate::sequence::WalkSequence;

/// What a walk request did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[must_use = "a walk outcome has to reach the client, or it will stop walking"]
pub enum Walk {
    /// The mobile turned on the spot and did not move.
    ///
    /// UO makes turning a whole step: a mobile facing north that is asked to go
    /// east *turns* east and stays put, and only the next request moves it.
    /// This is not a quirk to paper over — the client animates the turn and
    /// expects the ack, so collapsing it into a move desynchronises the two.
    Turned {
        /// The new facing.
        facing: Facing,
    },
    /// The mobile took a step.
    Moved {
        /// Where it is now.
        position: Point,
        /// Which way it is facing.
        facing: Facing,
    },
    /// The step is refused. The client snaps back and resets its sequence.
    Refused,
}

/// Whether a mobile may stand somewhere.
///
/// Every question here takes a **coordinate** and is answered by the map with
/// whatever the live world has laid over it. That is the whole subject: what a
/// graphic weighs, how tall it is or which hand it is held in used to be asked
/// here too, and none of those could be changed by a placed crate — they were a
/// `tiledata.mul` lookup wearing a terrain's coat, and they now go to the table
/// directly (`WorldState::tiles`, `openshard_uofiles::tiledata::TileData`).
///
/// A trait for now, and not for much longer: five of its six implementors are an
/// *action over* a terrain rather than a terrain — see `docs/map/terrain_seam.md`.
/// [`OpenWorld`] is the answer when there is no map at all.
pub trait Terrain {
    /// Can a mobile at `from` step to `to`?
    ///
    /// `to`'s `z` is a guess from the caller; an implementation that knows the
    /// map should correct it and return the real height.
    fn can_step(&self, from: Point, to: Point) -> Option<Point>;

    /// The ground height at `tile`, if this terrain knows one.
    ///
    /// Where a character spawns: the map holds the floor, not the config. An
    /// implementation with no map — [`OpenWorld`] — returns `None`, and the
    /// caller falls back to a flat default.
    fn ground_z(&self, _tile: Tile) -> Option<i8> {
        None
    }

    /// The *land* tile id at `tile` — the index into `tiledata.mul`'s land
    /// table, not a static's graphic.
    ///
    /// Read for what the ground *is* rather than how high it stands: a mountain
    /// face is a land tile a pickaxe works and a patch of sand is a land tile a
    /// shovel does, and neither can be told apart by height. It exists because the
    /// client does not send it — a `0x6C` location reply carries a graphic only
    /// when a *static* was clicked, and a click on bare land arrives with a
    /// graphic of zero (ServUO `PacketHandlers.cs`, the `LandTarget` branch), so
    /// the server has to look the tile up itself.
    fn land_tile(&self, _tile: Tile) -> Option<crate::LandTile> {
        None
    }

    /// The static tiles standing at `tile`, appended to `out` as
    /// `(graphic, z)` pairs.
    ///
    /// Only what the map holds; a terrain with no statics (an open world) adds
    /// nothing. The primitive tuple keeps this trait — which lives below `world` —
    /// free of the map's own types. Used to find door frames when generating the
    /// functional doors a building's static art only implies.
    fn statics_at(&self, _tile: Tile, _out: &mut Vec<(Graphic, i8)>) {}

    /// The z a mobile stands at on `tile`, reached from near `near_z` — the top
    /// of the walkable surface there, a building's raised floor and all.
    ///
    /// Where a spawn drops onto the ground: the pack gives a tile and a rough
    /// height, and the map says which floor that lands on (asking from `near_z`
    /// rather than the sky, so it finds the floor and not the roof above it).
    /// `None` when the tile has no reachable surface, or the terrain has no map.
    fn stand_z(&self, _tile: Tile, _near_z: i32) -> Option<i32> {
        None
    }

    /// Where to *place* a mobile on `tile`, near `near_z` — like
    /// [`stand_z`](Self::stand_z), but not bound by one step's reach.
    ///
    /// A spawn is not a step: a shopkeeper placed at ground level belongs on the
    /// building's raised floor above it, which [`stand_z`](Self::stand_z) refuses
    /// because it is more than a step up. This finds the surface a mobile actually
    /// fits on regardless of how far it is from `near_z`, so an NPC stops sinking
    /// through the shop floor. Defaults to [`stand_z`](Self::stand_z) for a terrain
    /// with no map.
    fn spawn_z(&self, tile: Tile, near_z: i32) -> Option<i32> {
        self.stand_z(tile, near_z)
    }

    /// Whether an object `height` tall can sit at `tile, z` — nothing solid in
    /// its body, and a surface under it to rest on.
    ///
    /// This is what keeps a generated door in a real doorway: a door belongs in an
    /// open gap with a floor, so a spot that is a solid wall (something in the way)
    /// or thin air (no surface) reports that nothing fits. An open world fits
    /// everything — it has no walls and, having no map, generates no doors anyway.
    fn can_fit(&self, _tile: Tile, _z: i32, _height: i32) -> bool {
        true
    }

    /// Whether the land at `tile` is water — the tiledata flag, not a guess from
    /// the land id.
    ///
    /// A terrain with no map answers `false`, which is the same bargain every
    /// other method here makes: a shard with no client files has no sea, so it
    /// has nowhere to moor a boat, and refusing every mooring is the safe half
    /// of that answer rather than the surprising one.
    ///
    /// A *coordinate* and not a graphic, which is what keeps it here while the
    /// tiledata questions have left: it cannot be answered without the map, since
    /// the map is what says which land tile is at `tile` in the first place.
    ///
    /// The step check already reads this flag — [`can_step`](Self::can_step)
    /// treats water as ground only for a swimming body — but it reads it *inside*
    /// a decision and never says so. A boat needs the fact on its own: not "may I
    /// walk here" but "is this the sea", and the two differ for a swimmer.
    fn land_is_water(&self, _tile: Tile) -> bool {
        false
    }

    /// Whether a straight sight line from `from` to `to` is clear of walls.
    ///
    /// What gates a creature noticing prey: both reference emulators require
    /// line of sight to *acquire* a target, and relax to cheaper range checks
    /// only for continuing a chase already begun. A terrain with no map hides
    /// nothing.
    fn sight_clear(&self, _from: Point, _to: Point) -> bool {
        true
    }
}

/// A tile's column and row, with no height — a [`Point`] flattened to the plane
/// a sight line or a path search reasons on.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Tile {
    /// East-west tile.
    pub x: u16,
    /// North-south tile.
    pub y: u16,
}

impl Tile {
    /// A tile at `(x, y)`.
    pub const fn new(x: u16, y: u16) -> Self {
        Self { x, y }
    }
}

/// The tiles a straight line from `from` to `to` crosses, endpoints excluded —
/// where a sight line looks for walls. Plain integer Bresenham; the endpoints
/// are the looker and the looked-at, and neither occludes itself.
#[must_use]
pub fn line_tiles(from: Tile, to: Tile) -> Vec<Tile> {
    let (mut x, mut y) = (i32::from(from.x), i32::from(from.y));
    let (tx, ty) = (i32::from(to.x), i32::from(to.y));
    let dx = (tx - x).abs();
    let dy = -(ty - y).abs();
    let sx = if x < tx { 1 } else { -1 };
    let sy = if y < ty { 1 } else { -1 };
    let mut err = dx + dy;
    let mut tiles = Vec::new();
    loop {
        if x == tx && y == ty {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
        if x == tx && y == ty {
            break;
        }
        tiles.push(Tile::new(x as u16, y as u16));
    }
    tiles
}

/// A world with no floor and no walls: every step is allowed, z never changes.
///
/// What a shard runs with no client files configured, and what these tests run
/// against. Useful for proving the handshake in isolation; useless as a game, so
/// the server warns at startup when it is in use.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct OpenWorld;

impl Terrain for OpenWorld {
    fn can_step(&self, _from: Point, to: Point) -> Option<Point> {
        Some(to)
    }
}

/// What a walk request means before terrain has a say.
///
/// The half of a step both ends of the wire have to agree on without talking:
/// the server decides it from the mobile it holds, and the client predicts it
/// from the body it is drawing, because `0x22` says only "yes" — it carries no
/// position. Two implementations of the turn rule would put the two a tile
/// apart, and the client would only find out on the next `0x21`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Intent {
    /// The request turns the mobile on the spot and moves it nowhere.
    Turned {
        /// The new facing.
        facing: Facing,
    },
    /// The request moves it, if the ground allows.
    Stepped {
        /// The tile it is asking for. `z` is carried over unchanged — height is
        /// the terrain's answer, and this function has no terrain.
        target: Point,
        /// Which way it will face.
        facing: Facing,
    },
    /// The step leaves the coordinate space; there is nowhere to allow.
    OffTheMap,
}

/// What one `0x02` means for a mobile at `position` facing `facing`.
///
/// Terrain, pace and the sequence are all somebody else's answer — this is only
/// the geometry, which is exactly the part a client can work out for itself.
#[must_use]
pub fn intend(position: Point, facing: Facing, want: Facing) -> Intent {
    // Turning is a step of its own. A mobile facing north asked to go east
    // turns to face east and stays where it is; the *next* request moves it.
    // The running bit is not part of this — a walking mobile asked to run the
    // way it already faces takes a step, it does not "turn".
    if facing.direction != want.direction {
        return Intent::Turned { facing: want };
    }
    match step_from(position, want.direction) {
        Some(target) => Intent::Stepped { target, facing: want },
        None => Intent::OffTheMap,
    }
}

/// One mobile's walk state.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Walker {
    /// Where it is.
    pub position: Point,
    /// Which way it faces.
    pub facing: Facing,
    /// Its walk sequence.
    pub sequence: WalkSequence,
    /// How fast it is allowed to move.
    pub pace: WalkPace,
}

impl Walker {
    /// A walker standing at `position`, facing `facing`, fresh.
    pub const fn new(position: Point, facing: Facing) -> Self {
        Self {
            position,
            facing,
            sequence: WalkSequence::new(),
            pace: WalkPace::new(),
        }
    }

    /// Handle a `0x02` walk request.
    ///
    /// The caller sends `0x22` for [`Walk::Turned`] and [`Walk::Moved`], and
    /// `0x21` for [`Walk::Refused`].
    ///
    /// `now` is a parameter rather than read here, so that a whole walk — pace
    /// and all — is a deterministic test with no `sleep` in it.
    /// `mounted` is passed rather than kept on the walker, and deliberately: a mount
    /// is put on and taken off, and a copy of that here is one more thing to keep in
    /// step with the world (the read-site-derivation argument `equipped_weapon` makes).
    /// The caller already knows, because it just looked the rider up.
    pub fn request(
        &mut self,
        request: WalkRequest,
        terrain: &dyn Terrain,
        now: Instant,
        mounted: bool,
    ) -> Walk {
        // The one place the request's sequence byte is *read* rather than
        // echoed, and the check it goes through: `.0` here because this is that
        // seam. What comes back out on a `0x22`/`0x21` is the same byte,
        // interpreted rather than validated — see
        // `openshard_protocol::world::RawStepSequence::interpret`.
        if self.sequence.accept(request.sequence).is_err() {
            self.sequence.reset();
            return Walk::Refused;
        }

        // The geometry, decided by the same function the client predicts with —
        // see [`intend`], and the reason it is shared rather than written twice.
        //
        // A turn is free: it costs no ground, so charging for it would let a
        // client be throttled for spinning on the spot, which is not a speedhack
        // and is something clients genuinely do. That is why the pace check sits
        // below the turn and above the step.
        let intent = intend(self.position, self.facing, request.facing);
        if let Intent::Turned { facing } = intent {
            self.facing = facing;
            return Walk::Turned { facing: self.facing };
        }

        if self.pace.allow(now, request.facing.running, mounted) == Pace::TooFast {
            // Moving faster than a body can move. Refuse the step rather than
            // the connection: the client snaps back, which is what a legitimate
            // one needs and what an illegitimate one deserves.
            self.sequence.reset();
            return Walk::Refused;
        }

        let Intent::Stepped { target, .. } = intent else {
            // Walked off the edge of the coordinate space. The client cannot
            // express where it wanted to go, so there is nowhere to allow.
            self.sequence.reset();
            return Walk::Refused;
        };

        let Some(landed) = terrain.can_step(self.position, target) else {
            self.sequence.reset();
            return Walk::Refused;
        };

        self.position = landed;
        self.facing = request.facing;
        Walk::Moved {
            position: self.position,
            facing: self.facing,
        }
    }
}

/// The eight-way direction from one tile toward another, or `None` when they are
/// the same tile. Shared by the creature brain and the townsfolk who turn to face
/// whoever they greet.
///
/// This only ever looks at *sign* — which quadrant `to` sits in — never at how
/// far off either axis it is, and that is correct for what calls it: a single
/// step, or a facing toward an adjacent tile, has nothing else to weigh. Fed a
/// distant target instead it degenerates: 99 tiles east and 1 south is exactly
/// as diagonal to this as 1 tile east and 1 south, because every quadrant off
/// both axes answers `NorthEast`/`SouthEast`/`SouthWest`/`NorthWest` regardless
/// of the ratio. [`heading_toward`] is the sibling for that case.
pub fn direction_toward(from: Point, to: Point) -> Option<Direction> {
    let dx = (i32::from(to.x) - i32::from(from.x)).signum();
    let dy = (i32::from(to.y) - i32::from(from.y)).signum();
    match (dx, dy) {
        (0, 0) => None,
        (0, -1) => Some(Direction::North),
        (1, -1) => Some(Direction::NorthEast),
        (1, 0) => Some(Direction::East),
        (1, 1) => Some(Direction::SouthEast),
        (0, 1) => Some(Direction::South),
        (-1, 1) => Some(Direction::SouthWest),
        (-1, 0) => Some(Direction::West),
        _ => Some(Direction::NorthWest),
    }
}

/// The eight-way *heading* from one point toward another — a compass sector,
/// each spanning 45° and centred on its direction, rather than [`direction_toward`]'s
/// quadrant-by-sign. `None` when the two points coincide.
///
/// This is what a continuous target wants: the mouse-steer heading in
/// `client/app`'s `Steering::steer`, and the straight-line fallback a stalled
/// destination in `Steering::take` degrades to. Both name a point that can be
/// almost anywhere relative to the body, and [`direction_toward`] would call
/// nearly all of that diagonal — a cursor one tile south and twenty tiles east
/// of the body is due east, not south-east, and a fair sector split is what
/// says so.
pub fn heading_toward(from: Point, to: Point) -> Option<Direction> {
    let (dx, dy) = (
        i32::from(to.x) - i32::from(from.x),
        i32::from(to.y) - i32::from(from.y),
    );
    if dx == 0 && dy == 0 {
        return None;
    }
    // atan2(dy, dx): 0° at due east, 90° at due south — tile `y` grows south,
    // the same sense as screen `y`, so no sign flip is needed to match it.
    // Rounding to the nearest 45° is the sector split: each direction owns the
    // 45° centred on its own bearing, not the read of just `dx`'s and `dy`'s
    // signs that flattens everything off-axis to a diagonal.
    let octant = (f64::from(dy).atan2(f64::from(dx)).to_degrees() / 45.0).round() as i64;
    Some(match octant.rem_euclid(8) {
        0 => Direction::East,
        1 => Direction::SouthEast,
        2 => Direction::South,
        3 => Direction::SouthWest,
        4 => Direction::West,
        5 => Direction::NorthWest,
        6 => Direction::North,
        _ => Direction::NorthEast,
    })
}

/// A heading, with the part of it the eight sectors throw away.
///
/// A sector is 45° wide and a body can only be sent along its centre, so
/// rounding to one is lossy by construction — and what it loses is the very
/// thing a player is saying when they hold the cursor a little to one side of
/// a corner. See [`Lean`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Heading {
    /// The sector: which of the eight ways this heading is nearest to.
    pub direction: Direction,
    /// Where inside that sector it actually points.
    pub lean: Lean,
}

impl Heading {
    /// A heading with nothing to say beyond its sector — what a held arrow key
    /// is, and what a direction reconstructed from anything but a real bearing
    /// has to be.
    #[must_use]
    pub const fn centred(direction: Direction) -> Self {
        Self {
            direction,
            lean: Lean::Centred,
        }
    }
}

/// Which side of its own sector a heading falls on: the sub-sector detail that
/// rounding to one of eight directions discards.
///
/// It is what a player leaning the cursor past a corner is saying, and there is
/// no other way to say it — the body can only be sent along a sector's centre,
/// so the *ask* is quantised even though the pointing was not. Where it matters
/// is a tie: two ways past an obstacle, both legal, and no reason in the
/// terrain to prefer either. The cursor has the reason.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Lean {
    /// Dead on the sector's own bearing — and exactly so, by integer
    /// arithmetic rather than by a tolerance (see [`Lean::of`]). A held arrow
    /// key is this, and so is a cursor squarely on the diagonal.
    #[default]
    Centred,
    /// Past the centre, the way the compass turns: north-east of north,
    /// south-east of east. The clockwise flank is the one being pointed at.
    Clockwise,
    /// Past the centre the other way.
    Counter,
}

impl Lean {
    /// Which side of the bearing `(ux, uy)` the vector `(dx, dy)` falls on.
    ///
    /// The sign of their 2D cross product — positive is clockwise where `y`
    /// grows downward, which is true of both spaces this is used in: the tile
    /// grid, whose `y` grows south, and the screen it is drawn on. It is the
    /// same answer in both, because the projection between them turns the
    /// plane without flipping it — so a caller measuring on the screen and a
    /// caller measuring on the grid mean the same thing by `Clockwise`, and
    /// [`crate::Detour`] can take either without being told which.
    ///
    /// The bearing is a vector and not a [`Direction`] because the screen
    /// bearing of a direction is not its grid step, and asking a pointing
    /// device is the whole point.
    ///
    /// Integer arithmetic on purpose: a tolerance would have to name a number
    /// of degrees, and the case that has to be exact is a cursor pointing
    /// *squarely* along a bearing — which is a cross product of zero, and
    /// nothing about trigonometry. Through `atan2` that same case comes out as
    /// 45.000000000000007 against 45, and a player pointing straight at a
    /// corner would lean one way for no reason anyone could see.
    #[must_use]
    pub const fn of(ux: i32, uy: i32, dx: i32, dy: i32) -> Self {
        match ux * dy - uy * dx {
            0 => Self::Centred,
            cross if cross > 0 => Self::Clockwise,
            _ => Self::Counter,
        }
    }
}

/// Where one step from `position` lands, or `None` at the world's edge.
///
/// The map is addressed with `u16`s, so a step west from x=0 has no
/// representation. Returning `None` rather than wrapping matters: wrapping would
/// teleport a mobile from Britain's west shore to the far east of the map.
pub fn step_from(position: Point, direction: Direction) -> Option<Point> {
    let (dx, dy) = direction.step();
    let x = u16::try_from(i32::from(position.x) + dx).ok()?;
    let y = u16::try_from(i32::from(position.y) + dy).ok()?;
    Some(Point { x, y, z: position.z })
}

/// Where one *legal* step from `from` lands, or `None` when that step is not a
/// step this world allows at all.
///
/// [`Terrain::can_step`] answers for the destination tile alone — is there
/// ground, does the body fit, is the climb within [`MAX_STEP_UP`]. That is the
/// whole answer for a cardinal and only half of it for a diagonal, which also
/// may not clip the corner where two blockers meet: both cardinal tiles
/// flanking it must themselves be steppable, the same rule the client enforces
/// and the rule `openshard_state::obstruct`'s `LiveTerrain` applies to every
/// step that reaches the wire.
///
/// It lives here, above every caller, because the three that need it are not
/// one layer: [`find_path`](crate::find_path) planning a route, the shard
/// validating a creature's step, and the client's own held-direction detour
/// deciding whether the way ahead is open. A [`Terrain`] implementation is free
/// to *also* refuse a corner — `LiveTerrain` does, because it is the last word
/// before a `0x21` — but nothing may rely on one that does not, and
/// [`MapTerrain`](crate::MapTerrain), the static map both ends share, is
/// exactly such a one. A client asking `can_step` directly therefore believes a
/// corner-cutting diagonal is walkable, sends it, and is rubber-banded — which
/// is a body stuck against a building corner for as long as the player holds
/// that direction.
#[must_use]
pub fn step_allowed(terrain: &dyn Terrain, from: Point, direction: Direction) -> Option<Point> {
    let to = step_from(from, direction)?;
    if direction.is_diagonal() && !corner_open(terrain, from, direction) {
        return None;
    }
    terrain.can_step(from, to)
}

/// Whether both cardinal tiles flanking a diagonal are steppable, so the
/// diagonal does not cut through a wall's corner. The flanks of a diagonal are
/// the two wire directions either side of it (NE lies between N and E).
fn corner_open(terrain: &dyn Terrain, from: Point, diagonal: Direction) -> bool {
    let d = diagonal.to_bits();
    let flanks = [
        Direction::from_bits((d + 7) % 8),
        Direction::from_bits((d + 1) % 8),
    ];
    flanks.iter().all(|&card| {
        step_from(from, card)
            .and_then(|tile| terrain.can_step(from, tile))
            .is_some()
    })
}

#[cfg(test)]
mod tests {
    use openshard_protocol::world::{RawFastwalkKey, RawStepSequence};

    use super::*;

    /// Where [`direction_toward`] flattens: far more east than south still
    /// reads as a diagonal, because only the signs of `dx`/`dy` are looked at.
    #[test]
    fn direction_toward_is_diagonal_off_both_axes_however_lopsided() {
        let from = Point::new(100, 100, 0);
        assert_eq!(
            direction_toward(from, Point::new(199, 101, 0)),
            Some(Direction::SouthEast),
            "99 tiles east and 1 south is still called diagonal by the sign-only version"
        );
    }

    /// [`heading_toward`] is the fair-sector sibling: the same lopsided target
    /// reads as the axis it is overwhelmingly closer to.
    #[test]
    fn heading_toward_favors_the_dominant_axis() {
        let from = Point::new(100, 100, 0);
        assert_eq!(
            heading_toward(from, Point::new(199, 101, 0)),
            Some(Direction::East),
            "99 tiles east and 1 south is due east, not south-east"
        );
        assert_eq!(
            heading_toward(from, Point::new(101, 199, 0)),
            Some(Direction::South),
            "and the same holds on the other axis"
        );
    }

    /// Each of the eight sectors spans 45°, centred on its own direction —
    /// exercised on the diagonals, where a sign-only read and a sector read
    /// happen to agree, and on points just either side of a sector boundary,
    /// where they do not.
    #[test]
    fn heading_toward_splits_the_compass_into_eight_even_sectors() {
        let from = Point::new(100, 100, 0);
        let cases = [
            ((110, 100), Direction::East),
            ((110, 110), Direction::SouthEast),
            ((100, 110), Direction::South),
            ((90, 110), Direction::SouthWest),
            ((90, 100), Direction::West),
            ((90, 90), Direction::NorthWest),
            ((100, 90), Direction::North),
            ((110, 90), Direction::NorthEast),
            // Just inside East's sector (< 22.5° off the axis): 10 tiles east,
            // 4 south is ~21.8°.
            ((110, 104), Direction::East),
            // Just past it (> 22.5°): 10 tiles east, 5 south is ~26.6°.
            ((110, 105), Direction::SouthEast),
        ];
        for ((x, y), expected) in cases {
            assert_eq!(
                heading_toward(from, Point::new(x, y, 0)),
                Some(expected),
                "({x}, {y})"
            );
        }
    }

    #[test]
    fn heading_toward_the_same_tile_is_none() {
        let here = Point::new(100, 100, 0);
        assert_eq!(heading_toward(here, here), None);
    }

    #[test]
    fn a_sight_line_crosses_the_tiles_between_and_not_the_ends() {
        let tiles = line_tiles(Tile::new(10, 10), Tile::new(10, 13));
        assert_eq!(tiles, vec![Tile::new(10, 11), Tile::new(10, 12)]);
        let diagonal = line_tiles(Tile::new(0, 0), Tile::new(3, 3));
        assert_eq!(diagonal, vec![Tile::new(1, 1), Tile::new(2, 2)]);
        assert!(line_tiles(Tile::new(5, 5), Tile::new(5, 5)).is_empty());
        assert!(
            line_tiles(Tile::new(5, 5), Tile::new(6, 5)).is_empty(),
            "adjacent tiles see each other"
        );
    }

    fn request(direction: Direction, sequence: u8) -> WalkRequest {
        WalkRequest {
            facing: Facing::walking(direction),
            sequence: RawStepSequence(sequence),
            fastwalk_key: RawFastwalkKey(0),
        }
    }

    fn walker() -> Walker {
        Walker::new(Point::new(100, 100, 0), Facing::walking(Direction::North))
    }

    /// A fresh instant. The pace bucket starts full, so a handful of steps in
    /// one test never trip it.
    fn now() -> Instant {
        Instant::now()
    }

    #[test]
    fn walking_the_way_you_face_moves_you() {
        let mut walker = walker();
        let outcome = walker.request(request(Direction::North, 0), &OpenWorld, now(), false);
        assert_eq!(
            outcome,
            Walk::Moved {
                position: Point::new(100, 99, 0),
                facing: Facing::walking(Direction::North),
            }
        );
        assert_eq!(walker.position, Point::new(100, 99, 0));
    }

    #[test]
    fn turning_is_a_step_of_its_own() {
        // The thing that surprises people. A mobile facing north asked to go
        // east turns and stays put; the next request moves it. The client
        // animates the turn and waits for the ack, so collapsing this into a
        // move puts the two ends a tile apart.
        let mut walker = walker();
        let outcome = walker.request(request(Direction::East, 0), &OpenWorld, now(), false);
        assert_eq!(
            outcome,
            Walk::Turned {
                facing: Facing::walking(Direction::East)
            }
        );
        assert_eq!(walker.position, Point::new(100, 100, 0), "did not move");

        // Now it moves.
        let outcome = walker.request(request(Direction::East, 1), &OpenWorld, now(), false);
        assert_eq!(
            outcome,
            Walk::Moved {
                position: Point::new(101, 100, 0),
                facing: Facing::walking(Direction::East),
            }
        );
    }

    #[test]
    fn a_turn_still_consumes_a_sequence_number() {
        // It is a step as far as the client is concerned, and it gets an ack.
        let mut walker = walker();
        let _ = walker.request(request(Direction::East, 0), &OpenWorld, now(), false);
        assert_eq!(walker.sequence.expected(), 1);
    }

    #[test]
    fn starting_to_run_the_way_you_face_is_a_step_not_a_turn() {
        // The running bit changes but the direction does not, so there is
        // nothing to turn to. Treating this as a turn would cost a step every
        // time a player broke into a run.
        let mut walker = walker();
        let outcome = walker.request(
            WalkRequest {
                facing: Facing::running(Direction::North),
                sequence: RawStepSequence(0),
                fastwalk_key: RawFastwalkKey(0),
            },
            &OpenWorld,
            now(),
            false,
        );
        assert!(matches!(outcome, Walk::Moved { .. }));
        assert!(walker.facing.running);
    }

    #[test]
    fn every_direction_steps_the_right_way() {
        for direction in Direction::ALL {
            let mut walker = Walker::new(Point::new(100, 100, 0), Facing::walking(direction));
            let outcome = walker.request(request(direction, 0), &OpenWorld, now(), false);

            let (dx, dy) = direction.step();
            let expected = Point::new((100 + dx) as u16, (100 + dy) as u16, 0);
            assert_eq!(
                outcome,
                Walk::Moved {
                    position: expected,
                    facing: Facing::walking(direction),
                },
                "{direction}"
            );
        }
    }

    #[test]
    fn a_fresh_walker_that_does_not_start_at_zero_is_refused() {
        let mut walker = walker();
        assert_eq!(
            walker.request(request(Direction::North, 5), &OpenWorld, now(), false),
            Walk::Refused
        );
        assert_eq!(walker.position, Point::new(100, 100, 0), "did not move");
        assert!(walker.sequence.is_fresh(), "and stays fresh");
    }

    #[test]
    fn a_refusal_resets_the_sequence() {
        let mut walker = walker();
        let _ = walker.request(request(Direction::North, 0), &OpenWorld, now(), false);
        let _ = walker.request(request(Direction::North, 1), &OpenWorld, now(), false);

        // A wall.
        struct Wall;
        impl Terrain for Wall {
            fn can_step(&self, _from: Point, _to: Point) -> Option<Point> {
                None
            }
        }

        assert_eq!(
            walker.request(request(Direction::North, 2), &Wall, now(), false),
            Walk::Refused
        );
        assert!(
            walker.sequence.is_fresh(),
            "the client resets on 0x21, so the server must too"
        );
    }

    #[test]
    fn terrain_can_move_a_step_somewhere_else() {
        // What real terrain does: the caller guesses a z, the map corrects it.
        // Walking up a hill lands you higher than you asked for.
        struct Hill;
        impl Terrain for Hill {
            fn can_step(&self, _from: Point, to: Point) -> Option<Point> {
                Some(Point { z: to.z + 5, ..to })
            }
        }

        let mut walker = walker();
        let outcome = walker.request(request(Direction::North, 0), &Hill, now(), false);
        assert_eq!(
            outcome,
            Walk::Moved {
                position: Point::new(100, 99, 5),
                facing: Facing::walking(Direction::North),
            }
        );
        assert_eq!(walker.position.z, 5, "the walker believes the terrain");
    }

    #[test]
    fn the_world_edge_refuses_rather_than_wrapping() {
        // A step west from x=0 has no u16 to land on. Wrapping would put the
        // walker at x=65535 — the far side of the map, instantly.
        let mut walker = Walker::new(Point::new(0, 0, 0), Facing::walking(Direction::West));
        assert_eq!(
            walker.request(request(Direction::West, 0), &OpenWorld, now(), false),
            Walk::Refused
        );
        assert_eq!(walker.position, Point::new(0, 0, 0));

        let mut walker = Walker::new(
            Point::new(u16::MAX, u16::MAX, 0),
            Facing::walking(Direction::SouthEast),
        );
        assert_eq!(
            walker.request(request(Direction::SouthEast, 0), &OpenWorld, now(), false),
            Walk::Refused
        );
    }

    #[test]
    fn step_from_refuses_every_edge() {
        for direction in Direction::ALL {
            let (dx, dy) = direction.step();
            if dx < 0 || dy < 0 {
                assert_eq!(
                    step_from(Point::new(0, 0, 0), direction),
                    None,
                    "{direction} from the origin"
                );
            }
            if dx > 0 || dy > 0 {
                assert_eq!(
                    step_from(Point::new(u16::MAX, u16::MAX, 0), direction),
                    None,
                    "{direction} from the far corner"
                );
            }
        }
    }

    #[test]
    fn an_intent_is_a_turn_whenever_the_direction_changes() {
        // The rule the client predicts with and the server enforces, checked on
        // its own: what `turning_is_a_step_of_its_own` proves through a whole
        // `Walker`, this proves for every pair of directions.
        let here = Point::new(100, 100, 0);
        for from in Direction::ALL {
            for to in Direction::ALL {
                let intent = intend(here, Facing::walking(from), Facing::walking(to));
                if from == to {
                    assert!(
                        matches!(intent, Intent::Stepped { .. }),
                        "{from} to {to} is a step"
                    );
                } else {
                    assert_eq!(
                        intent,
                        Intent::Turned {
                            facing: Facing::walking(to)
                        },
                        "{from} to {to} is a turn"
                    );
                }
            }
        }
    }

    #[test]
    fn breaking_into_a_run_is_a_step_and_the_edge_is_neither() {
        let here = Point::new(100, 100, 0);
        assert_eq!(
            intend(
                here,
                Facing::walking(Direction::North),
                Facing::running(Direction::North)
            ),
            Intent::Stepped {
                target: Point::new(100, 99, 0),
                facing: Facing::running(Direction::North),
            },
            "the direction did not change, so there is nothing to turn to"
        );
        // A turn at the edge is still just a turn: it needs no tile.
        let corner = Point::new(0, 0, 0);
        assert_eq!(
            intend(
                corner,
                Facing::walking(Direction::North),
                Facing::walking(Direction::West)
            ),
            Intent::Turned {
                facing: Facing::walking(Direction::West)
            }
        );
        assert_eq!(
            intend(
                corner,
                Facing::walking(Direction::West),
                Facing::walking(Direction::West)
            ),
            Intent::OffTheMap
        );
    }

    #[test]
    fn step_from_keeps_the_height() {
        // Height is the terrain's business, not the step's.
        let point = Point::new(100, 100, -20);
        assert_eq!(step_from(point, Direction::North), Some(Point::new(100, 99, -20)));
    }

    #[test]
    fn a_walk_around_the_block_returns_home() {
        let mut walker = Walker::new(Point::new(100, 100, 0), Facing::walking(Direction::North));
        let mut sequence = 0u8;
        let mut step = |walker: &mut Walker, direction: Direction| {
            // Two requests per direction: one turns, one moves.
            for _ in 0..2 {
                let _ = walker.request(request(direction, sequence), &OpenWorld, now(), false);
                sequence = sequence.wrapping_add(1);
            }
        };

        step(&mut walker, Direction::East);
        step(&mut walker, Direction::South);
        step(&mut walker, Direction::West);
        step(&mut walker, Direction::North);

        assert_eq!(
            walker.position,
            Point::new(100, 100, 0),
            "four turns and four steps come back to the start"
        );
    }
}
