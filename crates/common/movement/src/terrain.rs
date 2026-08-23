//! Whether a mobile can actually stand somewhere.
//!
//! Lives beside [`find_path`](crate::find_path) rather than in `openshard-world`
//! because both a server tick and a client's own click-to-walk planner need the
//! same answer, and a client may not depend on `openshard-world` — that crate
//! drags in the whole gameplay stack. `MapTerrain` reads nothing but a `WorldMap` and
//! a `TileData`, both of which the client already loads to draw the screen, so
//! this is the one piece of `Terrain` two very different callers can share
//! byte-for-byte. What stays server-side is `openshard-state::obstruct`'s
//! `Obstructions` — the dynamic half, doors and placed items, which needs the
//! entity registry a client does not have.

use crate::Tile;
use openshard_map::map::{LandTile, WorldMap};
use openshard_protocol::wire::Graphic;
use openshard_protocol::world::Point;
use openshard_tiles::{TileData, TileFlags};

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

/// The real world: ground heights, walls, water.
///
/// **Three borrows and a flag, built where it is asked.** Both ends already own
/// the two tables it reads — the server as `FacetState`'s snapshot and
/// `WorldState::tiles`, the client as `Resources` — so this is a view over what
/// the caller already has rather than a thing anybody stores. It used to be
/// generic over how the map and the tile data were *held*, which bought exactly
/// one property: a terrain with no lifetime, so it could be boxed in a struct
/// field. Nothing is boxed any more; see `docs/map/terrain_seam.md`'s node D.
///
/// `Copy`, because it is two pointers and a bool: an overlay that wants to keep
/// one beside its own data copies it rather than borrowing the borrow.
#[derive(Clone, Copy, Debug)]
pub struct MapTerrain<'a> {
    map: &'a WorldMap,
    tiles: &'a TileData,
    /// Whether water counts as ground. A boat or a fish says yes.
    ///
    /// A property of the *body* asking, which is why it is set on the view and
    /// not on the world: one facet has one map and many creatures over it, and a
    /// field on the map would turn the swimming on for all of them. Built as
    /// `false` and turned on by the query that is asking for a swimmer — see
    /// [`swimming`](Self::swimming).
    swimming: bool,
}

impl<'a> MapTerrain<'a> {
    /// Read a loaded map through the table that says what its graphics are.
    pub const fn new(map: &'a WorldMap, tiles: &'a TileData) -> Self {
        Self {
            map,
            tiles,
            swimming: false,
        }
    }

    /// Ask as something that swims: water becomes ground.
    pub const fn swimming(mut self, swimming: bool) -> Self {
        self.swimming = swimming;
        self
    }

    /// The map.
    pub const fn map(&self) -> &'a WorldMap {
        self.map
    }

    /// The tile definitions.
    pub const fn tiles(&self) -> &'a TileData {
        self.tiles
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

    /// A guess at the height a mobile stands at on `(x, y)`, coming from
    /// `near_z`, that never refuses.
    ///
    /// For prediction, not for deciding whether a step is allowed: `check`
    /// answers "how high, if this is reachable at all" and returns `None` when
    /// it is not, which is right for the server and wrong for a client that
    /// must draw *something* the instant a key is pressed, before the server
    /// has said whether the step is real (see `client/net`'s `Walk::step`, the
    /// only caller). The candidates are `spawn_z`'s — the ground, and the stand
    /// height of every platform static on the tile, a pier or a bridge among
    /// them — without `spawn_z`'s `can_fit` filter, which exists to place a
    /// spawn on a surface a body actually fits under and would drop a
    /// candidate rather than approximate one. Picks whichever is nearest
    /// `near_z`; an empty tile — off the map, no land cell — answers `near_z`
    /// itself, so a step onto ground this end knows nothing about keeps the
    /// height it came from rather than guessing zero.
    pub fn predict_z(&self, x: u16, y: u16, near_z: i32) -> i32 {
        self.surfaces(x, y)
            .into_iter()
            .min_by_key(|&z| (z - near_z).abs())
            .unwrap_or(near_z)
    }

    /// Every height a body could stand at on `(x, y)`: the ground, when the land
    /// there is ground at all, and the stand height of each platform static —
    /// a pier's planks, a stair's tread, the floor of the storey above.
    ///
    /// The candidate list [`predict_z`](Self::predict_z) and `spawn_z` both
    /// choose from, and one list rather than two copies of it: a surface that
    /// exists for the prediction and not for the placement is a body drawn
    /// where it will not be put.
    ///
    /// **Candidates, not permissions.** Nothing here has been asked whether a
    /// body *fits* — that is [`can_fit`](crate::Terrain::can_fit), and the
    /// callers that care apply it. Unordered, and the tile's own order at that.
    pub fn surfaces(&self, x: u16, y: u16) -> Vec<i32> {
        openshard_uofiles::surfaces::stand_surfaces(self.map(), self.tiles(), x, y, self.swimming)
    }

    /// How high the tile's contents reach — the top of the tallest static on it,
    /// and `None` where nothing stands.
    ///
    /// A roof over a room, the cap of a wall, the crown of a tree: this does not
    /// distinguish them, because what it is for is the vertical extent a tile
    /// occupies rather than the meaning of the thing occupying it.
    ///
    /// The same top [`is_obstructed`](Self::is_obstructed) measures a static by,
    /// and deliberately so: a diagram drawn from a second arithmetic would show
    /// a ceiling the step rule does not believe in.
    pub fn ceiling(&self, x: u16, y: u16) -> Option<i32> {
        self.map()
            .statics_at(x, y)
            .map(|item| self.static_top(item.tile, i32::from(item.z)))
            .max()
    }

    /// How high one static reaches from its base.
    ///
    /// A platform's is where a body stands on it — halved for a climbable
    /// bridge, the same `platform_surface` the step check picks candidates with,
    /// so the surface underfoot tops out exactly at the feet on it. Anything
    /// else is its art, and at least one unit: walls often carry zero height in
    /// tiledata, and a zero-tall wall that blocks nothing is not a wall.
    fn static_top(&self, tile: Graphic, base: i32) -> i32 {
        let tile = self.tiles().static_tile(tile.0);
        match tile.flags.is_platform() {
            true => platform_surface(base, i32::from(tile.height), tile.flags.is_climbable()).1,
            false => base + i32::from(tile.height).max(1),
        }
    }

    /// The height a mobile stepping from `from` onto `(x, y)` lands at — the
    /// prediction a client draws its own body at, and the one number that has to
    /// be the server's.
    ///
    /// This is [`check`](Self::check) — the *whole* step rule, reached from the
    /// top of the surface underfoot, standing on the highest surface within a
    /// step — and not [`predict_z`](Self::predict_z), which knows only a tile and
    /// a rough height and therefore picks the surface *nearest* that height. On
    /// bare ground the two agree. On a staircase they cannot: a stair tile carries
    /// the floor below it and the step above, the server climbs to the step
    /// (`check`'s `GetFixPoint` rule) and the nearest-z guess stays on the floor —
    /// so a body walked *through* the staircase at ground level while the shard
    /// had it half way up, and nothing said so, because a `0x22` carries no
    /// position and only a `0x20` would ever have corrected it.
    ///
    /// Still never a refusal, which is the contract every caller here relies on:
    /// where `check` says the step is impossible this falls back to
    /// [`predict_z`](Self::predict_z)'s guess rather than returning nothing.
    /// Whether a step is allowed is the server's answer and arrives as a `0x21`;
    /// this end only has to draw the body somewhere sane until it does.
    pub fn predict_step(&self, from: Point, x: u16, y: u16) -> i32 {
        let from_z = i32::from(from.z);
        let (_, start_top) = self.start_surface(from.x, from.y, from_z);
        self.check(x, y, from_z, start_top)
            .unwrap_or_else(|| self.predict_z(x, y, from_z))
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
    pub fn start_surface(&self, x: u16, y: u16, loc_z: i32) -> (i32, i32) {
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
        for item in self.map().statics_at(x, y) {
            let tile = self.tiles().static_tile(item.tile.0);
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

    /// Whether a mobile whose feet are at `start_z`, standing on a surface
    /// topping out at `start_top`, may step onto `(x, y)` — and the height it
    /// lands at.
    ///
    /// A blend of the three implementations, because the shard serves the 2D
    /// client and each got a different part of this right for it:
    ///
    /// - **Reach** is ServUO/RunUO's: a step reaches `start_top + 2` — the top of
    ///   the surface underfoot plus a step, not the feet. Starting from the feet
    ///   refuses steps up a slope the client took, which is what rubber-banded
    ///   every hillside before this.
    /// - **The body** is the client's own: it stands `PLAYER_HEIGHT` above
    ///   `start_z`, and `start_z` is the mobile's *feet*. ServUO measures it from
    ///   the base of the surface underfoot instead (`GetStartZ`'s `zLow` into
    ///   `checkTop`), which for anything thicker than a paving stone puts the
    ///   body below where it is standing: on a floor twenty tall the shard
    ///   thought the head was at the knees, and a wall four units below the feet
    ///   on the next tile was something to fall past rather than walk into.
    ///   ClassicUO's `CalculateMinMaxZ` seeds `minZ` from the surface's
    ///   *averageZ* — the height a body stands at — and refuses that step, which
    ///   makes ServUO's version a rubber-band even where it is not a fall.
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
        self.map().land(x, y)?;
        let (land_z, land_center, _) = self.land_heights(x, y);
        // How high a step reaches, and the headroom a body needs above its feet.
        let step_top = start_top + MAX_STEP_UP;
        let check_top = start_z + PLAYER_HEIGHT;

        let mut new_z = 0;
        let mut move_ok = false;

        for item in self.map().statics_at(x, y) {
            let tile = self.tiles().static_tile(item.tile.0);
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
                // `test_top`, not `our_z + PLAYER_HEIGHT`: the body has to *get*
                // to this surface, and it walks in at the height it left. See
                // `is_obstructed`.
                if !self.is_obstructed(x, y, our_z, test_top) {
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
            && !self.is_obstructed(x, y, land_center, check_top.max(land_center + PLAYER_HEIGHT))
        {
            new_z = land_center;
            move_ok = true;
        }

        move_ok.then_some(new_z)
    }

    /// Whether the land at `(x, y)` is something a mobile can stand on: not water
    /// it cannot swim in, and not flagged impassable.
    fn land_is_ground(&self, x: u16, y: u16) -> bool {
        let Some(land) = self.map().land(x, y) else {
            return false;
        };
        let flags = self.tiles().land(land.tile.0).flags;
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
        // The corner walk and the average both live on the map, because the
        // client has to compute the very same numbers: the walk ack carries no
        // `z`, so each end lands its own step, and two formulas that agree today
        // are two formulas. Off the map there is no tile and no relief.
        let corners = self.map().land_corners(x, y).unwrap_or([0; 4]);
        let avg = i32::from(openshard_map::map::average_corner_z(corners));
        let [top, right, left, bottom] = corners.map(i32::from);
        let min = top.min(left).min(right).min(bottom);
        let max = top.max(left).max(right).max(bottom);
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

    /// Whether anything on this tile would be in a mobile's way standing at `z`
    /// with its head at `top`.
    ///
    /// A static blocks if its body overlaps the space the mobile occupies —
    /// `z` to `top`. A wall whose top is below your feet is a step, not an
    /// obstacle, and one whose base is above your head is a ceiling.
    ///
    /// `top` is a parameter rather than `z + PLAYER_HEIGHT` because a body has to
    /// *get* to `z`, and it walks in at the height it left: ServUO passes
    /// `IsOk` a `testTop` of `max(startZ, ourZ) + PersonHeight`, and dropping the
    /// `startZ` half is a hole a body falls through. Britain's castle again — the
    /// terrace at z=40 with a stairwell beside it whose bottom step stands at
    /// z=22, under a wall spanning 40 to 49. Measured from the landing alone the
    /// body is 22 to 38 and the wall starts above its head, so the step read as
    /// open and walking north off the terrace dropped eighteen units into the
    /// stairwell — through a wall that is at eye level on the way in. Measured
    /// from where the body *came from* the wall is squarely in it. The tile one
    /// west is the same stairwell one step higher, and it blocked either way,
    /// which is what "there is a wall to the left of it and a hole to the right"
    /// was.
    ///
    /// **A surface counts too**, not only a wall: ServUO's `Movement.IsOk` tests
    /// `Impassable | Surface` together, and a stair, a stone plinth or an upper
    /// floor is exactly as solid to a body standing beside it as a wall is. The
    /// reason it is not self-blocking is the *top*: a surface you are standing on
    /// has its top at your feet, and `checkTop > ourZ` is false there — which is
    /// why the height compared here is the stand height (halved for a climbable
    /// bridge, ServUO's `CalcHeight`), not the art's full extent.
    ///
    /// Exempting surfaces outright — what this used to do — is two bugs at once,
    /// and both were visible in the client. A staircase could be walked *into*
    /// from the side, because the floor under it was an unobstructed candidate.
    /// And a body could *fall*: at (1409, 1713) a step east off the castle stair
    /// landed on the land at z=20, seventeen units down and directly underneath a
    /// stack of stone blocks, because the blocks were surfaces and so waved
    /// through. With them in the way there is no standable height on that tile at
    /// all, and the step is refused — which is what the client does.
    fn is_obstructed(&self, x: u16, y: u16, z: i32, top: i32) -> bool {
        self.map().statics_at(x, y).any(|item| {
            let tile = self.tiles().static_tile(item.tile.0);
            let platform = tile.flags.is_platform();
            if !tile.flags.is_blocking() && !platform {
                return false;
            }
            // Note what is *not* here: `UFLAG2_WINDOW`. Sphere's own comment on
            // that bit says "can walk thru it", but Sphere never once reads it in
            // `CWorldMap` — the only uses in the whole engine are three
            // line-of-sight tests in `CCharLOS.cpp`, and even those are gated on
            // `LOS_NB_WINDOWS`. ServUO's `Movement.Check` blocks on
            // `Impassable | Surface` with no window exemption either. So a window
            // is a hole for a *look*, never for a *step* — see `sight_clear`,
            // which is where the flag belongs and is still honoured.
            //
            // Exempting it here let anything the server moved walk through every
            // wall segment with a window in it. It never showed for a player,
            // because the client refuses the step before it is ever sent; it
            // showed for townsfolk walking home at night, which is the only end
            // of this rule nobody was watching.
            let bottom = i32::from(item.z);
            // See `static_top`: a surface's top is where a body stands on it, so
            // the one you are standing on does not block you, and a wall's is its
            // art.
            let item_top = self.static_top(item.tile, bottom);
            // Overlap between the static's [bottom, item_top) and the body's
            // [z, top).
            bottom < top && z < item_top
        })
    }

    /// Whether an object `height` tall fits at `at` and `z` with a surface under it.
    ///
    /// The same shape as [`Terrain::can_fit`](crate::Terrain::can_fit) on
    /// purpose: an inherent method that shadows a trait one of the *same* name
    /// and a *different* arity is a trap, and it caught two callers — one wrote
    /// `Terrain::can_fit(..)` in full to get past it, and this file wrote
    /// `MapTerrain::can_fit(self, tile.x, tile.y, ..)`. With one shape, whichever
    /// a caller reaches is the same answer.
    ///
    /// ServUO's `Map.CanFit` for a static-only world (`checkBlocksFit` and
    /// `checkMobiles` off, `requireSurface` on): a **surface or impassable** tile
    /// whose body overlaps `[z, z + height)` blocks — note *surface*, so a floor or
    /// platform planted through the door's body counts, not only a wall — and there
    /// must be a surface (the land, or a static floor) exactly at `z` to rest on.
    /// This is stricter than [`is_obstructed`](Self::is_obstructed), which only asks
    /// about walls in the way; door generation needs both halves, or it drops doors
    /// into wooden walls that read as frames but have no doorway.
    pub fn can_fit(&self, at: Tile, z: i32, height: i32) -> bool {
        let (low_z, avg_z, _top) = self.land_heights(at.x, at.y);
        let land_flags = self
            .map()
            .land(at.x, at.y)
            .map(|cell| self.tiles().land(cell.tile.0).flags);
        let land_impassable = land_flags.is_some_and(|f| f.is_blocking());
        // Impassable land (water, a mountain) in the object's body blocks it.
        if land_impassable && avg_z > z && z + height > low_z {
            return false;
        }
        // Passable land you sit exactly on is a surface to stand on.
        let mut has_surface = land_flags.is_some() && !land_impassable && z == avg_z;

        for item in self.map().statics_at(at.x, at.y) {
            let tile = self.tiles().static_tile(item.tile.0);
            let surface = tile.flags.is_platform();
            let impassable = tile.flags.is_blocking();
            let base = i32::from(item.z);
            // `calc_top` is the tile's top for fit purposes — halved for a bridge
            // (stairs), the same `CalcHeight` the client uses; `platform_surface`
            // already encodes that.
            let (_, calc_top) = platform_surface(base, i32::from(tile.height), tile.flags.is_climbable());
            if (surface || impassable) && calc_top > z && z + height > base {
                return false;
            }
            if surface && !impassable && z == calc_top {
                has_surface = true;
            }
        }
        has_surface
    }
}

/// The map's own answers — the nine questions a terrain trait used to hold, as
/// inherent methods on the one type that could answer them from a map.
///
/// Five of that trait's six implementors were an *action over* a terrain rather
/// than a terrain, and the trait existed so a search could take any of them. It
/// does not any more: a search takes a [`Footing`](crate::Footing), and a caller
/// that wants the bare map takes the map. See `docs/map/terrain_seam.md`.
impl MapTerrain<'_> {
    pub fn land_is_water(&self, tile: Tile) -> bool {
        self.map()
            .land(tile.x, tile.y)
            .is_some_and(|land| self.tiles().land(land.tile.0).flags.is_water())
    }

    pub fn can_step(&self, from: Point, to: Point) -> Option<Point> {
        let from_z = i32::from(from.z);
        // Reach the next tile from the top of what we stand on, not from our feet:
        // on a slope those differ, and starting from the feet refuses steps up the
        // slope the client took. `start_surface` is what the client reaches from.
        // The body is measured from `from_z` — its feet — and only the *reach*
        // comes from the surface underfoot. `start_surface`'s other half, the
        // base that surface stands on, is ServUO's `checkTop` and is the one
        // thing in this port the 2D client disagrees with; see `check`.
        let (_, start_top) = self.start_surface(from.x, from.y, from_z);
        let landing = self.check(to.x, to.y, from_z, start_top)?;
        let z = i8::try_from(landing).ok()?;
        Some(Point { x: to.x, y: to.y, z })
    }

    pub fn ground_z(&self, tile: Tile) -> Option<i8> {
        // The average, not the raw corner, so a character spawns at the same
        // height the client will compute for the tile — see `average_land_z`.
        self.map().land(tile.x, tile.y)?;
        i8::try_from(self.average_land_z(tile.x, tile.y)).ok()
    }

    pub fn land_tile(&self, tile: Tile) -> Option<LandTile> {
        self.map().land(tile.x, tile.y).map(|cell| cell.tile)
    }

    pub fn statics_at(&self, tile: Tile, out: &mut Vec<(Graphic, i8)>) {
        // `tile` is the static's graphic id — for statics it is the item graphic
        // itself, which is what the door-frame tables match against.
        out.extend(
            self.map()
                .statics_at(tile.x, tile.y)
                .map(|item| (item.tile, item.z)),
        );
    }

    pub fn stand_z(&self, tile: Tile, near_z: i32) -> Option<i32> {
        self.surface_at(tile.x, tile.y, near_z)
    }

    pub fn spawn_z(&self, tile: Tile, near_z: i32) -> Option<i32> {
        // First the ordinary step check: from a ground-level placement it finds the
        // ground floor and — crucially — cannot reach the storey above, so a banker
        // placed at z=0 stays on the bank's ground floor rather than climbing to the
        // second. Only when nothing is within a step (a shop's *raised* floor placed
        // at ground level, the tailor) do we look further.
        if let Some(z) = self.surface_at(tile.x, tile.y, near_z) {
            return Some(z);
        }

        // Every surface a mobile could stand on here: the ground, and the top of
        // each platform static. Unlike a step, this placement is not bound by reach —
        // a shop floor several tiles above the ground is still where the NPC goes.
        //
        // Keep only the surfaces a body actually fits on — a floor below and
        // headroom above — so the ground *under* a covering floor drops out (the
        // body would poke through it) and the floor itself is chosen. Among those,
        // the one nearest the requested height.
        self.surfaces(tile.x, tile.y)
            .into_iter()
            .filter(|&z| MapTerrain::can_fit(self, tile, z, PLAYER_HEIGHT))
            .min_by_key(|&z| (z - near_z).abs())
    }

    pub fn sight_clear(&self, from: Point, to: Point) -> bool {
        // Eye height: the ray runs at head level, interpolated between the two
        // ends so a look up a hill follows the slope.
        const EYE: i32 = 9;
        let tiles = crate::line_tiles(Tile::new(from.x, from.y), Tile::new(to.x, to.y));
        let count = tiles.len() as i32;
        for (i, tile) in tiles.into_iter().enumerate() {
            let t = i as i32 + 1;
            let ray_z = i32::from(from.z) + (i32::from(to.z) - i32::from(from.z)) * t / (count + 1) + EYE;
            // A hill in the way: ground above the eye line occludes.
            if self
                .ground_z(tile)
                .is_some_and(|ground| i32::from(ground) > ray_z)
            {
                return false;
            }
            for item in self.map().statics_at(tile.x, tile.y) {
                let static_tile = self.tiles().static_tile(item.tile.0);
                let flags = static_tile.flags;
                // Windows are the deliberate hole in a wall — Sphere's
                // `LOS_NB_WINDOWS`, and the one place `UFLAG2_WINDOW` is read at
                // all. A look passes; a step does not (see `is_obstructed`).
                if flags.has(TileFlags::WINDOW) {
                    continue;
                }
                // Sphere's `CCharLOS.cpp:400`: a static blocks sight if it is
                // `UFLAG1_WALL | UFLAG1_BLOCK | UFLAG2_PLATFORM`. The platform bit
                // is not an oversight there — an upper floor is exactly what stops
                // you seeing the storey above you. `NO_SHOOT` is ServUO's name for
                // `UFLAG2_WALL2` and covers the grilles and bars a look does not
                // cross either, which is why a monster does not aggro through a
                // portcullis it can never reach through.
                const WALLISH: u64 = TileFlags::WALL | TileFlags::BLOCK | TileFlags::NO_SHOOT;
                let wallish = flags.has(WALLISH);
                if !wallish && !flags.is_platform() {
                    continue;
                }
                let base = i32::from(item.z);
                // Walls often carry zero height in tiledata; treat them as a full
                // storey, the way the client draws them. A *platform* keeps its
                // real height, and the difference is not academic: a floor tile is
                // height 0, and lending it a storey walls off every doorway it is
                // laid in — which is how "an open doorway is a sight line" broke.
                let top = if wallish {
                    base + i32::from(static_tile.height.max(15))
                } else {
                    base + i32::from(static_tile.height)
                };
                if base <= ray_z && ray_z < top {
                    return false;
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openshard_protocol::direction::Direction;

    #[test]
    fn predict_z_answers_near_z_off_the_map_or_with_nothing_on_the_tile() {
        let Some(install) = real_install() else {
            return;
        };
        let t = install.terrain();
        assert_eq!(t.predict_z(7168, 0, 42), 42, "past the map edge");
    }

    #[test]
    fn predict_z_picks_a_platform_over_the_land_beneath_it() {
        // The bug this exists to fix: `check`'s `landCheck` guard may discard a
        // pier or bridge static as a step candidate when the land under it reads
        // close in height — but `predict_z` is not `check`, it never refuses, and
        // asking from the static's own height must not silently fall back to the
        // land under it, or a client predicting a walk onto a pier lands its body
        // at the water instead of the deck.
        let Some(install) = real_install() else {
            return;
        };
        let t = install.terrain();

        let mut checked = 0;
        for y in 1580..1610u16 {
            for x in 1490..1552u16 {
                if t.map().land(x, y).is_none() {
                    continue;
                }
                let (_, land_center, _) = t.land_heights(x, y);
                for item in t.map().statics_at(x, y) {
                    let tile = t.tiles().static_tile(item.tile.0);
                    if !tile.flags.is_platform() {
                        continue;
                    }
                    let (_, our_z) = platform_surface(
                        i32::from(item.z),
                        i32::from(tile.height),
                        tile.flags.is_climbable(),
                    );
                    if our_z == land_center {
                        continue;
                    }
                    assert_eq!(
                        t.predict_z(x, y, our_z),
                        our_z,
                        "({x},{y}) asked from a static's own height {our_z} but landed elsewhere",
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 5, "only {checked} static surfaces tested");
    }

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

    /// What an install owns, so a test can hold it and hand out views of it.
    ///
    /// A `MapTerrain` borrows both tables now, so something has to own them for
    /// longer than one expression. On the shard that is `FacetState` and
    /// `WorldState`; here it is this, which is the same arrangement written
    /// small — the fixture owning the tables and the rule reading them.
    struct Install {
        map: WorldMap,
        tiles: TileData,
    }

    impl Install {
        /// A walker's view of the install.
        fn terrain(&self) -> MapTerrain<'_> {
            MapTerrain::new(&self.map, &self.tiles)
        }

        /// A swimmer's view of the same install: one map, two bodies asking.
        fn swimming(&self) -> MapTerrain<'_> {
            self.terrain().swimming(true)
        }
    }

    fn real_install() -> Option<Install> {
        let dir = client_dir()?;
        let map = openshard_uofiles::map::read_facet(&dir, 0).expect("the client's map0 should load");
        let tiles = openshard_uofiles::tiledata::load(dir.join("tiledata.mul"))
            .expect("tiledata should load")
            .tiles;
        Some(Install { map, tiles })
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
        let Some(install) = real_install() else {
            return;
        };
        let t = install.terrain();

        let mut checked = 0;
        for y in 1580..1610u16 {
            for x in 1490..1552u16 {
                // Collect the standing heights of the platform statics here.
                let mut stands: Vec<i32> = t
                    .map()
                    .statics_at(x, y)
                    .filter_map(|item| {
                        let tile = t.tiles().static_tile(item.tile.0);
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
                if low == high || t.is_obstructed(x, y, high, high + PLAYER_HEIGHT) {
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

    /// The candidate list is the one the predictions are made from, and a tile's
    /// ceiling is the statics on it.
    ///
    /// `surfaces` was factored out of `predict_z` and `spawn_z`, which each held
    /// their own copy of it; this is what says the list did not drift while being
    /// shared. Every height `predict_z` can answer with must be a member of it —
    /// a prediction outside the list is a body drawn at a height nothing offers.
    #[test]
    fn a_predicted_height_is_one_of_the_tile_s_own_surfaces() {
        let Some(install) = real_install() else {
            return;
        };
        let t = install.terrain();
        let mut checked = 0;
        let mut roofed = 0;
        for y in 1580..1610u16 {
            for x in 1490..1552u16 {
                let surfaces = t.surfaces(x, y);
                let ceiling = t.ceiling(x, y);
                // A ceiling is the statics and nothing else: an empty tile has
                // none, and a tile with anything on it has one.
                assert_eq!(
                    ceiling.is_some(),
                    t.map().statics_at(x, y).next().is_some(),
                    "({x},{y}) disagrees about whether anything stands on it",
                );
                if let Some(ceiling) = ceiling {
                    let lowest = t.map().statics_at(x, y).map(|item| i32::from(item.z)).min();
                    assert!(
                        ceiling >= lowest.unwrap(),
                        "({x},{y}) tops out at {ceiling}, below its own lowest static",
                    );
                    // Something standing higher than anything standable is what
                    // the marker draws its lid from — a roof, a wall's cap.
                    if surfaces.iter().max().is_some_and(|&top| ceiling > top) {
                        roofed += 1;
                    }
                }
                if surfaces.is_empty() {
                    continue;
                }
                // From well below, from the ground, and from well above: the
                // nearest candidate differs at each, and every one of them has
                // to come out of the list.
                for near in [-60, 0, 20, 80] {
                    let predicted = t.predict_z(x, y, near);
                    assert!(
                        surfaces.contains(&predicted),
                        "({x},{y}) from {near} predicted {predicted}, which is not among {surfaces:?}",
                    );
                }
                checked += 1;
            }
        }
        assert!(checked > 100, "only {checked} tiles with a surface tested");
        assert!(
            roofed > 0,
            "no tile in the sweep had anything above its top surface"
        );
    }

    /// A client predicting a step must land where the shard lands it, and on a
    /// staircase that is the *step*, not the floor the same tile carries.
    ///
    /// The bug: `predict_z` picks the surface nearest the height a body already
    /// has, `check` stands on the highest surface within a step, and a stair tile
    /// carries both. So the shard climbed, the client stayed on the floor, and
    /// since a `0x22` carries no position nothing corrected it — the body walked
    /// *through* the staircase. `predict_step` is `check`, which is why the two
    /// now agree; the `disagreed` counter is what proves the test is not green
    /// because the two rules happen to answer the same everywhere.
    #[test]
    fn a_client_predicting_a_step_climbs_the_stairs_the_shard_climbs() {
        let Some(install) = real_install() else {
            return;
        };
        let t = install.terrain();

        let mut checked = 0;
        let mut disagreed = 0;
        for y in 1500..1900u16 {
            for x in 1350..1650u16 {
                let stair = t.map().statics_at(x, y).any(|item| {
                    let tile = t.tiles().static_tile(item.tile.0);
                    tile.flags.is_platform() && tile.flags.is_climbable()
                });
                if !stair {
                    continue;
                }
                for dir in Direction::ALL {
                    let (dx, dy) = dir.step();
                    let (nx, ny) = ((i32::from(x) + dx) as u16, (i32::from(y) + dy) as u16);
                    // Stand on the neighbour first: a step is only meaningful
                    // from a surface a body could actually be on.
                    let Some(stand) = t.surface_at(nx, ny, 0) else {
                        continue;
                    };
                    let Ok(stand) = i8::try_from(stand) else {
                        continue;
                    };
                    let from = Point::new(nx, ny, stand);
                    let Some(landed) = t.can_step(from, Point::new(x, y, stand)) else {
                        continue;
                    };
                    assert_eq!(
                        i8::try_from(t.predict_step(from, x, y)).ok(),
                        Some(landed.z),
                        "({nx},{ny},{stand}) -{dir:?}-> ({x},{y}): the client predicted a \
                         different height than the shard landed the step at",
                    );
                    checked += 1;
                    if i32::from(landed.z) != t.predict_z(x, y, i32::from(stand)) {
                        disagreed += 1;
                    }
                }
            }
        }
        assert!(checked > 50, "only {checked} steps onto a stair tested");
        assert!(
            disagreed > 5,
            "only {disagreed} of {checked} steps are ones the nearest-height guess got \
             wrong — the sweep is not reaching real staircases"
        );
    }

    /// A body cannot stand in the ground under a surface — a stair, a plinth,
    /// an upper floor — whose body is in the way.
    ///
    /// The premise is read straight from tiledata rather than from
    /// [`MapTerrain::is_obstructed`], so the test does not agree with the code by
    /// construction: a static that is a surface or a wall, whose span overlaps
    /// the sixteen units a body standing on the land would occupy, is something
    /// you are inside of. Exempting surfaces (what this code did) let a player
    /// walk *into* a staircase from the side and stand in the floor beneath it.
    #[test]
    fn the_ground_under_a_surface_is_not_somewhere_to_stand() {
        let Some(install) = real_install() else {
            return;
        };
        let t = install.terrain();

        let mut checked = 0;
        for y in 1500..1900u16 {
            for x in 1350..1650u16 {
                if !t.land_is_ground(x, y) {
                    continue;
                }
                let (_, land_center, _) = t.land_heights(x, y);
                // Is anything's body in the way of a body standing on the land?
                let buried = t.map().statics_at(x, y).any(|item| {
                    let tile = t.tiles().static_tile(item.tile.0);
                    if !tile.flags.is_platform() && !tile.flags.is_blocking() {
                        return false;
                    }
                    let base = i32::from(item.z);
                    let (_, stand) =
                        platform_surface(base, i32::from(tile.height), tile.flags.is_climbable());
                    stand > land_center && base < land_center + PLAYER_HEIGHT
                });
                if !buried {
                    continue;
                }
                assert_ne!(
                    t.surface_at(x, y, land_center),
                    Some(land_center),
                    "({x},{y}) stands a body at z={land_center} with something in it",
                );
                checked += 1;
            }
        }
        assert!(checked > 100, "only {checked} buried ground tiles found");
    }

    /// The step that made a body fall, on the geometry it fell on.
    ///
    /// Britain's castle wall at (1410, 1713): land at z=20, a stack of stone
    /// blocks standing on it, a stone stair over those and a wall over that. Every
    /// height on the tile is inside something — the land is under the blocks, and
    /// the blocks' own tops are under the wall — so there is nowhere on it to
    /// stand and every step onto it must be refused, from any neighbour a body can
    /// reach it from.
    ///
    /// While surfaces were exempt from [`MapTerrain::is_obstructed`] the land at
    /// z=20 looked clear, and a body on the castle stair beside it — seventeen
    /// units up — stepped east and *fell* into the masonry.
    #[test]
    fn a_step_into_a_stack_of_stone_is_refused_rather_than_fallen_into() {
        let Some(install) = real_install() else {
            return;
        };
        let t = install.terrain();
        let (x, y) = (1410u16, 1713u16);

        // The premise, from the map rather than from the code under test: the
        // land, something standing on it, and something standing over that.
        let land = t.map().land(x, y).expect("the block loads");
        assert_eq!(land.z, 20, "the castle wall's footing is not where it was");
        let mut surfaces = 0;
        let mut walls = 0;
        for item in t.map().statics_at(x, y) {
            let tile = t.tiles().static_tile(item.tile.0);
            match tile.flags.is_platform() {
                true => surfaces += 1,
                false if tile.flags.is_blocking() => walls += 1,
                false => {}
            }
        }
        assert!(
            surfaces >= 2 && walls >= 1,
            "({x},{y}) holds {surfaces} surfaces and {walls} walls; \
             this is no longer the stacked wall corner the test means",
        );

        assert_eq!(
            t.surface_at(x, y, i32::from(land.z)),
            None,
            "there is nowhere to stand inside a castle wall",
        );
        let mut approaches = 0;
        for (nx, ny) in [(x - 1, y), (x, y - 1), (x + 1, y), (x, y + 1)] {
            // From wherever a body can actually stand on the neighbour — the
            // corner is walled in on several sides, and a neighbour that is
            // itself unstandable is no approach at all.
            let Some(stand) = (0..=45).rev().find_map(|z| t.surface_at(nx, ny, z)) else {
                continue;
            };
            let from = Point::new(nx, ny, stand as i8);
            assert_eq!(
                t.can_step(from, Point::new(x, y, from.z)),
                None,
                "a step from ({nx},{ny},{stand}) landed inside the castle wall",
            );
            approaches += 1;
        }
        assert!(
            approaches >= 2,
            "only {approaches} neighbours a body can stand on; the test proved nothing",
        );
    }

    /// A body may not walk into a wall that stands at its own height, however
    /// deep the pit on the other side of it is.
    ///
    /// Britain's castle terrace at z=40, and the stairwell that runs along its
    /// north edge behind a wall spanning 40 to 49. Two neighbouring tiles of that
    /// wall, and the only difference between them is how far down the stairwell
    /// has got: at (1411, 1713) the step below stands at 27 and at (1412, 1713) at
    /// 22. Measuring a body from the landing alone, the first is blocked — 27 plus
    /// sixteen is inside the wall — and the second is not, so one tile of the wall
    /// was solid and the next one along was a hole a body fell eighteen units
    /// through. The wall is at eye level walking in either way, which is what
    /// `is_obstructed`'s `top` is for.
    ///
    /// The stairwell itself stays reachable the way it is meant to be: along the
    /// stairs, from the tile above it.
    #[test]
    fn a_wall_at_your_own_height_is_a_wall_however_deep_the_pit_behind_it() {
        let Some(install) = real_install() else {
            return;
        };
        let t = install.terrain();

        // The terrace, and that it is a terrace: paved, flat, walkable.
        for x in 1411..=1412u16 {
            assert_eq!(
                t.surface_at(x, 1714, 40),
                Some(40),
                "({x},1714) is no longer the paved terrace this test walks on",
            );
        }
        // The wall row north of it, and the stairwell behind: two steps of the
        // same stair, one lower than the other.
        let stand_of = |x: u16| {
            t.map()
                .statics_at(x, 1713)
                .filter_map(|item| {
                    let tile = t.tiles().static_tile(item.tile.0);
                    tile.flags.is_platform().then(|| {
                        platform_surface(
                            i32::from(item.z),
                            i32::from(tile.height),
                            tile.flags.is_climbable(),
                        )
                        .1
                    })
                })
                .max()
        };
        assert_eq!(stand_of(1411), Some(27), "the stairwell moved");
        assert_eq!(stand_of(1412), Some(22), "the stairwell moved");

        for x in 1411..=1412u16 {
            assert_eq!(
                t.can_step(Point::new(x, 1714, 40), Point::new(x, 1713, 40)),
                None,
                "({x},1714) walked north through the castle wall",
            );
        }

        // And the way in is still in: down the stairs, from the tile above.
        assert_eq!(
            t.can_step(Point::new(1412, 1712, 22), Point::new(1412, 1713, 22))
                .map(|p| p.z),
            Some(22),
            "the stairwell is no longer reachable from the stair above it",
        );
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
        let Some(install) = real_install() else {
            return;
        };
        let terrain = install.terrain();

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
        let Some(install) = real_install() else {
            return;
        };
        let terrain = install.terrain();
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
        let Some(install) = real_install() else {
            return;
        };
        let terrain = install.terrain();

        let mut checked = 0;
        for y in (1600..1900u16).step_by(7) {
            for x in (1350..1600u16).step_by(7) {
                let cell = terrain.map().land(x, y).unwrap();
                let flags = terrain.tiles().land(cell.tile.0).flags;
                if flags.is_blocking() || flags.is_water() {
                    continue;
                }
                // `surface_at` is obstruction-aware now — it is the whole movement
                // check — so a walkable land tile with a wall standing on it is
                // rightly not standable. Skip those: this test is about *reach*
                // from your own height, not about walls. A tile where a body would
                // stand clear is the case it means to protect.
                let stand = terrain.average_land_z(x, y);
                if terrain.is_obstructed(x, y, stand, stand + PLAYER_HEIGHT) {
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
        let Some(install) = real_install() else {
            return;
        };
        let terrain = install.terrain();
        assert_eq!((terrain.map().width(), terrain.map().height()), (7168, 4096));
        assert_eq!(terrain.map().facet_name(), "Felucca/Trammel (post-ML)");
    }

    /// A pier is the case where the ground and the surface are nowhere near each
    /// other, and everything that confused the two got it wrong there: the land
    /// under Britain's docks is water thirteen units below the planks somebody
    /// walks on. Anything that reads the land's height for "where is this tile
    /// on screen" puts the answer a tile and a half away from the boards — which
    /// is what made a pier tile impossible to point at with the mouse.
    #[test]
    fn a_pier_stands_on_its_planks_and_not_on_the_water_beneath() {
        let Some(install) = real_install() else {
            return;
        };
        let terrain = install.terrain();
        // Britain's docks. The land block here is water at -15; the `wooden
        // plank` static sits at -3 and is a platform one unit tall, so a body
        // stands at -2.
        let (x, y) = (1488, 1749);
        let land = terrain.map().land(x, y).expect("the block loads");
        assert_eq!(land.z, -15, "the pier's land is no longer the water it was");
        assert_eq!(
            terrain.predict_z(x, y, -2),
            -2,
            "the deck a body stands on is not the height the pier predicts"
        );
        assert_eq!(
            terrain.surface_at(x, y, -2),
            Some(-2),
            "a body on the deck cannot stand where it is standing"
        );
    }

    #[test]
    fn a_walking_human_cannot_stand_on_the_ocean() {
        let Some(install) = real_install() else {
            return;
        };
        let terrain = install.terrain();
        // Deep ocean west of Britannia. Water is BLOCK|WATER in tiledata, so a
        // walker gets nothing and a swimmer gets a surface.
        let mut wet = 0;
        let mut dry = 0;
        for x in 60..160u16 {
            for y in 60..160u16 {
                let cell = terrain.map().land(x, y).unwrap();
                if terrain.tiles().land(cell.tile.0).flags.is_water() {
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
        let Some(install) = real_install() else {
            return;
        };
        let terrain = install.terrain();
        let swimming = install.swimming();

        let mut found = false;
        for x in 60..160u16 {
            for y in 60..160u16 {
                let cell = terrain.map().land(x, y).unwrap();
                if !terrain.tiles().land(cell.tile.0).flags.is_water() {
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
        let Some(install) = real_install() else {
            return;
        };
        let terrain = install.terrain();

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
        let Some(install) = real_install() else {
            return;
        };
        let terrain = install.terrain();

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
        let Some(install) = real_install() else {
            return;
        };
        let terrain = install.terrain();

        let mut blocked = 0;
        for y in 1700..1850u16 {
            for x in 1400..1550u16 {
                let Some(ground) = terrain.map().land(x, y) else {
                    continue;
                };
                if terrain.is_obstructed(x, y, i32::from(ground.z), i32::from(ground.z) + PLAYER_HEIGHT) {
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
    fn a_window_wall_stops_a_step_but_not_a_look() {
        // The two halves of `UFLAG2_WINDOW`, which used to be read as one. A wall
        // segment with a window in it is solid to walk into and see-through to look
        // through; treating the flag as "walk thru it" (Sphere's comment says so,
        // Sphere's movement code never once agrees) let every server-driven mobile
        // stroll out of a building through the window.
        let Some(install) = real_install() else {
            return;
        };
        let terrain = install.terrain();

        let mut tested = 0;
        for y in 1550..1900u16 {
            for x in 1350..1600u16 {
                let Some(ground) = terrain.map().land(x, y) else {
                    continue;
                };
                let z = i32::from(ground.z);
                // A window-flagged static that also blocks, standing in the way at
                // ground level. That is a wall with a window, not an open archway.
                let is_window_wall = terrain.map().statics_at(x, y).any(|item| {
                    let tile = terrain.tiles().static_tile(item.tile.0);
                    let base = i32::from(item.z);
                    let top = base + i32::from(tile.height).max(1);
                    tile.flags.has(TileFlags::WINDOW)
                        && tile.flags.is_blocking()
                        && !tile.flags.is_platform()
                        && base < z + PLAYER_HEIGHT
                        && z < top
                });
                if !is_window_wall {
                    continue;
                }
                assert!(
                    terrain.is_obstructed(x, y, z, z + PLAYER_HEIGHT),
                    "({x},{y}) is a wall with a window and must not be walked through",
                );
                tested += 1;
            }
        }
        assert!(
            tested > 10,
            "only {tested} window walls found; the sweep is not reaching real statics",
        );
    }

    #[test]
    fn britain_has_statics_at_all() {
        let Some(install) = real_install() else {
            return;
        };
        let terrain = install.terrain();
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
        let Some(install) = real_install() else {
            return;
        };
        let terrain = install.terrain();
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
        let Some(install) = real_install() else {
            return;
        };
        let terrain = install.terrain();
        assert_eq!(terrain.surface_at(7168, 0, 0), None, "past the east edge");
        assert_eq!(terrain.surface_at(0, 4096, 0), None, "past the south edge");
        assert_eq!(terrain.surface_at(u16::MAX, u16::MAX, 0), None);
    }
}
