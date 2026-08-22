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
//! # A door is not a crate, and the graphic does not say which
//!
//! Both stop a step; only one of them is something a player can open. So a
//! blocker records *which* it is ([`Blocker::door`]), and the tiles the shut ones
//! stand on are the list this module exists to keep: **potentially passable, and
//! currently closed**. [`Cluttered`] can then be read either way — as the world
//! stands, or as it would be with every door open — which is what lets a route be
//! planned *through* a shut door to find out where the way would go, and the walk
//! stop in front of it. `steer::plan` is the reader; the server has the same pair
//! under the same name (`state::obstruct`'s `Obstacle::door` and
//! `LiveTerrain::through_doors`), which is the point.
//!
//! **The graphic is what says a door is open, and its own art disagrees.** This
//! module used to argue that no state had to be tracked at all — "a door's own
//! graphic changes when it swings, so the shut leaf blocks and the open one does
//! not". Measured against the real `tiledata.mul`, that is false: all 164 shut
//! leaves in `client/render`'s door table are impassable, and so are **132 of the
//! open ones**. A client trusting the flags alone therefore refuses to walk
//! through open doors — steps the shard allows, which is the mirror-image of the
//! bug this module was written for. What does say is the table itself, ported
//! from the reference emulator's own arithmetic
//! ([`openshard_client_render::doors`]): the shut leaves sit on a family's even
//! offsets and the open ones on the odd, and an open leaf is left out of the index
//! entirely.
//!
//! # Mobiles
//!
//! Server-side movement remains authoritative. Mobiles are a client-side
//! courtesy obstacle: the shard may permit occupying their tile, but a route
//! that visibly walks through an NPC is not a useful route. The next snapshot
//! replaces this short-lived projection and replans any active move order.

use std::collections::HashMap;

use openshard_client_net::view::Mobile;
use openshard_client_render::doors;
use openshard_client_render::items::GroundItem;
use openshard_map::map::Map;
use openshard_movement::{MapTerrain, Terrain, Tile};
use openshard_protocol::wire::Graphic;
use openshard_protocol::world::Point;
use openshard_uofiles::tiledata::TileData;

/// A mobile's body height in z-units — how tall a span a blocker has to reach
/// into to be in the way. `openshard_movement::PLAYER_HEIGHT`, and the server's
/// `obstruct::MOBILE_HEIGHT`, which is the same number said in the same units on
/// the other end of the wire.
const MOBILE_HEIGHT: i32 = openshard_movement::PLAYER_HEIGHT;

/// A placed item's tiledata height, kept distinct from other byte-sized item
/// properties such as weight and layer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct TileHeight(u8);

impl TileHeight {
    const fn new(raw: u8) -> Self {
        Self(raw)
    }

    /// The span a blocker occupies. Zero-height art still occupies its tile.
    fn span(self) -> i32 {
        i32::from(self.0).max(1)
    }
}

/// One placed item blocking a tile: where its body starts, how tall it is, and
/// whether it is something that could be opened.
///
/// No graphic and no serial. This end never has to say *which* item stopped the
/// step — it cannot open one anyway; a player does that by double-clicking — but
/// it does have to say what *kind* stopped it, because a route that stops at a
/// door means "go and open it" and a route that stops at a crate means "there is
/// no way through". The server's `Obstacle` carries the same flag beside the
/// entity, for the same distinction.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Blocker {
    /// The base z the item sits at — its `0x1A` position's z.
    z: i8,
    /// Its tiledata height: its body spans `[z, z + height)`. Zero-height art
    /// still occupies its own tile, so the span is never empty — see
    /// [`Clutter::blocked_at`].
    height: TileHeight,
    /// A shut door: in the way now, and not in the way at all if somebody opens
    /// it. What [`Clutter::blocked_through_doors`] leaves out, and the whole of
    /// what "potentially passable" means here.
    door: bool,
}

/// The ground items that block, indexed by tile.
///
/// Built once per view update and read by every step decision, rather than
/// scanned per call: `find_path` asks about hundreds of tiles for one click, and
/// a linear pass over everything on screen for each of them is the same answer
/// bought a hundred times over.
#[derive(Default, Debug)]
pub struct Clutter {
    tiles: HashMap<Tile, Vec<Blocker>>,
}

impl Clutter {
    /// Index whatever in `items` or `mobiles` is in the way, and say of each
    /// placed item whether it is a door.
    ///
    /// Two questions, two sources, and the module header is where the argument
    /// for the second one lives:
    ///
    /// - **Does it block?** `Terrain::item_blocks` — the tiledata flags — which
    ///   is the same predicate the server decides with, so the two ends agree by
    ///   construction rather than by resemblance.
    /// - **Is it a door, and has it swung?** `client/render`'s door table, which
    ///   is the reference emulator's own arithmetic over the art indices. The
    ///   flags cannot answer either half: an open leaf is impassable in tiledata
    ///   just as often as a shut one.
    ///
    /// An open leaf is therefore left out altogether — nothing is in the way of a
    /// body walking through an open door — and a shut one goes in marked, which
    /// is the list [`Clutter::blocked_through_doors`] reads.
    pub fn of<'a>(
        items: &[GroundItem],
        mobiles: impl IntoIterator<Item = &'a Mobile>,
        tiles: &TileData,
    ) -> Self {
        let mut blocked: HashMap<Tile, Vec<Blocker>> = HashMap::new();
        for item in items {
            // What is drawn is what is in the way — see `GroundItem::displayed`.
            let tile = tiles.static_tile(item.displayed().0);
            if !tile.flags.is_blocking() || doors::is_open(item.displayed()) {
                continue;
            }
            let at = Tile::new(item.at.x, item.at.y);
            blocked.entry(at).or_default().push(Blocker {
                z: item.at.z,
                height: TileHeight::new(tile.height),
                door: doors::is_door(item.displayed()),
            });
        }
        // The server may permit two mobiles on one tile, but routing a player
        // visibly through an NPC is still a bad client-side result.
        for mobile in mobiles {
            let at = Tile::new(mobile.position.x, mobile.position.y);
            blocked.entry(at).or_default().push(Blocker {
                z: mobile.position.z,
                height: TileHeight::new(MOBILE_HEIGHT as u8),
                door: false,
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
    fn blocked_at(&self, tile: Tile, stand_z: i32) -> bool {
        self.blocked(tile, stand_z, Doors::AsTheyStand)
    }

    /// The same question with every shut door on the tile opened: what would
    /// still be in the way then.
    ///
    /// The list this module keeps, read the way a plan reads it. A doorway with
    /// nothing but a shut leaf in it comes back open; a doorway with a crate
    /// dragged into it does not, because opening the door does not move the
    /// crate. Both halves matter: the first is where a route may be planned
    /// through, the second is why a route may not be planned through everything.
    fn blocked_through_doors(&self, tile: Tile, stand_z: i32) -> bool {
        self.blocked(tile, stand_z, Doors::AllOpen)
    }

    /// The two above, which differ only in whether a shut door counts.
    fn blocked(&self, tile: Tile, stand_z: i32, doors: Doors) -> bool {
        self.tiles.get(&tile).is_some_and(|blockers| {
            blockers.iter().any(|blocker| {
                if blocker.door && doors == Doors::AllOpen {
                    return false;
                }
                let bottom = i32::from(blocker.z);
                let top = bottom + blocker.height.span();
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

    /// The same map read as though every shut door on it stood open.
    ///
    /// **Not a walkability answer**, and never what decides a step: nothing here
    /// opens a door, and a body walked into one on this terrain's word would be
    /// refused by the shard. It is what a *plan* asks to find out where the way
    /// would go if the door were opened — `steer::plan`, which walks the real
    /// half of the answer and stops where the two part company. The server's
    /// `LiveTerrain::through_doors` is the same terrain under the same name, for
    /// the creature that plans a route it intends to open its way along.
    pub const fn over_with_doors_open<'a>(
        &'a self,
        map: &'a Map,
        tiles: &'a TileData,
    ) -> Cluttered<'a, MapTerrain<&'a Map, &'a TileData>> {
        Cluttered {
            map: MapTerrain::new(map, tiles),
            clutter: self,
            doors: Doors::AllOpen,
        }
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
        Cluttered {
            map,
            clutter: self,
            doors: Doors::AsTheyStand,
        }
    }
}

/// Which of the two readings of the same ground a [`Cluttered`] is.
///
/// A `bool` would do and is exactly what this must not be: the two are read at
/// call sites four modules apart, and "true" there says nothing about which way
/// round it is. See [`Clutter::over_with_doors_open`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Doors {
    /// Shut is shut: what a step is actually allowed by.
    AsTheyStand,
    /// Every shut door stands open: what a route may be planned through.
    AllOpen,
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
    /// Which reading this is — see [`Clutter::over_with_doors_open`].
    doors: Doors,
}

impl<M: Terrain> Cluttered<'_, M> {
    /// Whether this tile is blocked, by whichever of the two readings this is.
    fn blocked(&self, tile: Tile, stand_z: i32) -> bool {
        match self.doors {
            Doors::AsTheyStand => self.clutter.blocked_at(tile, stand_z),
            Doors::AllOpen => self.clutter.blocked_through_doors(tile, stand_z),
        }
    }
}

impl<M: Terrain> Terrain for Cluttered<'_, M> {
    fn can_step(&self, from: Point, to: Point) -> Option<Point> {
        let landed = self.map.can_step(from, to)?;
        // At the height the body will stand at, not the height it asked for:
        // `to.z` is the caller's guess and `can_step` is what corrects it.
        let onto = Tile::new(to.x, to.y);
        match self.blocked(onto, i32::from(landed.z)) {
            true => None,
            false => Some(landed),
        }
    }

    fn ground_z(&self, tile: Tile) -> Option<i8> {
        self.map.ground_z(tile)
    }

    fn land_tile(&self, tile: Tile) -> Option<openshard_movement::LandTile> {
        self.map.land_tile(tile)
    }

    fn statics_at(&self, tile: Tile, out: &mut Vec<(Graphic, i8)>) {
        self.map.statics_at(tile, out);
    }

    fn stand_z(&self, tile: Tile, near_z: i32) -> Option<i32> {
        self.map.stand_z(tile, near_z)
    }

    fn spawn_z(&self, tile: Tile, near_z: i32) -> Option<i32> {
        self.map.spawn_z(tile, near_z)
    }

    fn can_fit(&self, tile: Tile, z: i32, height: i32) -> bool {
        self.map.can_fit(tile, z, height) && !self.blocked(tile, z)
    }

    // The six tiledata questions this used to forward have left the trait. Two of
    // them — `multi_components` and `land_is_water` — were never forwarded at
    // all and fell into the trait's defaults, so a caller asking this overlay
    // about a multi was told there was none. Nothing on this end asked, which is
    // the only reason it was not a bug; the client reads `Resources::tiledata`
    // and `Resources::multis` directly, as `Clutter::of` above already does.
    fn sight_clear(&self, from: Point, to: Point) -> bool {
        // The map's answer alone. A crate is furniture, not a wall — the same
        // line the server draws, which only treats a shut *door* as opaque.
        //
        // That door is now a fact this end holds ([`Blocker::door`]), so the
        // remaining half of the server's rule *could* be drawn here. It is not,
        // because nothing on this end computes line of sight for gameplay yet
        // and a rule with no reader is a rule nobody will notice going wrong.
        // See `docs/client.md`'s backlog entry, which is where it is owed.
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
    ///
    /// Nothing placed this way is a door: which art is a door is the other half
    /// [`Clutter::of`] answers, and a fixture that could claim it would be
    /// claiming the very thing under test.
    fn placed(blockers: &[(Point, u8)]) -> Self {
        let mut tiles: HashMap<Tile, Vec<Blocker>> = HashMap::new();
        for (at, height) in blockers {
            tiles.entry(Tile::new(at.x, at.y)).or_default().push(Blocker {
                z: at.z,
                height: TileHeight::new(*height),
                door: false,
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
    use openshard_protocol::items::ItemAmount;
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

    #[test]
    fn a_step_into_an_npc_body_is_refused_by_this_end() {
        let east = Point::new(HERE.x + 1, HERE.y, 0);
        // Mobiles enter the shipping index with this same body-height span;
        // keep the routing rule test independent of client art files.
        let clutter = Clutter::placed(&[(east, MOBILE_HEIGHT as u8)]);
        let terrain = clutter.over_terrain(OpenWorld);
        assert!(
            step_allowed(&terrain, HERE, Direction::East).is_none(),
            "an NPC due east did not stop the step"
        );
        assert!(
            step_allowed(&terrain, HERE, Direction::North).is_some(),
            "an NPC due east stopped a step the other way"
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
            amount: ItemAmount::ONE,
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
        let clutter = Clutter::of(&[item(100, 100, 0, water_barrel)], [], &tiles);
        assert!(
            !clutter.blocked_at(Tile::new(100, 100), 0),
            "a water barrel was made solid here and the shard would still allow the step"
        );
    }

    #[test]
    fn a_placed_barrel_blocks_its_tile_at_ground_level() {
        let Some(tiles) = client_tiledata() else {
            return;
        };
        let clutter = Clutter::of(&[item(100, 100, 0, BARREL)], [], &tiles);
        assert!(
            clutter.blocked_at(Tile::new(100, 100), 0),
            "a barrel underfoot is not in the way"
        );
        assert!(
            !clutter.blocked_at(Tile::new(101, 100), 0),
            "a barrel blocked a tile it is not on"
        );
    }

    #[test]
    fn a_barrel_two_storeys_up_leaves_the_ground_floor_open() {
        let Some(tiles) = client_tiledata() else {
            return;
        };
        let clutter = Clutter::of(&[item(100, 100, 40, BARREL)], [], &tiles);
        assert!(
            !clutter.blocked_at(Tile::new(100, 100), 0),
            "a crate on an upper floor sealed the floor beneath it"
        );
        assert!(
            clutter.blocked_at(Tile::new(100, 100), 40),
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
        let clutter = Clutter::of(&[item(100, 100, 0, gold)], [], &tiles);
        assert!(
            !clutter.blocked_at(Tile::new(100, 100), 0),
            "a coin on the floor stopped a step"
        );
    }

    /// `MetalDoor` facing 0: shut on the even graphic, open on the odd — the
    /// family `client/render`'s door table was ported off.
    const DOOR_SHUT: Graphic = Graphic(0x0675);
    const DOOR_OPEN: Graphic = Graphic(0x0676);

    /// **The measurement that killed this module's old argument.** It used to
    /// hold that no door state had to be tracked, because "the shut leaf blocks
    /// and the open one does not" by its flags alone. Both leaves are
    /// impassable, so a client reading the flags refuses to walk through an open
    /// door — a step the shard allows, which is the mirror-image of the bug this
    /// module was written to fix.
    ///
    /// Pinned because it is the whole reason the door table is consulted at all,
    /// and because it is the kind of fact that reads like a typo a year later.
    #[test]
    fn an_open_door_s_own_art_is_impassable_just_like_the_shut_one() {
        let Some(tiles) = client_tiledata() else {
            return;
        };
        assert!(
            tiles.static_tile(DOOR_SHUT.0).flags.is_blocking(),
            "a shut door is not impassable in tiledata"
        );
        assert!(
            tiles.static_tile(DOOR_OPEN.0).flags.is_blocking(),
            "an open door's art is no longer impassable — the flags may now say what the table is for"
        );
        assert!(!doors::is_open(DOOR_SHUT), "0x0675 is the shut leaf");
        assert!(doors::is_open(DOOR_OPEN), "0x0676 is the open one");
    }

    /// So the open leaf is left out of the index entirely: nothing is in the way
    /// of a body walking through an open door.
    #[test]
    fn an_open_door_is_not_in_the_way() {
        let Some(tiles) = client_tiledata() else {
            return;
        };
        let clutter = Clutter::of(&[item(100, 100, 0, DOOR_OPEN)], [], &tiles);
        assert!(
            !clutter.blocked_at(Tile::new(100, 100), 0),
            "an open door blocked its own doorway"
        );
    }

    /// And the shut one is in the way *and* known to be openable, which is the
    /// list this module keeps: blocked now, not blocked with the doors open.
    #[test]
    fn a_shut_door_is_in_the_way_now_and_not_with_the_doors_open() {
        let Some(tiles) = client_tiledata() else {
            return;
        };
        let clutter = Clutter::of(&[item(100, 100, 0, DOOR_SHUT)], [], &tiles);
        let doorway = Tile::new(100, 100);
        assert!(clutter.blocked_at(doorway, 0), "a shut door let a step through");
        assert!(
            !clutter.blocked_through_doors(doorway, 0),
            "a shut door is what 'potentially passable' means; the plan must be able to look past it"
        );
    }

    /// A crate is in the way both ways round — opening a door does not move the
    /// furniture. Without this the two readings would differ by everything
    /// placed rather than by the doors, and a route would be planned through a
    /// stack of barrels nobody can shift.
    #[test]
    fn a_barrel_is_in_the_way_whatever_the_doors_are_doing() {
        let Some(tiles) = client_tiledata() else {
            return;
        };
        let clutter = Clutter::of(&[item(100, 100, 0, BARREL)], [], &tiles);
        assert!(clutter.blocked_through_doors(Tile::new(100, 100), 0));
    }

    /// A crate dragged into a doorway seals it: the tile is blocked in both
    /// readings even though a door stands there too, because the crate is still
    /// there once the door has swung. `any` over the blockers is what gets this
    /// right, and a naive "is there a door here" would get it wrong.
    #[test]
    fn a_shut_door_with_a_crate_in_it_is_not_potentially_passable() {
        let Some(tiles) = client_tiledata() else {
            return;
        };
        let clutter = Clutter::of(
            &[item(100, 100, 0, DOOR_SHUT), item(100, 100, 0, BARREL)],
            [],
            &tiles,
        );
        assert!(
            clutter.blocked_through_doors(Tile::new(100, 100), 0),
            "a doorway with a barrel in it was called openable"
        );
    }
}
