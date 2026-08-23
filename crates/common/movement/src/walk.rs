//! Turning a walk request into a step, or a refusal.

use std::time::Instant;

use openshard_protocol::direction::{Direction, Facing};
use openshard_protocol::world::{Point, WalkRequest};

use openshard_map::grid::Tile;
use openshard_map::overlay::{Body, Cover, Doors};

use crate::footing::Footing;
use crate::pace::{Pace, WalkPace};
use crate::sequence::WalkSequence;
use crate::terrain::{MAX_STEP_UP, PLAYER_HEIGHT};

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
        footing: &Footing<'_>,
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

        let Intent::Stepped { .. } = intent else {
            // Walked off the edge of the coordinate space. The client cannot
            // express where it wanted to go, so there is nowhere to allow.
            self.sequence.reset();
            return Walk::Refused;
        };

        // `step_allowed` and not `can_step`: the corner rule is half of what a
        // diagonal step means, and this is the last word before a `0x22`.
        // `intend` already produced `target` from this direction, so the two
        // cannot disagree about where the step is going.
        let Some(landed) = step_allowed(footing, self.position, request.facing.direction) else {
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

/// Where a step from `from` onto the tile of `to` lands, or `None` when there
/// is nothing there to stand on.
///
/// **The destination tile alone** — is there ground, does the body fit, is the
/// climb within [`MAX_STEP_UP`], and has the live world put anything in the
/// way. A diagonal has a second half to its answer that this does not give;
/// [`step_allowed`] is the one every caller wants.
///
/// The order the three sources are asked in is the whole of what a footing is
/// for. The map answers first, because it owns the ground. Where it refuses,
/// the overlay may still put a floor there — a deck over open water is the one
/// thing that can overrule a refusal, and no index that only subtracts could
/// say so. Where it *allows*, the overlay may still put a floor **above** the
/// answer: a house's stair tread, its first storey, which is [`climbed`]. Then,
/// at the height the body will actually stand at, the overlay is asked what is
/// in the way: a hull, a wall, a crate, or a shut door that yields only to a
/// route planned by somebody who will open it.
#[must_use]
pub fn can_step(footing: &Footing<'_>, from: Point, to: Point) -> Option<Point> {
    landing(footing, Stance::of(footing, from), to)
}

/// The tile a body is leaving, resolved once.
///
/// Every step out of one tile is measured from the same two heights, and both
/// are properties of that tile and the feet on it — not of where the step is
/// going. A node expansion asks about eight landings (sixteen, counting the
/// diagonals' flanks) from one of them, and re-deriving these per landing is
/// most of what made `8 × step_allowed` cost 1,105 ns where the same eight
/// answers cost 171. See [`steps_out_of`] and
/// `docs/map/navigation_spans.md`'s N3.
#[derive(Clone, Copy, Debug)]
struct Stance {
    /// The feet — ServUO's `startZ`, and what the body's height is measured
    /// from.
    z: i32,
    /// The top of what the *map* says is underfoot: `start_surface`'s second
    /// element, or the feet where there is no map.
    ///
    /// Kept apart from [`top`](Self::top) because the map's landing rule is
    /// asked with the map's own reach: the live world's crests are what a body
    /// *climbs* from, and folding them in here would let a stair tread the
    /// shard placed raise the reach of a step onto the ground beside it.
    map_top: i32,
    /// The same, with the live world's crests folded in — the reach
    /// [`climbed`] measures from.
    top: i32,
}

impl Stance {
    /// Resolve where a body at `from` is standing, on both layers.
    fn of(footing: &Footing<'_>, from: Point) -> Self {
        let z = i32::from(from.z);
        // The *reach*, and never the feet — on a slope, or on a stair, those
        // differ, and starting from the feet refuses the step the client took.
        // The live half is `Cover::crest`: standing half way up a tread, the
        // next step is measured from the top of the whole tread, which is what
        // carries a body off the flight and onto the floor it arrives at.
        let map_top = footing
            .map
            .map_or(z, |map| map.start_surface(from.x, from.y, z).1);
        Self {
            z,
            map_top,
            top: footing
                .overlay
                .surfaces_at(Tile::new(from.x, from.y))
                .filter(|cover| cover.surface() <= z)
                .map(Cover::crest)
                .fold(map_top, i32::max),
        }
    }
}

/// [`can_step`]'s whole rule, with the tile being stepped *off* already
/// resolved.
///
/// The three sources in the order [`can_step`] documents; what is hoisted is
/// only the answer about `from`, which every landing out of one tile shares.
fn landing(footing: &Footing<'_>, stance: Stance, to: Point) -> Option<Point> {
    let tile = Tile::new(to.x, to.y);
    let ground = match footing.map {
        // `land_at` and not `can_step`: the start half is `stance`'s, computed
        // once for every landing out of this tile rather than per call.
        Some(map) => match map.land_at(to, stance.z, stance.map_top) {
            Some(landed) => Some(i32::from(landed.z)),
            // The map says there is nothing to stand on, which over open water
            // is true right up until a ship is moored there.
            None => aboard(footing, stance, to),
        },
        // No map at all: no floor and no walls, so the ground allows everything
        // and only what the live world put there can refuse.
        None => Some(i32::from(to.z)),
    };
    let landed = climbed(footing, stance, tile, ground).or(ground)?;
    let z = i8::try_from(landed).ok()?;
    match footing
        .overlay
        .blocker_at(tile, Body::new(landed, PLAYER_HEIGHT), footing.doors)
    {
        Some(_) => None,
        None => Some(Point::new(to.x, to.y, z)),
    }
}

/// Where a body coming from `from` would land on a surface the live world put
/// at `to`, if it put one there — **and the map put none**.
///
/// The nearest one to where the body already is, which is a deck's rule: you
/// step onto it from a pier and down onto it from a mast, and either way you
/// arrive at the deck. Not [`climbed`]'s rule, because there is nothing to be
/// higher *than*: the map refused, so any surface at all is better than none.
fn aboard(footing: &Footing<'_>, stance: Stance, to: Point) -> Option<i32> {
    if footing.overlay.is_empty() {
        return None;
    }
    footing.overlay.surface_at(Tile::new(to.x, to.y), stance.z)
}

/// The highest surface the live world put at `tile` that a body coming from
/// `from` can reach, fits on, and would gain height by taking.
///
/// **How you get upstairs.** The map answers a house's tiles with the ground
/// under it, because a house is not in the map's files at all — so without this
/// a placed staircase is a picture and its first floor is unreachable, however
/// correctly the overlay describes them.
///
/// Three filters, and each is one of the step rule's own:
///
/// - **In reach.** [`Cover::reach`] against the top of what the body is
///   standing on plus [`MAX_STEP_UP`] — Sphere's and ServUO's `itemTop` against
///   `startTop + 2`. A climbable is met at its base, which is what lets a
///   flight be climbed a tread at a time.
/// - **Worth taking.** Strictly above what the map already answered. A house's
///   ground floor is a platform laid on the ground it duplicates, and this is
///   the filter that makes it a no-op rather than a body lifted a hair.
/// - **Fits.** Nothing of either layer in the body's way there, measured from
///   the height the body walked in at — see [`fits_at`].
///
/// Among what survives, the **highest**, which is Sphere's `GetFixPoint`: a
/// tile can carry both a floor and the stair over it, and the climber takes the
/// stair. Nearest-z would keep a body on the floor while the client climbed.
fn climbed(footing: &Footing<'_>, stance: Stance, tile: Tile, ground: Option<i32>) -> Option<i32> {
    // The hot path's two cheap refusals: a shard with nothing placed on it, and
    // a tile with nothing to climb — one length and one hash.
    if footing.overlay.is_empty() || footing.overlay.surfaces_at(tile).next().is_none() {
        return None;
    }
    let reach = stance.top + MAX_STEP_UP;
    footing
        .overlay
        .surfaces_at(tile)
        .filter(|cover| cover.reach() <= reach)
        .map(Cover::surface)
        .filter(|&z| ground.is_none_or(|ground| z > ground))
        .filter(|&z| fits_at(footing, stance, tile, z))
        .max()
}

/// Whether a body walking in from `from` fits standing at `z` on `tile`, asking
/// both layers.
///
/// The head is measured from where the body *came from* as well as from where
/// it is going — ServUO's `testTop`, and the hole a body falls through without
/// it: a wall that starts above the head of a body standing on the landing is
/// squarely in the way of the same body walking in at the height it left.
fn fits_at(footing: &Footing<'_>, stance: Stance, tile: Tile, z: i32) -> bool {
    let head = (stance.z + PLAYER_HEIGHT).max(z + PLAYER_HEIGHT);
    if footing
        .overlay
        .blocker_at(tile, Body::new(z, head - z), footing.doors)
        .is_some()
    {
        return false;
    }
    footing.map.is_none_or(|map| !map.obstructed(tile, z, head))
}

/// Whether an object `height` tall fits at `tile, z`: nothing solid in its body,
/// and a surface under it to rest on.
///
/// What keeps a generated door in a real doorway — a door belongs in an open gap
/// with a floor, so a spot that is a solid wall or thin air fits nothing.
///
/// **[`Doors::AsTheyStand`] whatever the footing says**, and deliberately: this
/// asks whether a thing *fits*, and a door that could be opened is still a door
/// hanging in the gap. Only a body that will open one may read past it, and a
/// body is not what this places.
#[must_use]
pub fn can_fit(footing: &Footing<'_>, tile: Tile, z: i32, height: i32) -> bool {
    // `height` and not `PLAYER_HEIGHT`: the map half below is already asked
    // about the thing being placed rather than about a person, and the overlay
    // half used to reach for the body constant because the overlay held one.
    // Every caller passes a person's height today, so this is the same answer.
    if footing
        .overlay
        .blocker_at(tile, Body::new(z, height), Doors::AsTheyStand)
        .is_some()
    {
        return false;
    }
    // A surface the live world put at exactly this height is a floor the map
    // does not have, so it answers for the map rather than alongside it.
    if footing.overlay.surface_at(tile, z) == Some(z) {
        return true;
    }
    footing.map.is_none_or(|map| map.can_fit(tile, z, height))
}

/// Whether a straight sight line from `from` to `to` is clear.
///
/// The map's walls, and then the live world's doors. A shut door is opaque; a
/// crate is furniture, not a wall — which is why this asks
/// [`Overlay::blocker_anywhere`](crate::Overlay::blocker_anywhere) and reads
/// only the door flag off what it finds.
#[must_use]
pub fn sight_clear(footing: &Footing<'_>, from: Point, to: Point) -> bool {
    if !footing.map.is_none_or(|map| map.sight_clear(from, to)) {
        return false;
    }
    line_tiles(Tile::new(from.x, from.y), Tile::new(to.x, to.y))
        .into_iter()
        .all(|tile| {
            footing
                .overlay
                .blocker_anywhere(tile)
                .is_none_or(|cover| !cover.is_door())
        })
}

/// Where one *legal* step from `from` lands, or `None` when that step is not a
/// step this world allows at all.
///
/// [`can_step`] answers for the destination tile alone. That is the whole
/// answer for a cardinal and only half of it for a diagonal, which also may not
/// clip the corner where two blockers meet: both cardinal tiles flanking it
/// must themselves be steppable.
///
/// It lives here, above every caller, because the ones that need it are not one
/// layer: [`find_path`](crate::find_path) planning a route, [`Walker::request`]
/// approving a client's `0x02`, the shard's own decree stepping a creature, a
/// chase asking whether the way to its quarry is open, and the client's
/// held-direction detour deciding whether the way ahead is open. **Every one of
/// them goes through here** — there is no longer a bare terrain to ask instead.
/// A client that asked one used to believe a corner-cutting diagonal was
/// walkable, send it, and be rubber-banded: a body stuck against a building
/// corner for as long as the player held that direction.
///
/// **One rule, and it is the strict one.** ServUO keeps two — a player below GM
/// needs both flanks and everything else needs only one, so its creatures cut
/// corners its players cannot. This engine gives everybody the player's rule,
/// which is also the rule the baked graph and every plan are made of. The
/// divergence is deliberate and argued in `docs/map/navigation_spans.md`'s
/// *Out of scope, named*; if the lax reading is ever wanted, this is where it
/// goes.
///
/// **One direction of [`steps_out_of`], and it costs the whole expansion.**
/// That is deliberate: the corner rule already made a diagonal ask about three
/// tiles, and the alternative is a second copy of the step rule that can drift
/// from the one a search uses. A caller asking about every direction — a
/// search, the detour's four tiles — asks [`steps_out_of`] once instead.
#[must_use]
pub fn step_allowed(footing: &Footing<'_>, from: Point, direction: Direction) -> Option<Point> {
    steps_out_of(footing, from)[direction.to_bits() as usize]
}

/// Every legal step out of `from`, indexed by
/// [`Direction::to_bits`] — which is [`Direction::ALL`]'s own order.
///
/// **One node expansion, and the primitive [`step_allowed`] is one reading
/// of.** A search asks about all eight neighbours of every tile it pops, and
/// asking them one at a time pays for the same two things over and over: the
/// tile being stepped *off*, resolved on every call from the same point, and
/// the four cardinal neighbours, each of which is asked about once as a
/// destination and again as some diagonal's flank. Answering the eight together
/// pays for each once — sixteen landing checks become eight — and it is why
/// this, rather than [`step_allowed`], is what [`find_path`](crate::find_path)
/// calls.
///
/// The answers are the same answers: `step_allowed` is *defined* as one slot of
/// this, so there is one rule here and no second one to drift.
#[must_use]
pub fn steps_out_of(footing: &Footing<'_>, from: Point) -> [Option<Point>; 8] {
    let stance = Stance::of(footing, from);
    // Every neighbour once, cardinals included — a diagonal's flank rule below
    // reads these rather than asking again.
    let mut landings = [None; 8];
    for direction in Direction::ALL {
        let Some(to) = step_from(from, direction) else {
            continue;
        };
        landings[direction.to_bits() as usize] = landing(footing, stance, to);
    }
    let mut allowed = landings;
    for direction in Direction::ALL {
        // A diagonal may not clip the corner where two blockers meet: both
        // cardinal tiles flanking it must themselves be steppable. The flanks of
        // a diagonal are the two wire directions either side of it (NE lies
        // between N and E), and both are cardinal — so this reads only landings
        // and can never re-enter the rule.
        let bits = usize::from(direction.to_bits());
        if direction.is_diagonal()
            && [(bits + 7) % 8, (bits + 1) % 8]
                .iter()
                .any(|&flank| landings[flank].is_none())
        {
            allowed[bits] = None;
        }
    }
    allowed
}

#[cfg(test)]
mod tests {
    use openshard_protocol::world::{RawFastwalkKey, RawStepSequence};

    use super::*;
    use openshard_map::overlay::{Cover, Overlay};

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

    /// A world with no floor and no walls: every step is allowed, z never
    /// changes. What a shard with no client files runs, and what these tests
    /// run against — it used to be a type of its own (`OpenWorld`) and an
    /// implementor of the trait, which is how the absence of a map came to be
    /// a kind of map.
    fn open_world() -> Overlay {
        Overlay::default()
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
        let outcome = walker.request(
            request(Direction::North, 0),
            &Footing::new(None, &open_world(), Doors::AsTheyStand),
            now(),
            false,
        );
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
        let outcome = walker.request(
            request(Direction::East, 0),
            &Footing::new(None, &open_world(), Doors::AsTheyStand),
            now(),
            false,
        );
        assert_eq!(
            outcome,
            Walk::Turned {
                facing: Facing::walking(Direction::East)
            }
        );
        assert_eq!(walker.position, Point::new(100, 100, 0), "did not move");

        // Now it moves.
        let outcome = walker.request(
            request(Direction::East, 1),
            &Footing::new(None, &open_world(), Doors::AsTheyStand),
            now(),
            false,
        );
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
        let _ = walker.request(
            request(Direction::East, 0),
            &Footing::new(None, &open_world(), Doors::AsTheyStand),
            now(),
            false,
        );
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
            &Footing::new(None, &open_world(), Doors::AsTheyStand),
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
            let outcome = walker.request(
                request(direction, 0),
                &Footing::new(None, &open_world(), Doors::AsTheyStand),
                now(),
                false,
            );

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
            walker.request(
                request(Direction::North, 5),
                &Footing::new(None, &open_world(), Doors::AsTheyStand),
                now(),
                false
            ),
            Walk::Refused
        );
        assert_eq!(walker.position, Point::new(100, 100, 0), "did not move");
        assert!(walker.sequence.is_fresh(), "and stays fresh");
    }

    #[test]
    fn a_refusal_resets_the_sequence() {
        let mut walker = walker();
        let _ = walker.request(
            request(Direction::North, 0),
            &Footing::new(None, &open_world(), Doors::AsTheyStand),
            now(),
            false,
        );
        let _ = walker.request(
            request(Direction::North, 1),
            &Footing::new(None, &open_world(), Doors::AsTheyStand),
            now(),
            false,
        );

        // A wall, due north of where those two steps left the walker.
        let mut wall = Overlay::default();
        wall.set(Tile::new(100, 97), vec![Cover::blocking(0, 20)]);

        assert_eq!(
            walker.request(
                request(Direction::North, 2),
                &Footing::new(None, &wall, Doors::AsTheyStand),
                now(),
                false
            ),
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
        // A step onto a low platform lands you higher than you asked for. The
        // climb is within `MAX_STEP_UP`, because a real map is what answers now
        // and a real map refuses more than that — the double this replaced
        // could lift a body five units in one step, which no ground does.
        let mut hill = crate::scene::Scene::flat_holding(100, 100, 0);
        hill.floor(100, 99, 0, 2);
        let mut walker = walker();
        let outcome = walker.request(request(Direction::North, 0), &hill.footing(), now(), false);
        assert_eq!(
            outcome,
            Walk::Moved {
                position: Point::new(100, 99, 2),
                facing: Facing::walking(Direction::North),
            }
        );
        assert_eq!(walker.position.z, 2, "the walker believes the terrain");
    }

    #[test]
    fn the_world_edge_refuses_rather_than_wrapping() {
        // A step west from x=0 has no u16 to land on. Wrapping would put the
        // walker at x=65535 — the far side of the map, instantly.
        let mut walker = Walker::new(Point::new(0, 0, 0), Facing::walking(Direction::West));
        assert_eq!(
            walker.request(
                request(Direction::West, 0),
                &Footing::new(None, &open_world(), Doors::AsTheyStand),
                now(),
                false
            ),
            Walk::Refused
        );
        assert_eq!(walker.position, Point::new(0, 0, 0));

        let mut walker = Walker::new(
            Point::new(u16::MAX, u16::MAX, 0),
            Facing::walking(Direction::SouthEast),
        );
        assert_eq!(
            walker.request(
                request(Direction::SouthEast, 0),
                &Footing::new(None, &open_world(), Doors::AsTheyStand),
                now(),
                false
            ),
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
                let _ = walker.request(
                    request(direction, sequence),
                    &Footing::new(None, &open_world(), Doors::AsTheyStand),
                    now(),
                    false,
                );
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
