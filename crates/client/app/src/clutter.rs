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
//! So this is the client's half of the shard's own index, built from the one
//! thing this end has: the ground items in the [`WorldView`](openshard_client_net::view::WorldView),
//! and the same `tiledata.mul` the server read to decide they block. Same
//! predicate (the tiledata blocking flag), same z-span (base z, tiledata height), so
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
//! cover records *which* it is (`CoverKind::Blocks`'s `door`), and the tiles the shut ones
//! stand on are the list this module exists to keep: **potentially passable, and
//! currently closed**. A `Footing` over it can then be read either way — as the
//! world stands, or as it would be with every door open — which is what lets a
//! route be planned *through* a shut door to find out where the way would go, and
//! the walk stop in front of it. `steer::plan` is the reader; the server has the same pair
//! under the same name (`state::obstruct`'s `Obstacle::door`), and since node E
//! of `docs/map/terrain_seam.md` it is not merely the same name but the same
//! type: both ends build an `openshard_movement::Overlay`.
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
use openshard_movement::{Cover, Overlay, Tile};
use openshard_uofiles::tiledata::TileData;

/// A mobile's body height in z-units — how tall a span a blocker has to reach
/// into to be in the way. `openshard_movement::PLAYER_HEIGHT`, and the server's
/// `obstruct::MOBILE_HEIGHT`, which is the same number said in the same units on
/// the other end of the wire.
const MOBILE_HEIGHT: i32 = openshard_movement::PLAYER_HEIGHT;

/// Index whatever in `items` or `mobiles` is in the way, and say of each placed
/// item whether it is a door.
///
/// Two questions, two sources, and the module header is where the argument for
/// the second one lives:
///
/// - **Does it block?** The tiledata flags, which is the same predicate the
///   server decides with, so the two ends agree by construction rather than by
///   resemblance.
/// - **Is it a door, and has it swung?** `client/render`'s door table, which is
///   the reference emulator's own arithmetic over the art indices. The flags
///   cannot answer either half: an open leaf is impassable in tiledata just as
///   often as a shut one.
///
/// An open leaf is therefore left out altogether — nothing is in the way of a
/// body walking through an open door — and a shut one goes in marked, which is
/// what [`Doors::AllOpen`] reads.
///
/// Built whole and thrown away whole. This end has no identities to address a
/// finer edit to, which is the same fact that keeps [`Cover`] free of an owner
/// — see `openshard_movement::overlay`.
pub fn of<'a>(
    items: &[GroundItem],
    mobiles: impl IntoIterator<Item = &'a Mobile>,
    tiles: &TileData,
) -> Overlay {
    let mut covers: HashMap<Tile, Vec<Cover>> = HashMap::new();
    for item in items {
        // What is drawn is what is in the way — see `GroundItem::displayed`.
        let tile = tiles.static_tile(item.displayed().0);
        if !tile.flags.is_blocking() || doors::is_open(item.displayed()) {
            continue;
        }
        let at = Tile::new(item.at.x, item.at.y);
        covers
            .entry(at)
            .or_default()
            .push(match doors::is_door(item.displayed()) {
                true => Cover::door(item.at.z, tile.height),
                false => Cover::blocking(item.at.z, tile.height),
            });
    }
    // The server may permit two mobiles on one tile, but routing a player
    // visibly through an NPC is still a bad client-side result. Not a category
    // the shared type names: nothing downstream reads a mobile *as* one, so it
    // goes in as furniture with a body's height.
    for mobile in mobiles {
        let at = Tile::new(mobile.position.x, mobile.position.y);
        covers
            .entry(at)
            .or_default()
            .push(Cover::blocking(mobile.position.z, MOBILE_HEIGHT as u8));
    }
    let mut overlay = Overlay::default();
    for (tile, covers) in covers {
        overlay.set(tile, covers);
    }
    overlay
}

/// Index blockers by hand, with no tiledata to read flags from.
///
/// For the tests that are about the *span* and the detour rather than about
/// which art is impassable — the flag half is [`of`]'s and is covered against
/// the real `tiledata.mul` below.
///
/// Nothing placed this way is a door: which art is a door is the other half
/// [`of`] answers, and a fixture that could claim it would be claiming the very
/// thing under test.
#[cfg(test)]
fn placed(blockers: &[(openshard_protocol::world::Point, u8)]) -> Overlay {
    let mut covers: HashMap<Tile, Vec<Cover>> = HashMap::new();
    for (at, height) in blockers {
        covers
            .entry(Tile::new(at.x, at.y))
            .or_default()
            .push(Cover::blocking(at.z, *height));
    }
    let mut overlay = Overlay::default();
    for (tile, covers) in covers {
        overlay.set(tile, covers);
    }
    overlay
}

#[cfg(test)]
mod tests {
    use super::*;
    use openshard_movement::{Around, Detour, Doors, Footing, Heading, Lean, Leeway, Step, step_allowed};
    use openshard_protocol::direction::Direction;
    use openshard_protocol::items::ItemAmount;
    use openshard_protocol::wire::{Graphic, Hue};
    use openshard_protocol::world::Point;

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
        let clutter = placed(&[(east, BARREL_HEIGHT)]);
        let terrain = Footing::new(None, &clutter, Doors::AsTheyStand);
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
        let clutter = placed(&[(east, MOBILE_HEIGHT as u8)]);
        let terrain = Footing::new(None, &clutter, Doors::AsTheyStand);
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
        let clutter = placed(&[(north_east, BARREL_HEIGHT)]);
        let terrain = Footing::new(None, &clutter, Doors::AsTheyStand);
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
        let clutter = placed(&[(east, BARREL_HEIGHT)]);
        let terrain = Footing::new(None, &clutter, Doors::AsTheyStand);
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
        let clutter = of(&[item(100, 100, 0, water_barrel)], [], &tiles);
        assert!(
            clutter
                .blocker_at(Tile::new(100, 100), 0, Doors::AsTheyStand)
                .is_none(),
            "a water barrel was made solid here and the shard would still allow the step"
        );
    }

    #[test]
    fn a_placed_barrel_blocks_its_tile_at_ground_level() {
        let Some(tiles) = client_tiledata() else {
            return;
        };
        let clutter = of(&[item(100, 100, 0, BARREL)], [], &tiles);
        assert!(
            clutter
                .blocker_at(Tile::new(100, 100), 0, Doors::AsTheyStand)
                .is_some(),
            "a barrel underfoot is not in the way"
        );
        assert!(
            clutter
                .blocker_at(Tile::new(101, 100), 0, Doors::AsTheyStand)
                .is_none(),
            "a barrel blocked a tile it is not on"
        );
    }

    #[test]
    fn a_barrel_two_storeys_up_leaves_the_ground_floor_open() {
        let Some(tiles) = client_tiledata() else {
            return;
        };
        let clutter = of(&[item(100, 100, 40, BARREL)], [], &tiles);
        assert!(
            clutter
                .blocker_at(Tile::new(100, 100), 0, Doors::AsTheyStand)
                .is_none(),
            "a crate on an upper floor sealed the floor beneath it"
        );
        assert!(
            clutter
                .blocker_at(Tile::new(100, 100), 40, Doors::AsTheyStand)
                .is_some(),
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
        let clutter = of(&[item(100, 100, 0, gold)], [], &tiles);
        assert!(
            clutter
                .blocker_at(Tile::new(100, 100), 0, Doors::AsTheyStand)
                .is_none(),
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
        let clutter = of(&[item(100, 100, 0, DOOR_OPEN)], [], &tiles);
        assert!(
            clutter
                .blocker_at(Tile::new(100, 100), 0, Doors::AsTheyStand)
                .is_none(),
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
        let clutter = of(&[item(100, 100, 0, DOOR_SHUT)], [], &tiles);
        let doorway = Tile::new(100, 100);
        assert!(
            clutter.blocker_at(doorway, 0, Doors::AsTheyStand).is_some(),
            "a shut door let a step through"
        );
        assert!(
            clutter.blocker_at(doorway, 0, Doors::AllOpen).is_none(),
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
        let clutter = of(&[item(100, 100, 0, BARREL)], [], &tiles);
        assert!(
            clutter
                .blocker_at(Tile::new(100, 100), 0, Doors::AllOpen)
                .is_some()
        );
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
        let clutter = of(
            &[item(100, 100, 0, DOOR_SHUT), item(100, 100, 0, BARREL)],
            [],
            &tiles,
        );
        assert!(
            clutter
                .blocker_at(Tile::new(100, 100), 0, Doors::AllOpen)
                .is_some(),
            "a doorway with a barrel in it was called openable"
        );
    }
}
