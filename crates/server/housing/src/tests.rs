//! Placing a house over a world with nothing in it but a terrain that knows one
//! multi.
//!
//! The multi is a fixture rather than a client file, and it is the right call
//! here for the reason `client_files.rs` is the wrong place for a rule: what is
//! under test is the *arithmetic* — an offset added to an origin, a height added
//! to a z, a flag deciding whether a component blocks — and a real villa would
//! test the same arithmetic 148 times while making the expected answer
//! impossible to write down.
//!
//! What a real file settles is the format, and `uofiles::multi` already gates
//! that against one.

use std::collections::BTreeMap;

use openshard_gateway::ConnectionId;
use openshard_map::grid::Tile;
use openshard_map::overlay::{
    Body,
    Doors,
};
use openshard_movement::scene::Scene;
use openshard_protocol::access::AccessLevel;
use openshard_protocol::direction::Direction;
use openshard_protocol::encoded::{
    DesignEdit,
    RawStorey,
};
use openshard_protocol::identity::AccountName;
use openshard_protocol::serial::{
    Serial,
    SerialKind,
};
use openshard_protocol::version::ClientVersion;
use openshard_protocol::wire::{
    Graphic,
    MultiId,
};
use openshard_protocol::world::{
    Facet,
    Point,
};
use openshard_state::components::HouseDesign;
use openshard_state::connection::Connection;
use openshard_state::{
    Client,
    FacetState,
};
use openshard_tiles::{
    TileData,
    TileFlags,
};
use openshard_uofiles::multi::{
    Component,
    Multi,
    Multis,
};

use super::*;

/// A reach no storey in these tests can be out of.
///
/// `Overlay::surface_at` bounds the climb as well as choosing the nearest
/// surface, and what these tests ask is whether a house *laid* a surface at all
/// — not whether a body standing on the ground could climb to it in one step. A
/// first floor seven above the ground is deliberately out of a step's reach and
/// is reached by its stair; asking with a real limit would make these assert the
/// stair rather than the floor.
const ANY_REACH: i32 = i32::MAX;

/// A small world, and the multi id everything here places.
const SIZE: u32 = 32;
const COTTAGE: u16 = 0x64;
/// A customisable foundation the fixture terrain knows the platform of. Any id
/// inside [`FOUNDATION_IDS`] would do; this is its first.
const FOUNDATION: u16 = 0x13EC;

/// Fixture/content ids arrive bare; the housing API deliberately does not.
fn place(
    state: &mut WorldState,
    actor: EntityId,
    at: Point,
    facet: Facet,
    multi: u16,
    owner: Serial,
) -> Result<EntityId, Refusal> {
    super::place(
        state,
        actor,
        at,
        facet,
        MultiId::from_graphic(Graphic(multi)),
        owner,
    )
}

/// A wall: impassable, twenty tall — the classic UO wall the door height was
/// taken from.
const WALL: u16 = 0x0006;
/// A floor: drawn, walked over, and *not* impassable — `PLATFORM` with a
/// tiledata height of zero, which is what every wooden-boards component of every
/// classic multi is.
///
/// It goes into the footprint as a **surface** and never as a body: a house
/// whose floor blocked would be sealed shut from the inside, and a floor of no
/// thickness laid on the ground it duplicates is the case that would do it. See
/// [`openshard_map::overlay::Cover::of_static`].
const FLOOR: u16 = 0x0007;

/// A stair: `PLATFORM | CLIMBABLE`, five tall, like multi `0x0064`'s own stone
/// stairs. Met at its base and stood on half way up.
const STAIR: u16 = 0x0008;
/// A decorative house banister. It is blocking in the client's generic
/// tiledata, but imported house designs must not turn it into a wall through
/// the floor below it.
const BANISTER: u16 = 0x08C6;

/// A roof tile: the one thing the design editor's build verb will not lay,
/// because roofs are their own two subcommands on their own plane.
const ROOF: u16 = 0x0009;

/// The ground these tests build on, and the table it reads.
///
/// **A real map.** `land` is the id under every tile — `0` is nothing in
/// particular, a road id makes the whole facet a street — and `fits` is whether
/// the ground will take a house at all, said the way the world says it: land
/// flagged [`TileFlags::BLOCK`] is ground nobody stands on, so there is no
/// surface for a footprint to rest on and `can_fit` refuses it through the
/// shard's own rule. That is ServUO's rules two and four asked as one question,
/// and it used to be a boolean a double returned.
///
/// A refused *step* in a test below is therefore the obstruction index refusing
/// it and never the terrain — flat ground at one height allows every step, which
/// is the same guarantee the double gave by fiat.
///
/// The wall is impassable and twenty tall, the floor is a platform of no
/// thickness and the stair a climbable one five tall. All three come from the
/// scene's own tiledata, which is the table the shard is handed, so a house's
/// footprint and the terrain under it cannot be reading two different files.
fn ground_scene(land: u16, fits: bool) -> Scene {
    let side = u16::try_from(SIZE - 1).unwrap();
    let mut scene = Scene::flat_holding(side, side, 0);
    scene.land_art(land, if fits { 0 } else { TileFlags::BLOCK });
    scene.land_everywhere(land);
    scene.art(WALL, TileFlags::WALL | TileFlags::BLOCK, 20);
    scene.art(FLOOR, TileFlags::FLOOR | TileFlags::PLATFORM, 0);
    scene.art(STAIR, TileFlags::PLATFORM | TileFlags::CLIMBABLE, 5);
    scene.art(BANISTER, TileFlags::BLOCK, 5);
    scene.art(ROOF, TileFlags::ROOF, 3);
    scene
}

/// A real multi table holding `components` under both ids the fixture places, so
/// a test can ask for the cottage or the foundation without a second table.
fn multis(components: Vec<Component>) -> Multis {
    Multis::of([
        Multi::new(COTTAGE, components.clone()),
        Multi::new(0x0074, components.clone()),
        Multi::new(FOUNDATION, components),
    ])
}

fn component(graphic: u16, dx: i16, dy: i16, dz: i16, drawn: bool) -> Component {
    Component {
        graphic: Graphic(graphic),
        dx,
        dy,
        dz,
        // `1` is the `.mul`'s "drawn" value and `0` its skip — the sense that
        // reads backwards, and the reason this helper takes a `bool`.
        flags: u64::from(drawn),
    }
}

#[test]
fn an_imported_design_uses_a_fitting_foundation_without_moving_its_origin() {
    let small = vec![component(1, -3, -3, 0, true), component(1, 3, 3, 0, true)];
    let fitting = vec![component(1, -5, -5, 0, true), component(1, 6, 6, 0, true)];
    let multis = Multis::of([Multi::new(0x13EC, small), Multi::new(0x13ED, fitting)]);
    // This export begins in its north-west corner. Its coordinates are its
    // placement contract, rather than an instruction to centre it again.
    let design = vec![component(FLOOR, 0, 0, 0, true), component(FLOOR, 11, 11, 0, true)];

    let (foundation, fitted) = fit_design_to_foundation(&multis, design).expect("a 12x12 foundation");

    assert_eq!(foundation, MultiId(0x13ED));
    assert_eq!(
        fitted,
        vec![component(FLOOR, 0, 0, 0, true), component(FLOOR, 11, 11, 0, true)],
        "choosing the foundation must not move the completed template"
    );
}

/// A cottage: four walls in a ring, a floor in the middle, and one component the
/// client never draws.
fn cottage() -> Vec<Component> {
    vec![
        component(1, 0, 0, 0, false), // the signature tile every multi starts with
        component(WALL, -1, -1, 0, true),
        component(WALL, 1, -1, 0, true),
        component(WALL, -1, 1, 0, true),
        component(WALL, 1, 1, 0, true),
        component(FLOOR, 0, 0, 0, true),
        // Drawn nowhere, and far enough away that folding it in would be obvious.
        component(WALL, 10, 10, 0, false),
    ]
}

fn world_with(components: Vec<Component>) -> WorldState {
    ground_of(components, 0, true)
}

/// A fixture world that knows one extra multi, for testing content models whose
/// own sign or closed-door art supplies fixtures.
fn world_with_extra_multi(multi: Multi) -> WorldState {
    let (map, tiles) = ground_scene(0, true).into_shard(Facet(0));
    let mut facets = BTreeMap::new();
    facets.insert(
        Facet(0),
        FacetState::new(
            Some(map),
            None,
            SIZE,
            SIZE,
            openshard_state::facet_rules::FacetRules::classic(Facet(0)),
            None,
            &tiles,
        ),
    );
    WorldState::new(
        facets,
        Facet(0),
        tiles,
        Multis::of([Multi::new(COTTAGE, cottage()), multi]),
        openshard_map::grid::Tile::new(0, 0),
        1,
    )
}

/// A world the size of Felucca, for the one test that uses the shipped region
/// data rather than a fixture.
///
/// `SIZE` is 32 and Covetous is at x 5376, so the small world cannot hold the
/// coordinate the data names. `Regions::bucket_of` *clamps* rather than failing,
/// which means a test placed off the edge of a small world would pass by
/// accident — the clamp would fold the point back into a bucket that happens to
/// hold the region. The real extent is what makes the coordinate mean what it
/// says.
///
/// The extent goes in at construction rather than being written over the facet's
/// fields afterwards. Three things are sized from it — the sector grid, the
/// region index, and the width and height the client is told — and
/// [`FacetState::new`] is the one place they cannot be put out of step with each
/// other.
fn britannia_with(components: Vec<Component>) -> WorldState {
    ground_sized(components, 0, true, 7168, 4096)
}

fn ground_of(components: Vec<Component>, land: u16, fits: bool) -> WorldState {
    ground_sized(components, land, fits, SIZE, SIZE)
}

/// The same, on a facet that claims `width` by `height` tiles. The ground is the
/// fixture scene either way — only the extent the live indexes are built from
/// changes.
fn ground_sized(components: Vec<Component>, land: u16, fits: bool, width: u32, height: u32) -> WorldState {
    // The ground and the table it reads, from one scene: a wall's height cannot
    // disagree with the tiledata the terrain is looking at.
    let (map, tiles) = ground_scene(land, fits).into_shard(Facet(0));
    let mut facets = BTreeMap::new();
    facets.insert(
        Facet(0),
        FacetState::new(
            Some(map),
            None,
            width,
            height,
            openshard_state::facet_rules::FacetRules::classic(Facet(0)),
            None,
            &tiles,
        ),
    );
    WorldState::new(
        facets,
        Facet(0),
        tiles,
        multis(components),
        openshard_map::grid::Tile::new(0, 0),
        1,
    )
}

/// An item on the ground, a container if asked for one.
fn an_item(state: &mut WorldState, at: Point, container: bool) -> EntityId {
    let (entity, _) = state.registry.spawn_with_serial(SerialKind::Item).unwrap();
    state.registry.insert(
        entity,
        Drawn {
            id:  Graphic(0x0E3C),
            hue: openshard_protocol::wire::Hue(0),
        },
    );
    state.registry.insert(entity, Position(at));
    state.registry.insert(entity, Facet(0));
    openshard_state::establish_item_location(
        state,
        entity,
        openshard_state::ItemLocation::ground(Facet(0), at),
    )
    .unwrap();
    if container {
        state.registry.insert(
            entity,
            openshard_state::components::Container {
                gump: Graphic(0x003C),
            },
        );
    }
    entity
}

fn an_owner(state: &mut WorldState) -> Serial {
    an_actor(state).1
}

/// A mobile, keeping both halves.
///
/// `place` takes the actor as well as the owner now, and most tests want the
/// same mobile for both — but not all of them, which is why the two are separate
/// parameters and this hands back a pair rather than a `Serial` the caller has
/// to look back up.
fn an_actor(state: &mut WorldState) -> (EntityId, Serial) {
    state.registry.spawn_with_serial(SerialKind::Mobile).unwrap()
}

#[test]
fn a_house_is_an_item_whose_graphic_is_the_multi() {
    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    let house = place(&mut state, actor, at, Facet(0), COTTAGE, owner).expect("a legal spot");

    assert_eq!(
        state.registry.get::<Drawn>(house).map(|drawn| drawn.id),
        Some(MultiId(COTTAGE).graphic()),
        "the wire carries a house as 0x4000 above its id"
    );
    assert_eq!(state.registry.get::<Position>(house).map(|p| p.0), Some(at));
    assert_eq!(
        state.registry.get::<House>(house),
        Some(&House {
            multi: MultiId(COTTAGE),
            owner,
            co_owners: Default::default(),
            friends: Default::default(),
            bans: Default::default(),
            age: 0,
            // Five drawn tiles at four apiece — see `storage::LOCKDOWNS_PER_TILE`.
            lockdowns: 20,
        })
    );
    // And it is an *item*, so everything that walks items reaches it.
    assert!(
        state.registry.serial_of(house).is_some_and(|s| s.is_item()),
        "a house took a mobile serial"
    );
}

#[test]
fn a_placed_house_is_revealed_to_a_stationary_client_immediately() {
    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    let connection = ConnectionId::from_raw(1);
    state.connections.insert(
        connection,
        Connection::new(
            ClientVersion::new(7, 0, 0, 0),
            AccountName::new("builder"),
            AccessLevel::GameMaster,
        ),
    );
    state.registry.insert(actor, Client { connection });
    let watching_from = Point::new(2, 2, 0);
    state.registry.insert(actor, Position(watching_from));
    state.registry.insert(actor, Facet(0));
    state.place_mobile(Facet(0), actor, watching_from);

    place(&mut state, actor, at, Facet(0), COTTAGE, owner).expect("a legal spot");

    assert_eq!(
        state
            .outbox
            .iter()
            .filter(|packet| packet.packet.first() == Some(&0x1A))
            .count(),
        3,
        "the client should receive the house, its door and its sign without moving"
    );
}

/// The whole point of H1: the walls stop somebody and the doorway does not.
#[test]
fn the_walls_block_and_the_floor_does_not() {
    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    place(&mut state, actor, at, Facet(0), COTTAGE, owner).expect("a legal spot");

    let obstructions = &state.facet_state(Facet(0)).obstructions();
    for (dx, dy) in [(-1, -1), (1, -1), (-1, 1), (1, 1)] {
        let tile = Tile::new((10 + dx) as u16, (10 + dy) as u16);
        assert!(
            obstructions.blocker_at_z(tile.x, tile.y, 0).is_some(),
            "the wall at ({dx}, {dy}) does not stop anybody"
        );
    }
    assert!(
        obstructions.blocker_at_z(10, 10, 0).is_none(),
        "the floor was folded in, which seals the house shut from the inside"
    );
    assert!(
        !obstructions.holds_anything(20, 20),
        "an undrawn component was folded in"
    );
}

#[test]
fn a_house_banister_is_visible_but_does_not_seal_its_floor() {
    let components = vec![
        component(FLOOR, 0, 0, 0, true),
        component(BANISTER, 0, 0, 10, true),
    ];
    let mut state = world_with(components);
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    place(&mut state, actor, at, Facet(0), COTTAGE, owner).expect("a floor with a decorative banister");

    let live = state.facet_state(Facet(0)).ground().live();
    assert!(
        live.blocker_at(Tile::new(at.x, at.y), Body::new(0, 16), Doors::AsTheyStand)
            .is_none(),
        "the banister above the floor must not make the ground-floor tile impassable"
    );
}

/// A house's floor is somewhere to stand, and the wall over it is still in the
/// way.
///
/// **What R3 is for.** `footprint_of` folded in only the components whose
/// tiledata said they blocked, so a house was its walls: no first floor, and a
/// staircase that stopped a body rather than lifting one. The two halves are
/// now one reading of the art — `Cover::of_static` — and this is that reading
/// arriving at the shard's own index.
///
/// The floor is over open ground it does *not* duplicate (`dz = 7`), because
/// the duplicate is a separate case with its own test below.
#[test]
fn a_house_floor_is_a_surface_and_the_wall_over_it_is_not() {
    let mut components = cottage();
    components.push(component(FLOOR, 2, 2, 7, true));
    components.push(component(WALL, 2, 3, 7, true));
    let mut state = world_with(components);
    let (actor, owner) = an_actor(&mut state);
    place(&mut state, actor, Point::new(10, 10, 0), Facet(0), COTTAGE, owner).expect("a legal spot");

    let facet = state.facet_state(Facet(0));
    let ground = facet.ground();
    // The first floor: somewhere to stand at z 7, and nothing in the way there.
    assert_eq!(ground.live().surface_at(Tile::new(12, 12), 0, ANY_REACH), Some(7));
    assert!(
        ground
            .live()
            .blocker_at(Tile::new(12, 12), Body::new(7, 16), Doors::AsTheyStand)
            .is_none(),
        "a body standing on the floor is blocked by the floor"
    );
    // Seven z-units leave less than a body's height below this floor, so the
    // map ground underneath is not another walkable layer. A real second floor
    // sits twenty units above the first and leaves enough headroom naturally.
    assert!(
        ground
            .live()
            .blocker_at(Tile::new(12, 12), Body::new(0, 16), Doors::AsTheyStand)
            .is_some()
    );
    // The wall beside it, at the same height, is in the way and is no surface.
    assert!(
        ground
            .live()
            .blocker_at(Tile::new(12, 13), Body::new(7, 16), Doors::AsTheyStand)
            .is_some(),
        "the upper-storey wall lets a body walk through it"
    );
    assert_eq!(ground.live().surface_at(Tile::new(12, 13), 7, ANY_REACH), None);
}

/// **The risk this node names.** A floor laid exactly on the ground it
/// duplicates must not change where a body stands, and must not seal the room.
///
/// A house's ground floor is a `PLATFORM` of tiledata height zero at `dz = 0`,
/// so its surface is the same z the land already answers with. The failure it
/// guards against is not subtle: a blocking half of height zero reaches one z up
/// (`Cover::top`'s `max(1)`, which is right for a wall), and a body standing on
/// its own ground floor would be inside it.
#[test]
fn a_ground_floor_laid_on_the_ground_seals_nothing() {
    let mut components = cottage();
    components.push(component(FLOOR, 2, 2, 0, true));
    let mut state = world_with(components);
    let (actor, owner) = an_actor(&mut state);
    place(&mut state, actor, Point::new(10, 10, 0), Facet(0), COTTAGE, owner).expect("a legal spot");

    let facet = state.facet_state(Facet(0));
    assert!(
        facet
            .ground()
            .live()
            .blocker_at(Tile::new(12, 12), Body::new(0, 16), Doors::AsTheyStand)
            .is_none(),
        "the ground floor seals the room it is the floor of"
    );
    // And the step onto it lands where it always did — on the ground, at zero,
    // not one unit up on a floor that duplicates it.
    let footing = state.footing(Facet(0), Doors::AsTheyStand);
    assert_eq!(
        openshard_movement::can_step(&footing, Point::new(12, 11, 0), Point::new(12, 12, 0)),
        Some(Point::new(12, 12, 0))
    );
}

/// A house stair is both a solid tread and a surface: a body may take it from
/// its base and then reach the floor above.  This goes through `place` and the
/// facet's live footing, rather than a hand-built overlay, so the components a
/// house actually contributes cannot quietly turn into a picture-only stair.
#[test]
fn a_placed_house_stair_reaches_its_first_floor() {
    let mut components = cottage();
    // Ground at the house origin, then north onto the tread and north again
    // onto the first floor.  A five-high climbable is met at z=2 and stood on
    // at z=4, which puts the z=7 floor within the next step's reach.
    components.push(component(STAIR, 0, -1, 2, true));
    components.push(component(FLOOR, 0, -2, 7, true));
    let mut state = world_with(components);
    let (actor, owner) = an_actor(&mut state);
    place(&mut state, actor, Point::new(10, 10, 0), Facet(0), COTTAGE, owner).expect("a legal house");

    let footing = state.footing(Facet(0), Doors::AsTheyStand);
    let tread = openshard_movement::can_step(&footing, Point::new(10, 10, 0), Point::new(10, 9, 0))
        .expect("the house stair is walkable from its base");
    assert_eq!(tread, Point::new(10, 9, 4));
    assert_eq!(
        openshard_movement::can_step(&footing, tread, Point::new(10, 8, 0)),
        Some(Point::new(10, 8, 7)),
        "the stair does not throw the walker back before the first floor"
    );
}

/// The exact shape of classic multi 0x0076's rejected indoor step: a flat floor
/// and a stair occupy the target tile at the same base z. The floor must not
/// replace the stair's surface in the live obstruction index.
#[test]
fn a_floor_does_not_erase_an_overlapping_house_stair() {
    let mut components = cottage();
    components.push(component(FLOOR, 0, 0, 7, true));
    // Keep this order: the shipped multi lists the stair before the floor, so
    // the old identity rule let this later flat surface overwrite the tread.
    components.push(component(STAIR, 1, 0, 7, true));
    components.push(component(FLOOR, 1, 0, 7, true));
    let mut state = world_with(components);
    let (actor, owner) = an_actor(&mut state);
    place(&mut state, actor, Point::new(10, 10, 0), Facet(0), COTTAGE, owner).expect("a legal house");

    let footing = state.footing(Facet(0), Doors::AsTheyStand);
    assert_eq!(
        openshard_movement::step_allowed(&footing, Point::new(10, 10, 7), Direction::East),
        Some(Point::new(11, 10, 9)),
        "the flat floor erased the tread above it"
    );
}

/// An elevated house floor replaces the usable map layer in its column. From
/// outside at ground z it is too high to climb and too low to walk beneath;
/// once on the house level it behaves as an ordinary floor.
#[test]
fn an_elevated_house_floor_cannot_be_walked_under() {
    let mut components = cottage();
    components.push(component(FLOOR, 1, 0, 7, true));
    components.push(component(FLOOR, 2, 0, 7, true));
    let mut state = world_with(components);
    let (actor, owner) = an_actor(&mut state);
    place(&mut state, actor, Point::new(10, 10, 0), Facet(0), COTTAGE, owner).expect("a legal house");

    let footing = state.footing(Facet(0), Doors::AsTheyStand);
    assert_eq!(
        openshard_movement::step_allowed(&footing, Point::new(11, 10, 0), Direction::East),
        None,
        "the body walked along the map underneath the house floor"
    );
    assert_eq!(
        openshard_movement::step_allowed(&footing, Point::new(11, 10, 7), Direction::East),
        Some(Point::new(12, 10, 7)),
        "the same floor stopped a body already standing on it"
    );
}

/// A wall on the second floor blocks the second floor and leaves the ground
/// open — the reason an obstacle carries a z-span, exercised through a house
/// rather than through the index directly.
#[test]
fn an_upper_storey_wall_leaves_the_ground_floor_open() {
    let mut components = cottage();
    components.push(component(WALL, -1, -1, 20, true));
    let mut state = world_with(components);
    let (actor, owner) = an_actor(&mut state);
    place(&mut state, actor, Point::new(10, 10, 0), Facet(0), COTTAGE, owner).expect("a legal spot");

    let obstructions = &state.facet_state(Facet(0)).obstructions();
    // One tile, one entity, two walls: both must be there. Keyed by the entity
    // alone the second would have overwritten the first.
    assert!(
        obstructions.blocker_at_z(9, 9, 0).is_some(),
        "the ground floor wall"
    );
    assert!(
        obstructions.blocker_at_z(9, 9, 25).is_some(),
        "the upper floor wall"
    );
    // And the storey above both is open sky.
    assert!(obstructions.blocker_at_z(9, 9, 60).is_none());
}

/// Two houses may not stand in each other.
#[test]
fn a_house_will_not_go_where_a_house_already_is() {
    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    place(&mut state, actor, Point::new(10, 10, 0), Facet(0), COTTAGE, owner).expect("a legal spot");
    assert_eq!(
        place(&mut state, actor, Point::new(10, 10, 0), Facet(0), COTTAGE, owner),
        Err(Refusal::Occupied)
    );
    // One tile over, the rings overlap at a corner, so it is still refused.
    assert_eq!(
        place(&mut state, actor, Point::new(12, 10, 0), Facet(0), COTTAGE, owner),
        Err(Refusal::Occupied)
    );
    // Well clear, and it goes up.
    assert!(place(&mut state, actor, Point::new(20, 20, 0), Facet(0), COTTAGE, owner).is_ok());
}

/// The four ways a placement is refused before the ground is even looked at.
#[test]
fn a_multi_nobody_can_build_is_refused_by_name() {
    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);

    assert_eq!(
        place(&mut state, actor, at, Facet(0), 0x0999, owner),
        Err(Refusal::NoSuchMulti),
        "an id the client has never heard of"
    );
    // A foundation is placeable now — but only one whose platform this shard can
    // actually read, because the design is built *out of* that platform. An id
    // inside the range that the client files do not hold has nothing to build
    // from, and the refusal it existed for still stands there.
    assert_eq!(
        place(&mut state, actor, at, Facet(0), FOUNDATION_IDS.end - 1, owner),
        Err(Refusal::NeedsCustomisation),
        "a foundation whose platform this shard cannot read has nothing to build a design from"
    );

    // A multi that is in the table and draws nothing — the treasure-site markers
    // a real file ships five of. Every component of all five is *undrawn*, which
    // is what the fixture says here: a drawn floor is a floor and belongs to the
    // house that has one.
    let mut marker = world_with(vec![component(FLOOR, 0, 0, 0, false)]);
    let (marker_actor, marker_owner) = an_actor(&mut marker);
    assert_eq!(
        place(&mut marker, marker_actor, at, Facet(0), COTTAGE, marker_owner),
        Err(Refusal::DrawsNothing)
    );
}

/// The graphic and the id are the same thing said two ways, and either reaches
/// the same house.
#[test]
fn a_graphic_and_an_id_place_the_same_house() {
    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let from_id = place(&mut state, actor, Point::new(5, 5, 0), Facet(0), COTTAGE, owner).expect("by id");
    let from_graphic = place(
        &mut state,
        actor,
        Point::new(20, 20, 0),
        Facet(0),
        MultiId(COTTAGE).graphic().0,
        owner,
    )
    .expect("by graphic");
    assert_eq!(
        state.registry.get::<House>(from_id).map(|h| h.multi),
        state.registry.get::<House>(from_graphic).map(|h| h.multi)
    );
}

/// A footprint that would hang off the north-west corner is refused rather than
/// wrapping to the far side of the world.
#[test]
fn a_house_at_the_edge_does_not_wrap_around_the_world() {
    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    assert_eq!(
        place(&mut state, actor, Point::new(0, 0, 0), Facet(0), COTTAGE, owner),
        Err(Refusal::OffTheMap),
        "a wall one tile west of x=0 became a wall at 65535"
    );
}

/// Taking the walls back out leaves the ground walkable, which is what a
/// demolition and a moving crate will need.
#[test]
fn unblocking_gives_the_ground_back() {
    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    let house = place(&mut state, actor, at, Facet(0), COTTAGE, owner).expect("a legal spot");
    let footprint = footprint_of(&state, at, MultiId(COTTAGE), None).expect("the same footprint");

    assert_eq!(house_at(&state, at, Facet(0)), Some(house));
    assert_eq!(
        house_at(&state, Point::new(9, 9, 20), Facet(0)),
        Some(house),
        "coverage is an x/y question, not a storey question"
    );
    assert_eq!(house_at(&state, Point::new(8, 8, 0), Facet(0)), None);

    unblock(&mut state, house, Facet(0), &footprint);
    assert!(
        !state.facet_state(Facet(0)).obstructions().holds_anything(9, 9),
        "a wall outlived the house"
    );
    assert_eq!(
        house_at(&state, at, Facet(0)),
        None,
        "unblocking left the house in the coverage index"
    );

    // The walls are gone and the spot is *still* refused, which is the half worth
    // asserting: a house is two facts — the obstructions it holds and the entity
    // that owns a yard — and `unblock` undoes only the first. A demolition that
    // called this and stopped would leave a plot nobody could ever build on.
    assert_eq!(
        place(&mut state, actor, at, Facet(0), COTTAGE, owner),
        Err(Refusal::TooCloseToAHouse)
    );

    state.registry.despawn(house);
    assert!(
        place(&mut state, actor, at, Facet(0), COTTAGE, owner).is_ok(),
        "with the house gone the plot is free again"
    );
}

/// House walls enter the live obstruction layer, so sight has to read their
/// structural span as well as the map's baked statics and shut doors.
#[test]
fn a_house_wall_blocks_line_of_sight() {
    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    place(&mut state, actor, Point::new(10, 10, 0), Facet(0), COTTAGE, owner).expect("a legal house");

    assert!(
        !openshard_movement::sight_clear(
            &state.footing(Facet(0), Doors::AsTheyStand),
            Point::new(8, 9, 0),
            Point::new(11, 9, 0)
        ),
        "the cottage wall at (9, 9) did not block the look"
    );
}

/// The rule a player notices the absence of: without it, houses go up in the
/// middle of Britain's streets.
#[test]
fn a_house_may_not_be_built_on_a_road() {
    // The whole facet is cobbles, which is the cheapest way to put a road under
    // every footprint tile.
    let mut state = ground_of(cottage(), 0x0071, true);
    let (actor, owner) = an_actor(&mut state);
    assert_eq!(
        place(&mut state, actor, Point::new(10, 10, 0), Facet(0), COTTAGE, owner),
        Err(Refusal::OnARoad)
    );

    // The list is ranges, not ids, so both ends and the middle of one must read
    // as road and the tile below it must not.
    assert!(is_road(0x0071) && is_road(0x0075) && is_road(0x0078));
    assert!(!is_road(0x0070), "one below the first range read as a road");
    assert!(is_road(0x3FF4), "the single-id range");
    assert!(is_road(0x0150) && is_road(0x015C), "the second furrow range");
    assert!(!is_road(0x015D));
}

/// Rules two and four, which `can_fit` asks as one question: a solid wall in the
/// way and thin air with no floor are the same refusal.
#[test]
fn ground_that_will_not_take_a_house_refuses_one() {
    let mut state = ground_of(cottage(), 0, false);
    let (actor, owner) = an_actor(&mut state);
    assert_eq!(
        place(&mut state, actor, Point::new(10, 10, 0), Facet(0), COTTAGE, owner),
        Err(Refusal::BadGround)
    );
}

/// **A house with an upper storey goes up on ordinary ground.**
///
/// The rule the sentence above hides: `can_fit` demands a *surface* at the z it
/// is asked about, so asking it at each component's own z means the second floor
/// is standing on thin air — every villa, keep and two-storey shop in the game
/// refused with `BadGround`, everywhere, forever. It went unseen because the
/// fixture terrain answered `can_fit` with a boolean the test set to `true`.
///
/// The ground is asked about the components that rest on it. Everything above
/// the house's own z rests on the house.
#[test]
fn a_second_storey_stands_on_the_house_and_not_on_the_ground() {
    let mut components = cottage();
    // A wall twenty units up, over a tile the ground floor also has a wall on —
    // the shape of every real house with a first floor.
    components.push(component(WALL, -1, -1, 20, true));
    let mut state = ground_of(components, 0, true);
    let (actor, owner) = an_actor(&mut state);

    assert!(
        place(&mut state, actor, Point::new(10, 10, 0), Facet(0), COTTAGE, owner).is_ok(),
        "the ground was asked to hold up a wall standing on the first floor",
    );
}

/// And the ground floor is still asked, so a house on ground that will take
/// nothing is refused however many storeys it has.
#[test]
fn a_second_storey_does_not_excuse_the_ground_floor() {
    let mut components = cottage();
    components.push(component(WALL, -1, -1, 20, true));
    let mut state = ground_of(components, 0, false);
    let (actor, owner) = an_actor(&mut state);

    assert_eq!(
        place(&mut state, actor, Point::new(10, 10, 0), Facet(0), COTTAGE, owner),
        Err(Refusal::BadGround),
    );
}

/// Every house keeps five tiles to itself, and the yard is measured against the
/// other house's *footprint* rather than a stored rectangle.
#[test]
fn a_house_keeps_a_yard_clear_of_other_houses() {
    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    place(&mut state, actor, Point::new(10, 10, 0), Facet(0), COTTAGE, owner).expect("the first house");

    // The yard is measured wall to wall, not origin to origin, and that is the
    // arithmetic worth pinning. The first cottage's east wall is at x=11; a
    // second at origin 17 puts its west wall at 16, five tiles away and so
    // *inside* the yard. Origin 18 puts it at 17, six away, and clear.
    assert_eq!(
        place(&mut state, actor, Point::new(17, 10, 0), Facet(0), COTTAGE, owner),
        Err(Refusal::TooCloseToAHouse),
        "two walls five tiles apart is inside a yard of five"
    );
    assert!(
        place(&mut state, actor, Point::new(18, 10, 0), Facet(0), COTTAGE, owner).is_ok(),
        "a house six tiles clear of another was refused"
    );
}

/// A shard with no client files places nothing rather than placing something
/// with no walls — the same bargain the terrain makes by allowing every step.
///
/// **The multi table and not the terrain.** This used to clear
/// `FacetState::terrain`, back when a multi's components were asked of the
/// ground it stood on. They are the shard's table now, so a facet with a map and
/// no multi table is expressible — and it is this refusal, not the ground, that
/// answers for it.
#[test]
fn a_world_with_no_multi_table_has_no_houses() {
    let mut state = world_with(cottage());
    state.multis = Multis::default();
    let (actor, owner) = an_actor(&mut state);
    assert_eq!(
        place(&mut state, actor, Point::new(10, 10, 0), Facet(0), COTTAGE, owner),
        Err(Refusal::NoSuchMulti)
    );
}

/// A fresh house with one owner and nobody else in it.
fn a_house(owner: Serial) -> House {
    House {
        multi: MultiId(COTTAGE),
        owner,
        co_owners: Default::default(),
        friends: Default::default(),
        bans: Default::default(),
        age: 0,
        lockdowns: 20,
    }
}

fn somebody(n: u32) -> Serial {
    Serial::new(n).expect("a serial")
}

/// The reference's rules are nested — a co-owner is a friend, an owner is a
/// co-owner — and asking them as one question is what stops the wrong one being
/// asked.
#[test]
fn standing_is_one_question_with_a_nested_answer() {
    let owner = somebody(1);
    let mut house = a_house(owner);
    let friend = somebody(2);
    let co_owner = somebody(3);
    let stranger = somebody(4);
    house.friends.insert(friend);
    house.co_owners.insert(co_owner);

    assert_eq!(house.standing_of(owner, false), Standing::Owner);
    assert_eq!(house.standing_of(co_owner, false), Standing::CoOwner);
    assert_eq!(house.standing_of(friend, false), Standing::Friend);
    assert_eq!(house.standing_of(stranger, false), Standing::Stranger);
    // The order is what makes "at least this trusted" a comparison.
    assert!(Standing::Owner > Standing::CoOwner);
    assert!(Standing::CoOwner > Standing::Friend);
    assert!(Standing::Friend > Standing::Stranger);
    assert!(Standing::Stranger > Standing::Banned);
}

/// Nobody bans the owner out of their own house, and staff are never turned
/// away — both are the reference's own first branches.
#[test]
fn the_owner_and_staff_cannot_be_banned() {
    let owner = somebody(1);
    let mut house = a_house(owner);
    let staffer = somebody(2);
    house.bans.insert(owner);
    house.bans.insert(staffer);

    assert_eq!(
        house.standing_of(owner, false),
        Standing::Owner,
        "the owner was banned from their own house"
    );
    assert_eq!(
        house.standing_of(staffer, true),
        Standing::CoOwner,
        "a game master was turned away"
    );
    // And the same mobile without the authority *is* banned.
    assert_eq!(house.standing_of(staffer, false), Standing::Banned);
}

/// Only the owner names a co-owner. A co-owner who could would be handing the
/// house to a crowd the owner never met.
#[test]
fn a_co_owner_may_name_friends_and_not_co_owners() {
    let owner = somebody(1);
    let mut house = a_house(owner);
    let co_owner = somebody(2);
    trust(&mut house, owner, co_owner, Standing::CoOwner, false).expect("the owner may");

    let newcomer = somebody(3);
    assert_eq!(
        trust(&mut house, co_owner, newcomer, Standing::Friend, false),
        Ok(()),
        "a co-owner may name a friend"
    );
    assert_eq!(
        trust(&mut house, co_owner, somebody(4), Standing::CoOwner, false),
        Err(ListRefusal::NotYours)
    );
    // And a friend may name nobody at all.
    assert_eq!(
        trust(&mut house, newcomer, somebody(5), Standing::Friend, false),
        Err(ListRefusal::NotYours)
    );
}

/// Promotion **moves** somebody rather than adding them twice: two lists holding
/// one person is two answers to one question.
#[test]
fn promoting_a_friend_leaves_them_in_one_list() {
    let owner = somebody(1);
    let mut house = a_house(owner);
    let who = somebody(2);
    trust(&mut house, owner, who, Standing::Friend, false).unwrap();
    trust(&mut house, owner, who, Standing::CoOwner, false).unwrap();

    assert!(house.co_owners.contains(&who));
    assert!(
        !house.friends.contains(&who),
        "they are in both lists, so which one answers depends on check order"
    );
    assert_eq!(house.standing_of(who, false), Standing::CoOwner);
}

/// A ban is the newer decision and it wins: "banned but still a co-owner" is a
/// state with no useful answer.
#[test]
fn banning_a_co_owner_takes_the_trust_with_it() {
    let owner = somebody(1);
    let mut house = a_house(owner);
    let turncoat = somebody(2);
    trust(&mut house, owner, turncoat, Standing::CoOwner, false).unwrap();

    ban(&mut house, owner, turncoat, false).expect("the owner may");
    assert_eq!(house.standing_of(turncoat, false), Standing::Banned);
    assert!(!house.co_owners.contains(&turncoat));

    // Lifting it gives back a stranger, not a co-owner: undoing a ban grants
    // nothing.
    unban(&mut house, owner, turncoat, false).expect("the owner may");
    assert_eq!(house.standing_of(turncoat, false), Standing::Stranger);
}

/// The owner is not a name on any list and cannot be dropped from one.
#[test]
fn nobody_evicts_the_owner() {
    let owner = somebody(1);
    let mut house = a_house(owner);
    let co_owner = somebody(2);
    trust(&mut house, owner, co_owner, Standing::CoOwner, false).unwrap();

    // `NotTheOwner` and not `NotYours`: a co-owner *may* drop friends, so the
    // refusal is about who was named rather than about who asked, and saying so
    // is the difference between a usable message and a puzzling one.
    assert_eq!(
        distrust(&mut house, co_owner, owner, false),
        Err(ListRefusal::NotTheOwner)
    );
    assert_eq!(
        ban(&mut house, co_owner, owner, false),
        Err(ListRefusal::NotTheOwner)
    );
    // Only the owner drops a co-owner.
    assert_eq!(
        distrust(&mut house, co_owner, co_owner, false),
        Err(ListRefusal::NotYours)
    );
    assert_eq!(distrust(&mut house, owner, co_owner, false), Ok(()));
    assert_eq!(house.standing_of(co_owner, false), Standing::Stranger);
}

/// The lists have ceilings, and re-adding somebody already on one is not a new
/// name.
#[test]
fn a_full_list_refuses_a_new_name_and_takes_an_old_one() {
    let owner = somebody(1);
    let mut house = a_house(owner);
    for n in 0..MAX_CO_OWNERS as u32 {
        trust(&mut house, owner, somebody(100 + n), Standing::CoOwner, false).expect("under the ceiling");
    }
    assert_eq!(
        trust(&mut house, owner, somebody(999), Standing::CoOwner, false),
        Err(ListRefusal::Full)
    );
    assert_eq!(
        trust(&mut house, owner, somebody(100), Standing::CoOwner, false),
        Ok(()),
        "somebody already on the list is not a new name"
    );
}

/// A door standing inside a house becomes the house's, and the house's rules
/// then decide who may work it.
///
/// The multi usually cannot be the source — only three of a shipped file's 326
/// carry a door component at all — so the general rule is the one a player
/// would state.
#[test]
fn a_house_adopts_the_doors_standing_inside_it() {
    use openshard_state::components::{
        Door,
        HouseDoor,
    };

    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);

    // One door where the house will stand, and one well outside it.
    let inside = door_at(&mut state, Point::new(10, 10, 0));
    let outside = door_at(&mut state, Point::new(25, 25, 0));

    let house = place(&mut state, actor, at, Facet(0), COTTAGE, owner).expect("a legal spot");
    let serial = state.registry.serial_of(house).unwrap();

    assert_eq!(
        state.registry.get::<HouseDoor>(inside).map(|d| d.house),
        Some(serial),
        "the door in the doorway is not the house's"
    );
    assert!(
        !state.registry.has::<HouseDoor>(outside),
        "a door in the next field was adopted"
    );
    // And it is still an ordinary door in every other respect.
    assert!(state.registry.has::<Door>(inside));
}

/// The classic multi is only the house's picture. Its functional door is a
/// separately placed item at the offset the house type defines.
#[test]
fn a_classic_house_places_its_door() {
    use openshard_state::components::{
        Door,
        HouseDoor,
    };

    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    let house = place(&mut state, actor, at, Facet(0), COTTAGE, owner).expect("a legal spot");
    let house_serial = state.registry.serial_of(house).unwrap();

    let doors: Vec<_> = state
        .registry
        .query::<HouseDoor>()
        .filter(|(_, door)| door.house == house_serial)
        .map(|(entity, _)| entity)
        .collect();
    assert_eq!(doors.len(), 1, "the cottage did not get exactly one door");
    let door = doors[0];
    assert_eq!(
        state.registry.get::<Position>(door).map(|position| position.0),
        Some(Point::new(10, 13, 7)),
        "the door does not stand in the cottage's entrance"
    );
    assert_eq!(
        state.registry.get::<Drawn>(door).map(|drawn| drawn.id),
        Some(Graphic(0x06A5)),
        "the wooden west-hinged leaf draws as the wrong art"
    );
    assert!(
        state.registry.has::<Door>(door),
        "the leaf is only decorative art"
    );
    assert!(
        state
            .facet_state(Facet(0))
            .ground()
            .live()
            .blocker_at(Tile::new(10, 13), Body::new(7, 16), Doors::AsTheyStand)
            .is_some(),
        "the shut door does not close the entrance"
    );
}

#[test]
fn an_imported_design_turns_its_closed_leaf_into_a_functional_door() {
    use openshard_state::components::{
        Door,
        HouseDoor,
    };

    const FOUNDATION: u16 = 0x13EC;
    let mut state = world_with_extra_multi(Multi::new(FOUNDATION, cottage()));
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    let house = place(&mut state, actor, at, Facet(0), FOUNDATION, owner).expect("a usable foundation");
    let mut imported = cottage();
    imported.push(Component {
        // A light-wood door: this family appears in the legacy cathedral and
        // is not one of the classic house fixture families.
        graphic: Graphic(0x06BD),
        dx:      2,
        dy:      1,
        dz:      7,
        flags:   1,
    });

    design::redesign(&mut state, actor, house, imported).expect("a non-empty imported design");
    install_doors(&mut state, house, Facet(0), at, MultiId(FOUNDATION));

    let house_serial = state.registry.serial_of(house).unwrap();
    let door = state
        .registry
        .query::<HouseDoor>()
        .find(|(_, entry)| entry.house == house_serial)
        .map(|(entity, _)| entity)
        .expect("the imported closed leaf became a door entity");
    assert_eq!(
        state.registry.get::<Position>(door).map(|position| position.0),
        Some(Point::new(12, 11, 7))
    );
    assert_eq!(
        state
            .registry
            .get::<Door>(door)
            .map(|door| (door.closed, door.open)),
        Some((Graphic(0x06BD), Graphic(0x06BE)))
    );
}

/// The alternate small-house multis are still classic houses: a player who
/// chooses one must not lose its declared door and sign merely because its id
/// is the odd member of the art pair.
#[test]
fn a_classic_house_variant_keeps_its_shared_fixtures() {
    use openshard_state::components::{
        HouseDoor,
        HouseSign,
    };

    let mut state = world_with_extra_multi(Multi::new(0x0065, cottage()));
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    let house = place(&mut state, actor, at, Facet(0), 0x0065, owner).expect("a legal variant house");
    let serial = state.registry.serial_of(house).unwrap();

    assert!(
        state
            .registry
            .query::<HouseDoor>()
            .any(|(_, door)| door.house == serial)
    );
    assert!(state.registry.query::<HouseSign>().any(|(entity, sign)| {
        sign.house == serial
            && state.registry.get::<Position>(entity).map(|position| position.0)
                == Some(Point::new(12, 14, 5))
    }));
}

/// `0x0087` supplies its own visible house sign and closed door leaves. Those
/// art components are the authoritative frames for the functional fixtures.
#[test]
fn an_embedded_house_sign_and_doors_become_functional_fixtures() {
    use openshard_state::components::{
        HouseDoor,
        HouseSign,
    };

    let mut state = world_with_extra_multi(Multi::new(
        0x0087,
        vec![
            component(1, 0, 0, 0, false),
            component(WALL, 0, 0, 0, true),
            component(0x0BD2, 0, 7, -2, true),
            component(0x06A7, 4, -1, 0, true),
            component(0x06A5, -4, 6, 0, true),
            component(0x06A7, -3, 6, 0, true),
        ],
    ));
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    let house = place(&mut state, actor, at, Facet(0), 0x0087, owner).expect("a legal content house");
    let serial = state.registry.serial_of(house).unwrap();

    let doors: Vec<_> = state
        .registry
        .query::<HouseDoor>()
        .filter(|(_, door)| door.house == serial)
        .map(|(entity, _)| state.registry.get::<Position>(entity).unwrap().0)
        .collect();
    assert_eq!(
        doors.len(),
        3,
        "every closed leaf in the multi must become a door"
    );
    assert!(doors.contains(&Point::new(6, 16, 0)));
    assert!(doors.contains(&Point::new(7, 16, 0)));
    assert!(doors.contains(&Point::new(14, 9, 0)));
    assert!(state.registry.query::<HouseSign>().any(|(entity, sign)| {
        sign.house == serial
            && state.registry.get::<Position>(entity).map(|position| position.0)
                == Some(Point::new(10, 17, -2))
    }));
}

/// An imported design's plaque is not a request to hang a second sign at the
/// generic foundation corner: it is the exact attachment point for the live
/// sign that replaces the decorative component in the design packet.
#[test]
fn an_imported_design_keeps_its_embedded_sign_position() {
    use openshard_state::components::HouseSign;

    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    let imported = vec![component(WALL, 0, 0, 0, true), component(0x0BD1, 3, 4, 7, true)];
    state.house_templates.insert("with-sign".into(), imported.clone());
    let house = super::place_design(
        &mut state,
        actor,
        at,
        Facet(0),
        MultiId(FOUNDATION),
        owner,
        imported,
    )
    .expect("an imported house with a declared sign");
    let serial = state.registry.serial_of(house).unwrap();

    assert!(state.registry.query::<HouseSign>().any(|(entity, sign)| {
        sign.house == serial
            && state.registry.get::<Position>(entity).map(|position| position.0)
                == Some(Point::new(13, 14, 7))
            && state.registry.get::<Drawn>(entity).map(|drawn| drawn.id) == Some(Graphic(0x0BD1))
    }));
}

/// The legacy custom-house pack marks its usable sign as a `metal signpost`,
/// not as either of the conventional house-sign graphics.  It remains at the
/// declared component position; the server replaces only its representation
/// with a live `HouseSign` so use opens the house menu.
#[test]
fn an_imported_design_can_use_a_metal_signpost_as_its_house_sign() {
    use openshard_state::components::HouseSign;

    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    let imported = vec![component(WALL, 0, 0, 0, true), component(0x0B9E, 2, 6, 0, true)];
    state
        .house_templates
        .insert("metal-signpost".into(), imported.clone());
    let house = super::place_design(
        &mut state,
        actor,
        at,
        Facet(0),
        MultiId(FOUNDATION),
        owner,
        imported,
    )
    .expect("an imported house with a metal signpost");
    let serial = state.registry.serial_of(house).unwrap();

    assert!(state.registry.query::<HouseSign>().any(|(entity, sign)| {
        sign.house == serial
            && state.registry.get::<Position>(entity).map(|position| position.0)
                == Some(Point::new(12, 16, 0))
            && state.registry.get::<Drawn>(entity).map(|drawn| drawn.id) == Some(Graphic(0x0B9E))
    }));
}

#[test]
fn an_imported_design_without_a_plaque_does_not_gain_a_corner_sign() {
    use openshard_state::components::HouseSign;

    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let imported = vec![component(WALL, 0, 0, 0, true)];
    state
        .house_templates
        .insert("without-sign".into(), imported.clone());

    super::place_design(
        &mut state,
        actor,
        Point::new(10, 10, 0),
        Facet(0),
        MultiId(FOUNDATION),
        owner,
        imported,
    )
    .expect("a signless imported house");

    assert!(state.registry.query::<HouseSign>().next().is_none());
}

#[test]
fn a_classic_houses_door_comes_down_with_it() {
    use openshard_state::components::HouseDoor;

    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let house =
        place(&mut state, actor, Point::new(10, 10, 0), Facet(0), COTTAGE, owner).expect("a legal spot");
    let door = state
        .registry
        .query::<HouseDoor>()
        .map(|(entity, _)| entity)
        .next()
        .expect("the cottage's door");

    decay::demolish(&mut state, house).expect("a readable classic house");

    assert!(
        state.registry.serial_of(door).is_none(),
        "the demolished house left its door behind"
    );
    assert!(
        state
            .facet_state(Facet(0))
            .ground()
            .live()
            .blocker_at(Tile::new(10, 13), Body::new(7, 16), Doors::AsTheyStand)
            .is_none(),
        "the removed door left an invisible obstruction"
    );
}

/// A door with no house opens for anyone, which is every door in Britannia.
fn door_at(state: &mut WorldState, at: Point) -> EntityId {
    use openshard_state::components::Door;
    let (entity, _) = state
        .registry
        .spawn_with_serial(openshard_protocol::serial::SerialKind::Item)
        .unwrap();
    state.registry.insert(entity, Position(at));
    state.registry.insert(entity, Facet(0));
    state.registry.insert(
        entity,
        Door {
            closed:   Graphic(0x06A5),
            open:     Graphic(0x06A6),
            offset_x: 1,
            offset_y: 0,
            link:     None,
            is_open:  false,
            close_at: openshard_state::WorldTick::ZERO,
        },
    );
    entity
}

/// A ban that only locked the door would leave whoever was already inside there
/// for good. This is the rule that makes one worth anything.
#[test]
fn a_ban_puts_out_whoever_is_already_inside() {
    use openshard_state::components::Body;

    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    let house = place(&mut state, actor, at, Facet(0), COTTAGE, owner).expect("a legal spot");

    // Three people standing in the doorway tile: the owner, a friend, and one
    // about to be banned.
    let inside = [owner, an_owner(&mut state), an_owner(&mut state)];
    for who in inside {
        let entity = state.registry.entity_of(who).expect("a mobile");
        state.registry.insert(entity, Position(at));
        state.registry.insert(entity, Facet(0));
        state.registry.insert(
            entity,
            Body {
                id:  Graphic(0x0190),
                hue: openshard_protocol::wire::Hue(0),
            },
        );
    }
    let friend = inside[1];
    let unwelcome = inside[2];
    {
        let entry = state.registry.get_mut::<House>(house).expect("its component");
        trust(entry, owner, friend, Standing::Friend, false).unwrap();
        ban(entry, owner, unwelcome, false).unwrap();
    }

    let moved = evict_the_banned(&mut state, house);
    assert_eq!(
        moved.len(),
        1,
        "the wrong number of people were put out: {moved:?}"
    );

    let where_of = |serial: Serial| {
        let entity = state.registry.entity_of(serial).unwrap();
        state.registry.get::<Position>(entity).unwrap().0
    };
    assert_eq!(where_of(owner), at, "the owner was put out of their own house");
    assert_eq!(where_of(friend), at, "a friend was put out");
    assert_ne!(where_of(unwelcome), at, "the banned player stayed inside");
    // Just outside the box's west edge, which is where the doorstep is.
    assert_eq!(
        where_of(unwelcome),
        doorstep(&state, Facet(0), at, MultiId(COTTAGE))
    );
}

/// A classic house uses the sign offset its house type declares, rather than
/// deriving one from the bounds of the multi it draws.
#[test]
fn a_classic_house_hangs_its_sign_at_its_declared_offset() {
    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    let house = place(&mut state, actor, at, Facet(0), COTTAGE, owner).expect("a legal spot");
    let serial = state.registry.serial_of(house).expect("the house's serial");

    assert_eq!(
        sign_spot(&state, at, MultiId(COTTAGE), None),
        Some(Point::new(12, 14, 5))
    );
    let signs: Vec<_> = state
        .registry
        .query::<openshard_state::components::HouseSign>()
        .map(|(entity, sign)| (entity, sign.house))
        .collect();
    assert_eq!(signs.len(), 1, "a house got {} signs", signs.len());
    assert_eq!(signs[0].1, serial, "the sign names another house");
    assert_eq!(
        state.registry.get::<Position>(signs[0].0).map(|p| p.0),
        Some(Point::new(12, 14, 5))
    );
    assert_eq!(
        state.registry.get::<Drawn>(signs[0].0).map(|drawn| drawn.id),
        Some(Graphic(SIGN_GRAPHIC))
    );
    // And it is an item, so the interest sweep announces it like any other.
    assert!(
        state
            .registry
            .serial_of(signs[0].0)
            .is_some_and(|serial| serial.is_item()),
        "the sign took a mobile serial"
    );
}

/// Regression for the guild house reported at 1340,1894: its box starts seven
/// tiles west, but its plaque belongs four east, eight south and sixteen up.
#[test]
fn a_guild_house_sign_is_beside_its_entrance_not_at_its_box_corner() {
    let state = world_with(cottage());
    assert_eq!(
        sign_spot(&state, Point::new(1340, 1894, 0), MultiId(0x0074), None),
        Some(Point::new(1344, 1902, 16))
    );
}

/// A shard with no client files gets a house with no sign, rather than a sign
/// at the origin.
///
/// The same bargain the walls make. A sign hung at the house's own tile would be
/// *inside* it on every multi whose box is not centred, and a plaque a player
/// cannot reach is worse than no plaque.
#[test]
fn a_house_with_no_multi_table_hangs_no_sign() {
    let mut state = world_with(cottage());
    let owner = an_owner(&mut state);
    assert_eq!(
        sign_spot(&state, Point::new(10, 10, 0), MultiId(COTTAGE + 1), None),
        None,
        "an id the table does not hold got a spot anyway"
    );
    let _ = owner;
}

/// The sign's window is a window over the five verbs, and it obeys them.
///
/// The row is the half a cursor cannot do — taking somebody *off* a list — so
/// this is the branch worth pinning: a friend pressing a co-owner's row changes
/// nothing, and the co-owner pressing it does.
#[test]
fn only_a_co_owner_may_drop_a_name_from_the_window() {
    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    let house = place(&mut state, actor, at, Facet(0), COTTAGE, owner).expect("a legal spot");

    let friend = an_owner(&mut state);
    let co_owner = an_owner(&mut state);
    {
        let entry = state.registry.get_mut::<House>(house).expect("its component");
        trust(entry, owner, friend, Standing::Friend, false).unwrap();
        trust(entry, owner, co_owner, Standing::CoOwner, false).unwrap();
    }

    // A friend pressing the row: refused, because `distrust` asks for co-owner.
    let as_friend = state.registry.entity_of(friend).expect("a mobile");
    sign::apply(
        &mut state,
        as_friend,
        house,
        openshard_state::HouseChange::Drop,
        co_owner,
    );
    assert!(
        state
            .registry
            .get::<House>(house)
            .is_some_and(|entry| entry.co_owners.contains(&co_owner)),
        "a friend dropped a co-owner"
    );

    // The co-owner pressing the friend's row: done.
    let as_co_owner = state.registry.entity_of(co_owner).expect("a mobile");
    sign::apply(
        &mut state,
        as_co_owner,
        house,
        openshard_state::HouseChange::Drop,
        friend,
    );
    assert!(
        state
            .registry
            .get::<House>(house)
            .is_some_and(|entry| !entry.friends.contains(&friend)),
        "a co-owner could not drop a friend"
    );
}

/// A row button reads back as the row it was drawn for, and a number past the
/// end reads back as nothing.
#[test]
fn a_row_button_reads_back_as_the_row_it_was_drawn_for() {
    for row in 0..8 {
        assert_eq!(sign::row_of(sign::row_button(row), 8), Some(row));
    }
    assert_eq!(
        sign::row_of(sign::row_button(8), 8),
        None,
        "a reply naming row nine of an eight-row list resolved to something"
    );
    assert_eq!(
        sign::row_of(sign::button::BAN, 8),
        None,
        "an action button was read as a row"
    );
}

/// A house's ceiling is its own footprint at four apiece, computed once and
/// stored.
///
/// The cottage draws five tiles — four walls and a floor — so twenty lockdowns
/// and forty of storage. The number on the component rather than a recomputation
/// is the half worth pinning: the drop path reads it with no terrain in hand.
#[test]
fn a_house_gets_its_allowance_from_its_own_footprint() {
    use crate::storage::{
        LOCKDOWNS_PER_TILE,
        allowance,
        allowance_for,
    };

    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    let house = place(&mut state, actor, at, Facet(0), COTTAGE, owner).expect("a legal spot");

    let tiles = tiles_of(&state, at, MultiId(COTTAGE), None).len();
    assert_eq!(tiles, 5, "the cottage draws five tiles");
    assert_eq!(
        state.registry.get::<House>(house).map(|entry| entry.lockdowns),
        Some((tiles * LOCKDOWNS_PER_TILE) as u32)
    );
    assert_eq!(allowance(&state, house), allowance_for(tiles));
    assert_eq!(allowance(&state, house).lockdowns, 20);
    assert_eq!(allowance(&state, house).storage, 40);
}

/// Lock down, secure, release — and the three rules that decide each.
#[test]
fn only_a_co_owner_pins_and_only_inside_the_house() {
    use crate::storage::{
        StorageRefusal,
        lock_down,
        locked_down,
        release,
    };

    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    let house = place(&mut state, actor, at, Facet(0), COTTAGE, owner).expect("a legal spot");
    let master = state.registry.entity_of(owner).expect("a mobile");

    // A chest on the house's own floor, and a barrel two tiles outside it.
    let inside = an_item(&mut state, at, true);
    let outside = an_item(&mut state, Point::new(at.x + 8, at.y, at.z), true);

    let stranger = an_owner(&mut state);
    let stranger = state.registry.entity_of(stranger).expect("a mobile");
    assert_eq!(
        lock_down(&mut state, stranger, house, inside, None),
        Err(StorageRefusal::NotYours),
        "a stranger pinned something in somebody else's house"
    );
    assert_eq!(
        lock_down(&mut state, master, house, outside, None),
        Err(StorageRefusal::NotInThisHouse),
        "a thing on the grass was locked down in the house"
    );
    assert_eq!(lock_down(&mut state, master, house, inside, None), Ok(()));
    assert_eq!(locked_down(&state, house), vec![inside]);
    assert_eq!(
        lock_down(&mut state, master, house, inside, None),
        Err(StorageRefusal::NoChange),
        "pinning the same item twice counted twice"
    );

    // A secure has to be a container, and the same item becomes one for free —
    // it is already on the list.
    let plank = an_item(&mut state, at, false);
    assert_eq!(
        lock_down(&mut state, master, house, plank, Some(Standing::Friend)),
        Err(StorageRefusal::NotAContainer)
    );
    assert_eq!(
        lock_down(&mut state, master, house, inside, Some(Standing::Friend)),
        Ok(())
    );
    assert_eq!(
        locked_down(&state, house).len(),
        1,
        "making a lockdown secure spent a second slot"
    );

    assert_eq!(release(&mut state, master, house, inside), Ok(()));
    assert!(locked_down(&state, house).is_empty());
    assert_eq!(
        release(&mut state, master, house, inside),
        Err(StorageRefusal::NoChange)
    );
}

/// The allowance is a ceiling and the ceiling refuses.
#[test]
fn a_full_house_takes_no_more_lockdowns() {
    use crate::storage::{
        StorageRefusal,
        allowance,
        lock_down,
    };

    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    let house = place(&mut state, actor, at, Facet(0), COTTAGE, owner).expect("a legal spot");
    let master = state.registry.entity_of(owner).expect("a mobile");

    let ceiling = allowance(&state, house).lockdowns;
    for _ in 0..ceiling {
        let item = an_item(&mut state, at, false);
        assert_eq!(lock_down(&mut state, master, house, item, None), Ok(()));
    }
    let one_too_many = an_item(&mut state, at, false);
    assert_eq!(
        lock_down(&mut state, master, house, one_too_many, None),
        Err(StorageRefusal::NoRoom)
    );
}

/// A secure opens by standing, and every other container in Britannia opens for
/// anybody.
#[test]
fn a_secure_opens_for_the_standing_it_names() {
    use crate::storage::{
        lock_down,
        may_open,
    };

    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    let house = place(&mut state, actor, at, Facet(0), COTTAGE, owner).expect("a legal spot");
    let master = state.registry.entity_of(owner).expect("a mobile");

    let chest = an_item(&mut state, at, true);
    let plain = an_item(&mut state, at, true);
    lock_down(&mut state, master, house, chest, Some(Standing::CoOwner)).unwrap();

    let friend = an_owner(&mut state);
    let stranger = an_owner(&mut state);
    {
        let entry = state.registry.get_mut::<House>(house).expect("its component");
        trust(entry, owner, friend, Standing::Friend, false).unwrap();
    }
    let friend = state.registry.entity_of(friend).expect("a mobile");
    let stranger = state.registry.entity_of(stranger).expect("a mobile");

    assert!(may_open(&state, master, chest), "the owner was shut out");
    assert!(
        !may_open(&state, friend, chest),
        "a friend opened a co-owners' secure"
    );
    assert!(!may_open(&state, stranger, chest));
    assert!(
        may_open(&state, stranger, plain),
        "an ordinary chest refused a stranger"
    );

    // And "anyone" means the bottom of the ladder, not the absence of one: a
    // banned player is still below it.
    lock_down(&mut state, master, house, chest, Some(Standing::Stranger)).unwrap();
    assert!(may_open(&state, stranger, chest));
    let banned = state.registry.serial_of(stranger).unwrap();
    {
        let entry = state.registry.get_mut::<House>(house).expect("its component");
        ban(entry, owner, banned, false).unwrap();
    }
    assert!(
        !may_open(&state, stranger, chest),
        "a banned player opened a secure standing open to anyone"
    );
}

/// The storage ceiling counts what is in the secures, one level deep.
#[test]
fn the_storage_ceiling_counts_what_is_in_the_secures() {
    use crate::storage::{
        allowance,
        has_room_for,
        lock_down,
        stored,
    };

    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    let house = place(&mut state, actor, at, Facet(0), COTTAGE, owner).expect("a legal spot");
    let master = state.registry.entity_of(owner).expect("a mobile");

    let chest = an_item(&mut state, at, true);
    lock_down(&mut state, master, house, chest, Some(Standing::Friend)).unwrap();
    let chest_serial = state.registry.serial_of(chest).unwrap();

    assert_eq!(stored(&state, house), 0);
    for _ in 0..3 {
        let item = an_item(&mut state, at, false);
        openshard_state::relocate_item(
            &mut state,
            item,
            openshard_state::ItemLocation::contained(openshard_state::components::Contained {
                container: chest_serial,
                position:  openshard_protocol::gump::GumpPoint::new(0, 0),
                grid:      openshard_protocol::containers::GridSlot(0),
            }),
        )
        .unwrap();
    }
    assert_eq!(stored(&state, house), 3);
    assert!(has_room_for(&state, house, allowance(&state, house).storage - 3));
    assert!(
        !has_room_for(&state, house, allowance(&state, house).storage - 2),
        "the ceiling let one past it"
    );
}

/// House search sees only recursive descendants of same-house lockdown roots,
/// partitions them by secure standing, and returns no mutation authority.
#[test]
fn house_inventory_search_is_recursive_permissioned_and_revalidated() {
    use openshard_protocol::containers::GridSlot;
    use openshard_protocol::gump::GumpPoint;
    use openshard_state::components::{
        Amount,
        Contained,
    };
    use openshard_state::{
        HouseInventoryError,
        HouseItemIdentity,
    };

    use crate::inventory::{
        SearchRefusal,
        resolve,
        search,
    };
    use crate::storage::{
        lock_down,
        release,
    };

    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    let house = place(&mut state, actor, at, Facet(0), COTTAGE, owner).expect("a legal spot");
    state.registry.insert(actor, Position(at));
    state.registry.insert(actor, Facet(0));

    let chest = an_item(&mut state, at, true);
    lock_down(&mut state, actor, house, chest, Some(Standing::Friend)).unwrap();
    let chest_serial = state.registry.serial_of(chest).unwrap();
    let bag = an_item(&mut state, at, true);
    state.registry.insert(
        bag,
        Drawn {
            id:  Graphic(0x0E76),
            hue: Hue::NONE,
        },
    );
    openshard_state::relocate_item(
        &mut state,
        bag,
        openshard_state::ItemLocation::contained(Contained {
            container: chest_serial,
            position:  GumpPoint::new(1, 1),
            grid:      GridSlot(0),
        }),
    )
    .unwrap();
    let bag_serial = state.registry.serial_of(bag).unwrap();
    let pile = an_item(&mut state, at, false);
    let gold = Drawn {
        id:  Graphic(0x0EED),
        hue: Hue::NONE,
    };
    state.registry.insert(pile, gold);
    state.registry.insert(pile, Amount(7));
    openshard_state::relocate_item(
        &mut state,
        pile,
        openshard_state::ItemLocation::contained(Contained {
            container: bag_serial,
            position:  GumpPoint::new(2, 2),
            grid:      GridSlot(0),
        }),
    )
    .unwrap();

    let identity = HouseItemIdentity::Legacy {
        graphic: gold.id,
        hue:     gold.hue,
    };
    assert!(matches!(
        search(&state, actor, None, &[identity], None, 10),
        Err(SearchRefusal::Index(HouseInventoryError::Unavailable { .. }))
    ));
    state.advance_house_inventory_rebuilds(256);
    let page = search(&state, actor, None, &[identity], None, 10).expect("the owner may search");
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.rows[0].aggregate_total, 7);
    assert_eq!(page.rows[0].root, chest_serial);
    assert_eq!(page.rows[0].first_pile, state.registry.serial_of(pile).unwrap());
    assert_eq!(
        resolve(
            &state,
            actor,
            page.epoch,
            identity,
            chest_serial,
            page.rows[0].first_pile,
        ),
        Some(pile)
    );

    let stranger_serial = an_owner(&mut state);
    let stranger = state.registry.entity_of(stranger_serial).unwrap();
    state.registry.insert(stranger, Position(at));
    state.registry.insert(stranger, Facet(0));
    assert!(
        search(&state, stranger, None, &[identity], None, 10)
            .expect("an empty permitted search is still available")
            .rows
            .is_empty()
    );
    state
        .registry
        .get_mut::<House>(house)
        .unwrap()
        .friends
        .insert(stranger_serial);
    assert_eq!(
        search(&state, stranger, None, &[identity], None, 10)
            .unwrap()
            .rows[0]
            .root_total,
        7
    );
    state
        .registry
        .get_mut::<House>(house)
        .unwrap()
        .bans
        .insert(stranger_serial);
    assert_eq!(
        search(&state, stranger, None, &[identity], None, 10),
        Err(SearchRefusal::Banned)
    );

    release(&mut state, actor, house, chest).unwrap();
    assert_eq!(
        resolve(
            &state,
            actor,
            page.epoch,
            identity,
            chest_serial,
            page.rows[0].first_pile,
        ),
        None,
        "a released root remained usable through its old result"
    );
    state.advance_house_inventory_rebuilds(256);
    assert!(
        search(&state, actor, None, &[identity], None, 10)
            .unwrap()
            .rows
            .is_empty()
    );
}

/// A canonical containment change invalidates the old epoch immediately. The
/// rebuild may span ticks; neither search nor result resolution trusts its
/// partial rows.
#[test]
fn house_inventory_epoch_hides_partial_rebuilds_and_stale_results() {
    use openshard_protocol::containers::GridSlot;
    use openshard_protocol::gump::GumpPoint;
    use openshard_state::HouseInventoryError;
    use openshard_state::components::Contained;

    use crate::inventory::{
        resolve,
        search,
    };
    use crate::storage::lock_down;

    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    let house = place(&mut state, actor, at, Facet(0), COTTAGE, owner).unwrap();
    state.registry.insert(actor, Position(at));
    state.registry.insert(actor, Facet(0));
    let chest = an_item(&mut state, at, true);
    lock_down(&mut state, actor, house, chest, Some(Standing::CoOwner)).unwrap();
    let chest_serial = state.registry.serial_of(chest).unwrap();
    let identity = openshard_state::house_item_identity(&state, chest).unwrap();
    state.advance_house_inventory_rebuilds(256);
    let first = search(&state, actor, None, &[identity], None, 10).unwrap();

    let child = an_item(&mut state, at, false);
    openshard_state::relocate_item(
        &mut state,
        child,
        openshard_state::ItemLocation::contained(Contained {
            container: chest_serial,
            position:  GumpPoint::new(0, 0),
            grid:      GridSlot(0),
        }),
    )
    .unwrap();
    assert!(matches!(
        search(&state, actor, Some(first.epoch), &[identity], None, 10),
        Err(crate::inventory::SearchRefusal::Index(
            HouseInventoryError::StaleEpoch { .. }
        ))
    ));
    assert_eq!(
        resolve(
            &state,
            actor,
            first.epoch,
            identity,
            chest_serial,
            first.rows[0].first_pile,
        ),
        None
    );

    state.advance_house_inventory_rebuilds(1);
    assert!(matches!(
        search(&state, actor, None, &[identity], None, 10),
        Err(crate::inventory::SearchRefusal::Index(
            HouseInventoryError::Unavailable { .. }
        ))
    ));
    for _ in 0..8 {
        state.advance_house_inventory_rebuilds(1);
    }
    let rebuilt = search(&state, actor, None, &[identity], None, 10).unwrap();
    assert_eq!(rebuilt.rows[0].root_total, 2);

    let house_serial = state.registry.serial_of(house).unwrap();
    design::redesign(&mut state, actor, house, a_lean_to()).unwrap();
    assert!(matches!(
        state.house_inventory_page(
            house_serial,
            Standing::Owner,
            Some(rebuilt.epoch),
            &[identity],
            None,
            10,
        ),
        Err(HouseInventoryError::StaleEpoch { .. })
    ));
    state.advance_house_inventory_rebuilds(256);
    assert!(
        state
            .house_inventory_page(house_serial, Standing::Owner, None, &[identity], None, 10,)
            .unwrap()
            .rows
            .is_empty(),
        "a root outside the replacement footprint survived the epoch rebuild"
    );
}

/// Pagination is over ordered roots, not over every pile, and a root belonging
/// to an adjacent house never enters the current house's aggregate.
#[test]
fn house_inventory_pages_roots_without_crossing_into_an_adjacent_house() {
    use crate::inventory::search;
    use crate::storage::lock_down;

    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let first_at = Point::new(8, 8, 0);
    let first_house = place(&mut state, actor, first_at, Facet(0), COTTAGE, owner).unwrap();
    state.registry.insert(actor, Position(first_at));
    state.registry.insert(actor, Facet(0));
    let first_root = an_item(&mut state, first_at, false);
    let second_root = an_item(&mut state, first_at, false);
    lock_down(&mut state, actor, first_house, first_root, None).unwrap();
    lock_down(&mut state, actor, first_house, second_root, None).unwrap();

    let (other_actor, other_owner) = an_actor(&mut state);
    let other_at = Point::new(22, 22, 0);
    let other_house = place(&mut state, other_actor, other_at, Facet(0), COTTAGE, other_owner).unwrap();
    let foreign = an_item(&mut state, other_at, false);
    lock_down(&mut state, other_actor, other_house, foreign, None).unwrap();

    state.advance_house_inventory_rebuilds(1024);
    let identity = openshard_state::house_item_identity(&state, first_root).unwrap();
    let page = search(&state, actor, None, &[identity], None, 1).unwrap();
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.rows[0].aggregate_total, 2);
    let next = page.next.expect("a second root remains");
    let tail = search(&state, actor, Some(page.epoch), &[identity], Some(next), 1).unwrap();
    assert_eq!(tail.rows.len(), 1);
    assert_ne!(page.rows[0].root, tail.rows[0].root);
    assert!(tail.next.is_none());
}

/// The six stages are the reference's thresholds, and the boundaries are where
/// it puts them.
///
/// The boundaries rather than a sample from the middle of each band: they are
/// not evenly spaced — the first stage is half a percent of the period and the
/// last is five — so a rounding slip shows up nowhere else.
#[test]
fn a_house_wears_through_the_reference_stages() {
    use crate::decay::Condition;

    for (per_mille, expected) in [
        (0, Condition::LikeNew),
        (4, Condition::LikeNew),
        (5, Condition::Slightly),
        (249, Condition::Slightly),
        (250, Condition::Somewhat),
        (499, Condition::Somewhat),
        (500, Condition::Fairly),
        (749, Condition::Fairly),
        (750, Condition::Greatly),
        (949, Condition::Greatly),
        (950, Condition::InDanger),
        (999, Condition::InDanger),
        (1000, Condition::Collapsed),
        (5000, Condition::Collapsed),
    ] {
        assert_eq!(Condition::at(per_mille), expected, "at {per_mille} per mille");
    }
}

/// The clock runs, the refresh stops it, and a period of zero turns it off.
#[test]
fn the_sweep_ages_a_house_and_a_refresh_undoes_it() {
    use crate::decay::{
        Condition,
        age_and_collect,
        condition,
        refresh,
    };

    let mut state = world_with(cottage());
    state.gameplay.house_decay_ticks = 1000;
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    let house = place(&mut state, actor, at, Facet(0), COTTAGE, owner).expect("a legal spot");

    assert_eq!(condition(&state, house), Condition::LikeNew);
    for _ in 0..600 {
        assert!(age_and_collect(&mut state).is_empty());
    }
    assert_eq!(condition(&state, house), Condition::Fairly);
    refresh(&mut state, house);
    assert_eq!(condition(&state, house), Condition::LikeNew);

    // Decay off: nothing ages, so nothing ever collapses.
    state.gameplay.house_decay_ticks = 0;
    for _ in 0..5000 {
        assert!(age_and_collect(&mut state).is_empty());
    }
    assert_eq!(
        state.registry.get::<House>(house).map(|entry| entry.age),
        Some(0),
        "a shard with decay off still counted"
    );
}

/// A footprint refusal is the first answer: no packing or fixture teardown has
/// started, and the old obstruction still has a live house behind it.
#[test]
fn demolition_refuses_an_unreadable_shape_before_touching_the_house() {
    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    let house = place(&mut state, actor, at, Facet(0), COTTAGE, owner).expect("a legal spot");
    let house_serial = state.registry.serial_of(house).expect("the house serial");
    let door = state
        .registry
        .query::<openshard_state::components::HouseDoor>()
        .find(|(_, door)| door.house == house_serial)
        .map(|(entity, _)| entity)
        .expect("the cottage door");
    let sign = state
        .registry
        .query::<openshard_state::components::HouseSign>()
        .find(|(_, sign)| sign.house == house_serial)
        .map(|(entity, _)| entity)
        .expect("the cottage sign");
    let pinned = an_item(&mut state, at, false);
    let owner_entity = state.registry.entity_of(owner).expect("the owner");
    storage::lock_down(&mut state, owner_entity, house, pinned, None).expect("the owner may pin it");

    // The persisted design is damaged: its one wall lands west of the facet.
    // The obstruction index still holds the classic walls established before
    // that bad row was restored, which is exactly what must not be orphaned.
    state.registry.insert(
        house,
        HouseDesign {
            components: vec![component(WALL, -20, 0, 0, true)],
            revision:   7,
        },
    );

    assert_eq!(
        decay::demolish(&mut state, house),
        Err(decay::DemolishError::CurrentShapeUnreadable(Refusal::OffTheMap))
    );
    assert_eq!(
        state.registry.serial_of(house),
        Some(house_serial),
        "the house was deleted"
    );
    assert!(state.registry.contains(door), "the fixture door was deleted");
    assert!(state.registry.contains(sign), "the sign was deleted");
    assert!(
        state
            .registry
            .has::<openshard_state::components::LockedDown>(pinned),
        "the item was unpinned"
    );
    assert_eq!(
        state.registry.get::<Position>(pinned).map(|position| position.0),
        Some(at),
        "the item was packed before the refusal"
    );
    assert!(
        state.facet_state(Facet(0)).obstructions().holds_anything(9, 9),
        "the indexed wall was removed"
    );
    assert!(
        state
            .registry
            .query::<Drawn>()
            .all(|(_, drawn)| drawn.id != Graphic(decay::CRATE_GRAPHIC)),
        "a moving crate was created before the refusal"
    );
}

/// The whole of H5 in one house: it comes down, the walls go with it, and what
/// it was holding is in the crate rather than gone.
#[test]
fn a_collapsed_house_leaves_a_crate_and_no_walls() {
    use openshard_state::components::{
        Contained,
        Container,
    };

    use crate::decay::{
        CRATE_GRAPHIC,
        age_and_collect,
        demolish,
    };
    use crate::storage::lock_down;

    let mut state = world_with(cottage());
    state.gameplay.house_decay_ticks = 10;
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    let house = place(&mut state, actor, at, Facet(0), COTTAGE, owner).expect("a legal spot");
    let master = state.registry.entity_of(owner).expect("a mobile");
    let house_serial = state.registry.serial_of(house).expect("its serial");

    // A locked-down plank, a secure chest, and something inside the chest.
    let plank = an_item(&mut state, at, false);
    let chest = an_item(&mut state, at, true);
    lock_down(&mut state, master, house, plank, None).unwrap();
    lock_down(&mut state, master, house, chest, Some(Standing::Friend)).unwrap();
    let chest_serial = state.registry.serial_of(chest).unwrap();
    let inside = an_item(&mut state, at, false);
    openshard_state::relocate_item(
        &mut state,
        inside,
        openshard_state::ItemLocation::contained(Contained {
            container: chest_serial,
            position:  openshard_protocol::gump::GumpPoint::new(0, 0),
            grid:      openshard_protocol::containers::GridSlot(0),
        }),
    )
    .unwrap();
    // And a loose barrel nobody pinned, which is not the house's to move.
    let loose = an_item(&mut state, at, false);

    // The walls are up before, and down after.
    assert!(
        state
            .facet_state(Facet(0))
            .obstructions()
            .holds_anything(at.x - 1, at.y - 1),
        "the cottage never had walls"
    );
    let mut down = Vec::new();
    for _ in 0..11 {
        down = age_and_collect(&mut state);
        if !down.is_empty() {
            break;
        }
    }
    assert_eq!(down, vec![house], "the period ran out and nothing collapsed");
    demolish(&mut state, house).expect("a readable collapsed house");

    assert!(
        !state
            .facet_state(Facet(0))
            .obstructions()
            .holds_anything(at.x - 1, at.y - 1),
        "the walls outlived the house"
    );
    assert!(
        state.registry.serial_of(house).is_none(),
        "the house is still there"
    );
    assert!(
        state
            .registry
            .query::<openshard_state::components::HouseSign>()
            .next()
            .is_none(),
        "the sign outlived its house"
    );

    // One crate, on the house's own tile, holding the plank and the chest — and
    // the chest still holding what was in it.
    let crates: Vec<_> = state
        .registry
        .query::<Container>()
        .filter(|(entity, _)| {
            state
                .registry
                .get::<Drawn>(*entity)
                .is_some_and(|drawn| drawn.id == Graphic(CRATE_GRAPHIC))
        })
        .map(|(entity, _)| entity)
        .collect();
    assert_eq!(crates.len(), 1, "the wrong number of crates");
    let crate_serial = state.registry.serial_of(crates[0]).unwrap();
    assert_eq!(state.registry.get::<Position>(crates[0]).map(|p| p.0), Some(at));

    let packed: Vec<EntityId> = state
        .registry
        .query::<Contained>()
        .filter(|(_, held)| held.container == crate_serial)
        .map(|(entity, _)| entity)
        .collect();
    assert_eq!(packed.len(), 2, "the crate holds {packed:?}");
    assert!(packed.contains(&plank) && packed.contains(&chest));
    assert_eq!(
        state.registry.get::<Contained>(inside).map(|held| held.container),
        Some(chest_serial),
        "the chest was emptied into the crate beside it"
    );
    assert!(
        state.registry.get::<Position>(loose).is_some(),
        "the loose barrel was swept up with the house's own things"
    );
    assert!(
        !state
            .registry
            .has::<openshard_state::components::LockedDown>(plank),
        "the plank came out of the house still pinned to it"
    );
    let _ = house_serial;
}

/// An installed addon leaves a collapsing house as the deed that made it.
///
/// The components would otherwise land in the crate as two loose oven graphics:
/// nothing lost, but the [`AddonPart`] group that made them one oven dies with
/// the house, so what the player got back could never be taken up again.
#[test]
fn a_collapsed_house_packs_an_installed_oven_as_one_deed() {
    use openshard_state::components::{
        AddonKind,
        AddonPart,
        Contained,
        Container,
        ItemKind,
    };

    use crate::decay::{
        CRATE_GRAPHIC,
        demolish,
    };
    use crate::storage::lock_down;

    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    let house = place(&mut state, actor, at, Facet(0), COTTAGE, owner).expect("a legal spot");
    let master = state.registry.entity_of(owner).expect("a mobile");

    // Two locked-down tiles grouped the way `place_addon_from_deed` groups them:
    // the root carries the group as well, naming itself.
    let root = an_item(&mut state, at, false);
    let second = an_item(&mut state, at, false);
    lock_down(&mut state, master, house, root, None).unwrap();
    lock_down(&mut state, master, house, second, None).unwrap();
    let root_serial = state
        .registry
        .serial_of(root)
        .expect("a locked-down item has a serial");
    for component in [root, second] {
        state.registry.insert(
            component,
            AddonPart {
                addon: AddonKind::StoneOvenEast,
                root:  root_serial,
            },
        );
    }

    demolish(&mut state, house).expect("a readable house");

    assert!(
        state.registry.serial_of(root).is_none() && state.registry.serial_of(second).is_none(),
        "the oven tiles outlived the addon they were part of"
    );

    let crates: Vec<EntityId> = state
        .registry
        .query::<Container>()
        .filter(|(entity, _)| {
            state
                .registry
                .get::<Drawn>(*entity)
                .is_some_and(|drawn| drawn.id == Graphic(CRATE_GRAPHIC))
        })
        .map(|(entity, _)| entity)
        .collect();
    assert_eq!(crates.len(), 1, "the wrong number of crates");
    let crate_serial = state
        .registry
        .serial_of(crates[0])
        .expect("a fresh crate has a serial");
    let packed: Vec<EntityId> = state
        .registry
        .query::<Contained>()
        .filter(|(_, held)| held.container == crate_serial)
        .map(|(entity, _)| entity)
        .collect();
    assert_eq!(packed.len(), 1, "the crate holds {packed:?}");
    assert_eq!(
        state.registry.get::<ItemKind>(packed[0]).map(|kind| kind.0),
        Some(AddonKind::StoneOvenEast.deed_kind()),
        "the refund is not the oven's own deed"
    );
}

/// A house with nothing pinned in it leaves no crate.
#[test]
fn an_empty_house_leaves_no_crate() {
    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    let house = place(&mut state, actor, at, Facet(0), COTTAGE, owner).expect("a legal spot");
    assert_eq!(house_at(&state, at, Facet(0)), Some(house));

    assert_eq!(crate::decay::demolish(&mut state, house), Ok(None));
    assert_eq!(
        house_at(&state, at, Facet(0)),
        None,
        "demolition left a discoverable indexed house"
    );
    assert!(
        state
            .registry
            .query::<openshard_state::components::Container>()
            .next()
            .is_none(),
        "an empty house left a crate to stand in the road"
    );
}

/// Lay a `no_housing` region over a box, so a placement has something to be
/// refused by.
///
/// A fixture rather than the shipped dataset, and that is the trade this file
/// already makes everywhere: what is under test is the *arithmetic* — which
/// tiles are asked about, and at what z — and `regions.json`'s Covetous would
/// test the same arithmetic against a rectangle nobody can hold in their head.
/// The shipped data is exercised by the world crate's own boot, where a region
/// set is registered for real.
fn forbid_housing(state: &mut WorldState, rect: openshard_state::RegionRect) {
    state
        .facet_state_mut(Facet(0))
        .regions
        .set(vec![openshard_state::Region {
            id:       openshard_state::RegionId(0),
            name:     "a dungeon".into(),
            priority: 0,
            rects:    vec![rect],
            flags:    openshard_state::RegionFlags {
                no_housing:  true,
                guarded:     false,
                no_teleport: false,
                no_recall:   false,
                safe:        false,
            },
            music:    None,
            light:    None,
        }]);
}

/// A house does not go up in Covetous.
///
/// **Against the shipped dataset, by name**, so the data and the reader are
/// tested together. Every other test in this file is right to use a fixture —
/// what they check is arithmetic — but this one checks that a flag twenty-one
/// real rows carry actually reaches the rule, and a fixture cannot say that: the
/// flag was plumbed from JSON through codegen to the save and back for five
/// phases while nothing read it, and only real data catches the sixth way that
/// could go wrong.
#[test]
fn a_shipped_no_housing_region_refuses_a_house() {
    let mut state = britannia_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let felucca = openshard_state::region::shipped()
        .into_iter()
        .find(|set| set.facet == Facet(0))
        .expect("the shard ships one region set");
    let covetous = felucca
        .regions
        .iter()
        .find(|region| region.name == "Covetous")
        .expect("Covetous is in the shipped data")
        .clone();
    assert!(covetous.flags.no_housing, "Covetous stopped refusing houses");
    let inside = covetous.rects[0];
    state.facet_state_mut(Facet(0)).regions.set(felucca.regions);

    let at = Point::new(inside.x + 5, inside.y + 5, 0);
    assert_eq!(
        place(&mut state, actor, at, Facet(0), COTTAGE, owner),
        Err(Refusal::NoHousingHere)
    );
    // And nothing was left behind by the refusal — the serial is spent after the
    // checks, so a refused placement costs a click and no more.
    assert!(state.registry.query::<House>().next().is_none());
}

/// The same rule against a fixture, which is what the other cases build on.
#[test]
fn a_house_is_refused_inside_a_no_housing_region() {
    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    forbid_housing(&mut state, openshard_state::RegionRect::new(5, 5, 12, 12));

    assert_eq!(
        place(&mut state, actor, at, Facet(0), COTTAGE, owner),
        Err(Refusal::NoHousingHere)
    );
}

/// **The specification.** A house whose origin is outside the region and whose
/// wall reaches in is refused.
///
/// The test that fails if the check reads `at` instead of the covered tiles, and
/// the reason D9 walks the whole footprint: a region boundary is a rectangle
/// edge, and a player standing one tile outside Deceit with their east wall
/// inside it has built a house in Deceit.
#[test]
fn a_house_whose_wall_reaches_a_no_housing_region_is_refused() {
    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    // The cottage's box runs from -1 to +1, so it covers x 9..=11. A region
    // starting at x 11 contains the east wall and not the origin.
    forbid_housing(&mut state, openshard_state::RegionRect::new(11, 5, 12, 12));

    assert!(
        state
            .region_at(Facet(0), at)
            .is_none_or(|region| !region.flags.no_housing),
        "the origin is inside the region, so this test proves nothing"
    );
    assert_eq!(
        place(&mut state, actor, at, Facet(0), COTTAGE, owner),
        Err(Refusal::NoHousingHere),
        "the east wall stands inside a region that refuses houses"
    );
}

/// A house is judged at its own height, not at each component's.
///
/// A banded region — the shape 247 of the shipped rects have, and what keeps the
/// sky above a dungeon open — refuses a house placed at its floor. Testing each
/// tile at its component's z would read a roof as being above the band and
/// answer "not in the dungeon" for the top half of a house that is
/// unambiguously in it. D9a.
#[test]
fn a_house_is_judged_at_its_own_height_not_its_roof() {
    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    // A band that contains the foundation and stops well below any roof.
    forbid_housing(
        &mut state,
        openshard_state::RegionRect::new(5, 5, 12, 12).with_z(-5, 5),
    );

    assert_eq!(
        place(&mut state, actor, at, Facet(0), COTTAGE, owner),
        Err(Refusal::NoHousingHere)
    );
}

/// The region refusal is given before the ground refusal.
///
/// The order of the checks is the *message*. `BadGround` means "try a tile
/// over", and inside a dungeon that is a lie — so a spot that is both refuses
/// for the reason a player can act on. D9b, which is invisible otherwise.
#[test]
fn the_region_refusal_comes_before_the_ground_refusal() {
    // `fits: false` makes every tile bad ground, so both rules would fire.
    let mut state = ground_of(cottage(), 0, false);
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    forbid_housing(&mut state, openshard_state::RegionRect::new(5, 5, 12, 12));

    assert_eq!(
        place(&mut state, actor, at, Facet(0), COTTAGE, owner),
        Err(Refusal::NoHousingHere),
        "the ground answered first, and told the player to try a tile over"
    );
}

/// A region with the flag off does not refuse anything.
///
/// The other half of the base case: it would be easy to write a check that
/// refuses inside *any* region, and 51 of the shipped 128 are guarded towns
/// where a house is perfectly legal.
#[test]
fn an_ordinary_region_takes_a_house() {
    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    state
        .facet_state_mut(Facet(0))
        .regions
        .set(vec![openshard_state::Region {
            id:       openshard_state::RegionId(0),
            name:     "a town".into(),
            priority: 0,
            rects:    vec![openshard_state::RegionRect::new(5, 5, 12, 12)],
            flags:    openshard_state::RegionFlags {
                guarded:     true,
                no_teleport: false,
                no_recall:   false,
                no_housing:  false,
                safe:        false,
            },
            music:    None,
            light:    None,
        }]);

    assert!(place(&mut state, actor, at, Facet(0), COTTAGE, owner).is_ok());
}

/// Staff build where a dungeon forbids it.
///
/// D3 has claimed this since H1 and it was never true, because `place` had no
/// actor to ask about. This is the only proof the exemption exists at all.
#[test]
fn staff_may_build_where_a_dungeon_forbids_it() {
    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    forbid_housing(&mut state, openshard_state::RegionRect::new(5, 5, 12, 12));

    assert_eq!(
        place(&mut state, actor, at, Facet(0), COTTAGE, owner),
        Err(Refusal::NoHousingHere),
        "the rule does not apply to a player"
    );
    state.registry.insert(actor, openshard_state::components::Staff);
    assert!(
        place(&mut state, actor, at, Facet(0), COTTAGE, owner).is_ok(),
        "a game master could not lay out a town"
    );
}

/// **The other half of D10, and the one a careless bypass breaks silently.**
///
/// The reference's exemption is a single early return. Copying that shape here
/// would be wrong, because this engine's `Refusal` mixes judgements about the
/// plot with facts about the id — and skipping the second kind reopens holes
/// other decisions closed: an invisible house out of a treasure-site marker, or
/// a foundation placed with no stairs and nobody able to get in.
#[test]
fn staff_are_still_refused_what_is_not_a_house() {
    let mut state = world_with(vec![component(1, 0, 0, 0, false)]);
    let (actor, owner) = an_actor(&mut state);
    state.registry.insert(actor, openshard_state::components::Staff);
    let at = Point::new(10, 10, 0);

    assert_eq!(
        place(&mut state, actor, at, Facet(0), COTTAGE, owner),
        Err(Refusal::DrawsNothing),
        "staff spawned an invisible house from a marker"
    );
    assert_eq!(
        place(&mut state, actor, at, Facet(0), COTTAGE + 1, owner),
        Err(Refusal::NoSuchMulti),
        "staff placed a multi no client knows"
    );
    assert_eq!(
        place(&mut state, actor, at, Facet(0), FOUNDATION_IDS.end - 1, owner),
        Err(Refusal::NeedsCustomisation),
        "staff placed a foundation whose platform this shard cannot read, so it has no stairs \
         — the failure that refusal exists to prevent"
    );
}

/// Staff are exempt from the plot's judgements, not from arithmetic.
///
/// `OffTheMap` comes out of `footprint_of`, above the exemption, so a house whose
/// components would land at a negative coordinate is refused whoever asks. It is
/// in D10's second row for a reason: there is no tile there to place it on.
#[test]
fn staff_are_still_refused_a_house_off_the_edge_of_the_world() {
    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    state.registry.insert(actor, openshard_state::components::Staff);

    assert_eq!(
        place(&mut state, actor, Point::new(0, 0, 0), Facet(0), COTTAGE, owner),
        Err(Refusal::OffTheMap),
        "the cottage's west wall would stand at x -1"
    );
}

// -- C1: a house that is not the shape its multi id says ---------------------

/// A second shape to redesign into: one wall, far enough from the cottage's ring
/// that "did the old walls come out" and "did the new ones go in" are two
/// different tiles rather than one.
fn a_lean_to() -> Vec<Component> {
    vec![
        component(1, 0, 0, 0, false),
        component(WALL, 3, 3, 0, true),
        component(FLOOR, 3, 4, 0, true),
    ]
}

/// **The one that fails if the old walls are unblocked as the new shape.**
///
/// A redesign is two edits to the obstruction index and they read different
/// shapes: out as what stood there, in as what stands there now. Getting the
/// order right is not enough — asking the *new* design where the old walls were
/// leaves every tile the two do not share blocked by a wall that is gone, and
/// nothing reports it. A player finds it by walking into thin air.
#[test]
fn a_redesigned_house_takes_its_old_walls_out_and_puts_its_new_ones_in() {
    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    let house = place(&mut state, actor, at, Facet(0), COTTAGE, owner).expect("a legal spot");

    assert!(
        state.facet_state(Facet(0)).obstructions().holds_anything(9, 9),
        "the cottage's north-west wall"
    );

    design::redesign(&mut state, actor, house, a_lean_to()).expect("the owner may redesign");

    assert_eq!(
        house_at(&state, Point::new(9, 9, 0), Facet(0)),
        None,
        "the old design remained in the coverage index"
    );
    assert_eq!(
        house_at(&state, Point::new(13, 14, 0), Facet(0)),
        Some(house),
        "a non-blocking floor was omitted from house coverage"
    );

    let obstructions = &state.facet_state(Facet(0)).obstructions();
    assert!(
        !obstructions.holds_anything(9, 9),
        "a wall that is no longer part of the house is still blocking"
    );
    assert!(obstructions.holds_anything(13, 13), "the lean-to's wall");
    assert!(
        obstructions.blocker_at_z(13, 14, 0).is_none(),
        "the floor is walked over, not blocked — the same rule as before the redesign"
    );
    assert!(
        obstructions.holds_anything(13, 14),
        "and it is still *there*: the new design's floor is somewhere to stand"
    );
}

/// The sparse row is a derived shortcut, never permission authority. A damaged
/// caller that changes the canonical design without using `redesign` may leave
/// an old candidate behind, but that candidate must not answer as a house.
#[test]
fn stale_house_coverage_is_revalidated_against_the_current_design() {
    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    let house = place(&mut state, actor, at, Facet(0), COTTAGE, owner).expect("a legal spot");
    assert_eq!(house_at(&state, at, Facet(0)), Some(house));

    state.registry.insert(
        house,
        HouseDesign {
            components: a_lean_to(),
            revision:   2,
        },
    );

    assert_eq!(
        house_at(&state, at, Facet(0)),
        None,
        "a stale derived row overruled the canonical design"
    );
}

/// A broken saved design is shard damage, not an empty old footprint. Treating
/// it as the latter commits the replacement while leaving the walls that were
/// indexed before the damage as invisible collision.
#[test]
fn a_redesign_refuses_an_unreadable_current_shape_without_committing() {
    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    let house = place(&mut state, actor, at, Facet(0), COTTAGE, owner).expect("a legal spot");
    let broken = vec![component(WALL, -20, 0, 0, true)];
    state.registry.insert(
        house,
        HouseDesign {
            components: broken.clone(),
            revision:   7,
        },
    );

    assert_eq!(
        design::redesign(&mut state, actor, house, a_lean_to()),
        Err(design::DesignRefusal::CurrentShapeUnreadable),
    );
    assert_eq!(
        design::revision(&state, house),
        7,
        "the failed commit bumped the revision"
    );
    assert_eq!(
        design::shape_of_house(&state, house),
        Some(broken),
        "the failed commit replaced the damaged shape"
    );
    let obstructions = state.facet_state(Facet(0)).obstructions();
    assert!(obstructions.holds_anything(9, 9), "the old indexed wall vanished");
    assert!(
        !obstructions.holds_anything(13, 13),
        "the rejected replacement was added to the obstruction index"
    );
}

/// The sign hangs off the box's west-south corner, so a design that moves the
/// box moves the sign. One sign, not two: the old one comes down.
#[test]
fn a_redesigned_house_hangs_its_sign_on_the_new_box() {
    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    let house = place(&mut state, actor, at, Facet(0), COTTAGE, owner).expect("a legal spot");

    let before = sign_position(&state);
    design::redesign(&mut state, actor, house, a_lean_to()).expect("the owner may redesign");
    let after = sign_position(&state);

    assert_ne!(before, after, "the sign is still on the old box's corner");
    assert_eq!(
        after,
        // The box's west-south corner, and `bounds` takes entry zero as well as
        // every drawn component — so a lean-to that is entirely south-east of
        // its origin still has `min_x` of 0. The same rule the cottage's sign
        // is placed by, which is the point of asserting it here rather than
        // re-deriving one for designed houses.
        Some(Point::new(10, 14, i8::try_from(SIGN_Z).unwrap())),
        "the lean-to's west-south corner"
    );
}

/// Every sign in the world, which is one, or the test says why not.
fn sign_position(state: &WorldState) -> Option<Point> {
    let signs: Vec<EntityId> = state
        .registry
        .query::<openshard_state::components::HouseSign>()
        .map(|(entity, _)| entity)
        .collect();
    assert_eq!(signs.len(), 1, "a house has exactly one sign");
    state.registry.get::<Position>(signs[0]).map(|p| p.0)
}

/// A design with nothing drawn in it is refused **with the house still
/// standing** — the walls are not taken down until the new shape is known to be
/// legal. The alternative is a house you can walk through because a bad command
/// half-succeeded.
#[test]
fn a_design_that_draws_nothing_leaves_the_house_exactly_as_it_was() {
    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    let house = place(&mut state, actor, at, Facet(0), COTTAGE, owner).expect("a legal spot");

    assert_eq!(
        design::redesign(&mut state, actor, house, vec![component(1, 0, 0, 0, false)]),
        Err(design::DesignRefusal::DrawsNothing),
        "a design of one undrawn signature tile is not a house"
    );
    assert!(
        state.facet_state(Facet(0)).obstructions().holds_anything(9, 9),
        "the refusal took the walls down anyway"
    );
    assert_eq!(
        design::revision(&state, house),
        0,
        "and it did not bump the revision"
    );
}

/// A co-owner may lock things down and let people in; neither of those changes
/// what the building *is*. Redesigning does, so it is the owner's alone.
#[test]
fn a_co_owner_may_not_redesign_a_house() {
    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);
    let house = place(&mut state, actor, at, Facet(0), COTTAGE, owner).expect("a legal spot");

    let (other, other_serial) = an_actor(&mut state);
    let entry = state.registry.get_mut::<House>(house).unwrap();
    trust(entry, owner, other_serial, Standing::CoOwner, false).expect("the owner may co-own");

    assert_eq!(
        design::redesign(&mut state, other, house, a_lean_to()),
        Err(design::DesignRefusal::NotYours),
    );
}

/// The revision is what tells a client its cached picture is stale, so it goes
/// up on every commit — including one that happens to produce the same walls.
#[test]
fn every_redesign_bumps_the_revision() {
    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let house =
        place(&mut state, actor, Point::new(10, 10, 0), Facet(0), COTTAGE, owner).expect("a legal spot");

    assert_eq!(
        design::revision(&state, house),
        0,
        "an undesigned house is at zero"
    );
    assert_eq!(design::redesign(&mut state, actor, house, a_lean_to()), Ok(1));
    assert_eq!(design::redesign(&mut state, actor, house, a_lean_to()), Ok(2));
}

/// The allowance is four per tile of the house's own area, and a redesign
/// changes the area. Recounted, or a shrunken house keeps the storage of the one
/// it used to be.
#[test]
fn a_redesign_recounts_the_lockdown_allowance() {
    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let house =
        place(&mut state, actor, Point::new(10, 10, 0), Facet(0), COTTAGE, owner).expect("a legal spot");

    // Four walls and a floor: five drawn tiles.
    assert_eq!(
        storage::allowance(&state, house).lockdowns,
        5 * storage::LOCKDOWNS_PER_TILE
    );

    design::redesign(&mut state, actor, house, a_lean_to()).expect("the owner may redesign");

    // One wall and one floor.
    assert_eq!(
        storage::allowance(&state, house).lockdowns,
        2 * storage::LOCKDOWNS_PER_TILE
    );
}

/// A door standing in a doorway the redesign just opened belongs to the house.
///
/// The reason `adopt_doors` is called again from the commit tail rather than
/// only at placement: the walls moved, so what is "inside" moved with them.
#[test]
fn a_redesign_adopts_a_door_its_new_walls_now_stand_around() {
    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let house =
        place(&mut state, actor, Point::new(10, 10, 0), Facet(0), COTTAGE, owner).expect("a legal spot");

    // Well outside the cottage, and inside the lean-to.
    let (door, _) = state.registry.spawn_with_serial(SerialKind::Item).unwrap();
    state.registry.insert(door, Position(Point::new(13, 13, 0)));
    state.registry.insert(door, Facet(0));
    state.registry.insert(
        door,
        openshard_state::components::Door {
            closed:   Graphic(0x0675),
            open:     Graphic(0x0676),
            offset_x: 0,
            offset_y: 0,
            link:     None,
            is_open:  false,
            close_at: openshard_state::WorldTick::ZERO,
        },
    );
    assert!(
        !state.registry.has::<openshard_state::components::HouseDoor>(door),
        "the cottage does not reach that tile"
    );

    design::redesign(&mut state, actor, house, a_lean_to()).expect("the owner may redesign");

    assert!(
        state.registry.has::<openshard_state::components::HouseDoor>(door),
        "the lean-to stands over the door and did not adopt it"
    );
}

// -- C2: a foundation is placeable -------------------------------------------

/// **A foundation goes down, and it goes down with stairs.**
///
/// The whole of what `Refusal::NeedsCustomisation` was waiting for. Its own
/// doc named the reason exactly — a foundation's component list has no stairs,
/// so one placed bare is a house nobody can get into — and the fix is not
/// deleting the refusal but building the design ServUO's `GetEmptyFoundation`
/// derives.
#[test]
fn a_foundation_is_placed_with_a_design_that_has_stairs() {
    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);

    let house = place(&mut state, actor, at, Facet(0), FOUNDATION, owner)
        .expect("a foundation this shard can read the platform of");

    let shape = design::shape_of_house(&state, house).expect("a foundation is placed designed");
    assert!(
        shape.iter().any(|component| component.graphic == Graphic(0x0751)),
        "the design has no stairs, which is the whole reason the refusal existed"
    );
    // One row further south than the platform reaches. The cottage's own box
    // ends at +1, so the stairs are at +2.
    assert!(
        shape
            .iter()
            .filter(|component| component.graphic == Graphic(0x0751))
            .all(|component| component.dy == 2),
        "the stairs are not on the row the box was grown by"
    );
}

/// And the shape it stands as is the design's, not the platform's — which is
/// what makes the walls, the sign and the lockdown allowance agree with the
/// picture the client is sent.
#[test]
fn a_foundation_blocks_where_its_design_says_and_not_where_its_platform_does() {
    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let at = Point::new(10, 10, 0);

    place(&mut state, actor, at, Facet(0), FOUNDATION, owner).expect("open ground");

    // The cottage fixture's walls are at ±1, and the design keeps them: a
    // design is the platform *plus* a floor and stairs, never a replacement.
    assert!(
        state.facet_state(Facet(0)).obstructions().holds_anything(9, 9),
        "the platform's own components were dropped from the design"
    );
}

/// A shard with no client files cannot read a foundation's platform, so it has
/// nothing to build a design out of — and the refusal still stands, for
/// everybody. The same bargain every other client-file question makes.
#[test]
fn a_shard_with_no_client_files_still_refuses_a_foundation() {
    let mut state = world_with(cottage());
    state.multis = Multis::default();
    state.set_tiles(TileData::empty());
    let (actor, owner) = an_actor(&mut state);

    assert_eq!(
        place(
            &mut state,
            actor,
            Point::new(10, 10, 0),
            Facet(0),
            FOUNDATION,
            owner
        ),
        Err(Refusal::NeedsCustomisation),
    );
}

/// The design a foundation is placed with is revision 1, not 0.
///
/// Zero is what `design::revision` answers for a house that has never been
/// designed, so a foundation sitting at zero would be indistinguishable from a
/// classic house — and a client would never be told its picture had arrived.
#[test]
fn a_placed_foundation_starts_at_revision_one() {
    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let house = place(
        &mut state,
        actor,
        Point::new(10, 10, 0),
        Facet(0),
        FOUNDATION,
        owner,
    )
    .expect("open ground");

    assert_eq!(design::revision(&state, house), 1);
}

/// Imported content is the house's first shape, not a redesign after an empty
/// foundation has already been revealed to clients.
#[test]
fn a_supplied_design_is_installed_at_its_first_revision() {
    let mut state = world_with(cottage());
    let (actor, owner) = an_actor(&mut state);
    let imported = vec![component(FLOOR, 0, 0, 0, true), component(WALL, 2, 0, 0, true)];

    let house = super::place_design(
        &mut state,
        actor,
        Point::new(10, 10, 0),
        Facet(0),
        MultiId(FOUNDATION),
        owner,
        imported.clone(),
    )
    .expect("the complete imported design is placeable");

    assert_eq!(design::revision(&state, house), 1);
    assert_eq!(design::shape_of_house(&state, house), Some(imported));
}

// -- The design session's brackets -------------------------------------------

/// A mobile with a connection new enough to speak the design packets, which is
/// what a session's entry asks for and what the plain [`an_actor`] fixture has
/// no reason to carry.
fn a_connected_actor(state: &mut WorldState, raw: u64) -> (EntityId, Serial, ConnectionId) {
    let (actor, serial) = an_actor(state);
    let connection = ConnectionId::from_raw(raw);
    state.connections.insert(
        connection,
        Connection::new(
            // 4.0.0a is `Feature::CustomMulti`'s own boundary; 7.0 is what a
            // shard actually sees.
            ClientVersion::new(7, 0, 0, 0),
            AccountName::new("builder"),
            AccessLevel::Player,
        ),
    );
    state.registry.insert(actor, Client { connection });
    state.players.insert(connection, actor);
    (actor, serial, connection)
}

/// A designed house with its owner standing beside it, connected.
fn a_foundation(state: &mut WorldState, raw: u64) -> (EntityId, Serial, EntityId) {
    let (actor, owner, _) = a_connected_actor(state, raw);
    let house = place(state, actor, Point::new(10, 10, 0), Facet(0), FOUNDATION, owner)
        .expect("a foundation this shard can read the platform of");
    (actor, owner, house)
}

/// Every `0xBF 0x20` this connection has been sent, by the bracket byte the
/// reference writes at offset 9.
fn brackets_sent(state: &WorldState, connection: ConnectionId) -> Vec<u8> {
    state
        .outbox
        .iter()
        .filter(|outbound| outbound.connection == connection)
        .filter(|outbound| {
            outbound.packet.first() == Some(&0xBF) && outbound.packet.get(3..5) == Some(&[0x00, 0x20])
        })
        .filter_map(|outbound| outbound.packet.get(9).copied())
        .collect()
}

/// The whole of step 1: the owner opens a session, and the shard can be asked
/// about it.
#[test]
fn the_owner_opens_a_session_over_their_own_designed_house() {
    let mut state = world_with(cottage());
    let (actor, owner, house) = a_foundation(&mut state, 1);

    session::begin(&mut state, actor, house).expect("the owner may design their own house");

    assert!(session::is_open(&state, house));
    assert_eq!(session::editor_of(&state, house), Some(owner));
    assert_eq!(session::house_of_editor(&state, owner), Some(house));
}

/// The working copy starts as the committed design, because an editor opens on
/// the house as it stands — and it is a *copy*, which is the whole of C7.
#[test]
fn a_session_opens_with_the_committed_design_as_its_working_copy() {
    let mut state = world_with(cottage());
    let (actor, _, house) = a_foundation(&mut state, 1);
    let committed = design::shape_of_house(&state, house).expect("a foundation is placed designed");

    session::begin(&mut state, actor, house).expect("the owner may design their own house");

    let working = state
        .registry
        .get::<openshard_state::components::DesignSession>(house)
        .expect("the session")
        .working
        .clone();
    assert_eq!(working, committed);
    assert_eq!(
        design::shape_of_house(&state, house),
        Some(committed),
        "opening a session changed the committed design"
    );
}

/// C7's load-bearing rule, asked of the world rather than of the component: a
/// session is not a change to anything a stranger outside can see.
#[test]
fn an_open_session_leaves_the_walls_and_the_revision_where_they_were() {
    let mut state = world_with(cottage());
    let (actor, _, house) = a_foundation(&mut state, 1);
    let revision = design::revision(&state, house);
    let blocked = state.facet_state(Facet(0)).obstructions().holds_anything(9, 9);
    assert!(blocked, "the foundation never had walls to keep");

    session::begin(&mut state, actor, house).expect("the owner may design their own house");

    assert!(
        state.facet_state(Facet(0)).obstructions().holds_anything(9, 9),
        "opening a session took the committed walls out of the index"
    );
    assert_eq!(
        design::revision(&state, house),
        revision,
        "opening a session bumped the cache key clients hold"
    );
}

/// The bracket goes to the editor's own screen, and to nobody else's.
#[test]
fn the_brackets_reach_the_editor_and_close_again() {
    let mut state = world_with(cottage());
    let (actor, _, house) = a_foundation(&mut state, 1);
    let (_, _, watcher) = a_connected_actor(&mut state, 2);
    state.outbox.clear();

    session::begin(&mut state, actor, house).expect("the owner may design their own house");
    assert_eq!(brackets_sent(&state, ConnectionId::from_raw(1)), vec![0x04]);
    assert!(
        brackets_sent(&state, watcher).is_empty(),
        "a bystander was told about somebody else's editor"
    );

    assert_eq!(session::end(&mut state, actor), Some(house));
    assert_eq!(brackets_sent(&state, ConnectionId::from_raw(1)), vec![0x04, 0x05]);
    assert!(!session::is_open(&state, house));
}

/// Redesigning is the owner's, and a co-owner is not the owner — the same split
/// [`design::redesign`] makes, asked at the door instead.
#[test]
fn a_co_owner_and_a_stranger_are_both_refused_a_session() {
    let mut state = world_with(cottage());
    let (_, owner, house) = a_foundation(&mut state, 1);
    let (co_owner, co_owner_serial, _) = a_connected_actor(&mut state, 2);
    let (stranger, _, _) = a_connected_actor(&mut state, 3);
    let entry = state.registry.get_mut::<House>(house).expect("the house");
    trust(entry, owner, co_owner_serial, Standing::CoOwner, false).expect("the owner may add one");

    assert_eq!(
        session::begin(&mut state, co_owner, house),
        Err(session::SessionRefusal::NotYours)
    );
    assert_eq!(
        session::begin(&mut state, stranger, house),
        Err(session::SessionRefusal::NotYours)
    );
    assert!(!session::is_open(&state, house));
}

/// A classic house's shape is a multi id in every client's own files. There is
/// nothing here to edit, and a design invented for it would draw as walls no
/// client has.
#[test]
fn a_classic_house_has_nothing_to_design() {
    let mut state = world_with(cottage());
    let (actor, owner, _) = a_connected_actor(&mut state, 1);
    let house =
        place(&mut state, actor, Point::new(10, 10, 0), Facet(0), COTTAGE, owner).expect("a legal spot");

    assert_eq!(
        session::begin(&mut state, actor, house),
        Err(session::SessionRefusal::NotDesignable)
    );
}

/// One editor at a time: two working copies of one house are two commits racing
/// to be the shape.
#[test]
fn a_house_already_open_refuses_a_second_editor() {
    let mut state = world_with(cottage());
    let (actor, owner, house) = a_foundation(&mut state, 1);
    session::begin(&mut state, actor, house).expect("the owner may design their own house");

    assert_eq!(
        session::begin(&mut state, actor, house),
        Err(session::SessionRefusal::AlreadyOpen)
    );
    assert_eq!(session::editor_of(&state, house), Some(owner));
}

/// A ghost does not build. The entry rule and death's own ender are the two
/// halves of one statement.
#[test]
fn a_ghost_is_refused_a_session() {
    let mut state = world_with(cottage());
    let (actor, _, house) = a_foundation(&mut state, 1);
    state.registry.insert(
        actor,
        openshard_state::components::Ghost {
            body: openshard_state::components::Body {
                id:  Graphic(400),
                hue: openshard_protocol::wire::Hue(0),
            },
        },
    );

    assert_eq!(
        session::begin(&mut state, actor, house),
        Err(session::SessionRefusal::Dead)
    );
}

/// A client with no design packets has no editor to open — and, worse, no way
/// to say it closed one. Refused rather than left to a session only a logout
/// could end.
#[test]
fn a_client_too_old_for_the_design_packets_is_refused_a_session() {
    let mut state = world_with(cottage());
    let (actor, owner, house) = a_foundation(&mut state, 1);
    let connection = ConnectionId::from_raw(1);
    state.connections.insert(
        connection,
        Connection::new(
            // One build below `Feature::CustomMulti`'s 4.0.0a.
            ClientVersion::new(3, 0, 0, 0),
            AccountName::new("builder"),
            AccessLevel::Player,
        ),
    );

    assert_eq!(
        session::begin(&mut state, actor, house),
        Err(session::SessionRefusal::ClientTooOld)
    );
    assert_eq!(session::house_of_editor(&state, owner), None);
}

/// Closing a window the shard has already closed is the ordinary answer to a
/// second click, not an error.
#[test]
fn ending_a_session_nobody_has_open_is_harmless() {
    let mut state = world_with(cottage());
    let (actor, _, _) = a_foundation(&mut state, 1);

    assert_eq!(session::end(&mut state, actor), None);
}

/// The ender with teeth: a demolished house cannot be left carrying a session,
/// because the entity it sits on is about to stop existing.
#[test]
fn demolishing_a_house_ends_the_session_over_it() {
    let mut state = world_with(cottage());
    let (actor, owner, house) = a_foundation(&mut state, 1);
    session::begin(&mut state, actor, house).expect("the owner may design their own house");
    state.outbox.clear();

    decay::demolish(&mut state, house).expect("a readable house comes down");

    assert_eq!(
        session::house_of_editor(&state, owner),
        None,
        "the session outlived the house it was over"
    );
    assert_eq!(
        brackets_sent(&state, ConnectionId::from_raw(1)),
        vec![0x05],
        "the editor was left with a window over nothing"
    );
}

/// And the sign's Customise button is the way in, re-asking the authority the
/// window was drawn under.
#[test]
fn the_signs_customise_button_opens_a_session_for_the_owner_alone() {
    use openshard_protocol::gump::{
        GumpResponse,
        RawButtonId,
        RawGumpId,
        RawGumpKey,
    };

    let mut state = world_with(cottage());
    let (actor, owner, house) = a_foundation(&mut state, 1);
    let press = |state: &mut WorldState, who: EntityId| {
        sign::show(state, who, house);
        let serial = state.registry.serial_of(who).expect("a mobile");
        sign::handle(
            state,
            state
                .registry
                .get::<Client>(who)
                .expect("a connected mobile")
                .connection,
            &GumpResponse {
                serial:       RawGumpKey(serial.raw()),
                gump_id:      RawGumpId(sign::HOUSE_GUMP.0),
                button:       RawButtonId(sign::button::CUSTOMISE.0),
                switches:     Vec::new(),
                text_entries: Vec::new(),
            },
        )
    };

    assert!(press(&mut state, actor), "the house window did not answer");
    assert_eq!(session::editor_of(&state, house), Some(owner));

    // And a stranger pressing the same id — the button they were never drawn —
    // is refused by the verb rather than by the layout.
    session::end(&mut state, actor).expect("the owner closes it again");
    let (stranger, _, _) = a_connected_actor(&mut state, 2);
    assert!(press(&mut state, stranger), "the house window did not answer");
    assert!(!session::is_open(&state, house));
}

// -- The editing verbs -------------------------------------------------------
//
// The fixture foundation's platform is `cottage()`, whose box runs -1..=1 on
// both axes, so the grid these verbs work on is -1..=1 east and -1..=2 south:
// three tiles wide, four deep, the last row being the stair strip
// `initial_foundation` lays. Three storeys, since neither side reaches
// fourteen.

/// The working copy, which is the only thing step 2 is allowed to change.
fn working(state: &WorldState, house: EntityId) -> Vec<Component> {
    state
        .registry
        .get::<openshard_state::components::DesignSession>(house)
        .expect("the session")
        .working
        .clone()
}

/// Which storey the editor is on.
fn storey(state: &WorldState, house: EntityId) -> u8 {
    state
        .registry
        .get::<openshard_state::components::DesignSession>(house)
        .expect("the session")
        .floor
}

/// A foundation with a session already open over it.
fn an_open_foundation(state: &mut WorldState, raw: u64) -> (EntityId, EntityId) {
    let (actor, _, house) = a_foundation(state, raw);
    session::begin(state, actor, house).expect("the owner may design their own house");
    (actor, house)
}

/// The grid is the foundation's own box with the stair row on the end, and it
/// is three storeys tall — the two numbers every verb below is checked against,
/// asserted once here so a change to the fixture shows up as one failure rather
/// than as ten.
#[test]
fn the_grid_is_the_foundations_box_with_the_stair_row() {
    let mut state = world_with(cottage());
    let (_, _, house) = a_foundation(&mut state, 1);

    let grid = editing::buildable_box(&state, house).expect("the fixture knows the platform");
    assert_eq!(
        (grid.min_x, grid.min_y, grid.max_x, grid.max_y),
        (-1, -1, 1, 2),
        "the platform's box, one row deeper"
    );
    assert_eq!(
        editing::max_storeys(grid),
        3,
        "nothing here reaches fourteen tiles"
    );
    assert_eq!(editing::storey_z(1), 7, "ServUO's GetLevelZ for level one");
    assert_eq!(editing::storey_z(3), 47);
}

/// A piece lands at the height of the storey being edited, and nowhere else on
/// the shard changes.
#[test]
fn a_built_piece_lands_on_the_storey_the_editor_is_on() {
    let mut state = world_with(cottage());
    let (actor, house) = an_open_foundation(&mut state, 1);
    let committed = design::shape_of_house(&state, house).expect("a foundation is placed designed");
    let revision = design::revision(&state, house);
    state.outbox.clear();

    editing::apply(
        &mut state,
        actor,
        DesignEdit::Build {
            graphic: Graphic(WALL),
            dx:      0,
            dy:      0,
        },
    )
    .expect("the editor may lay a wall inside their own house");

    assert!(
        working(&state, house).contains(&component(WALL, 0, 0, editing::FIRST_STOREY_Z, true)),
        "the wall did not land on the first storey"
    );
    // C7, asked of everything a stranger could notice.
    assert_eq!(
        design::shape_of_house(&state, house),
        Some(committed),
        "an edit reached the committed design"
    );
    assert_eq!(
        design::revision(&state, house),
        revision,
        "an edit bumped the cache key"
    );
    assert!(state.outbox.is_empty(), "an edit put a packet on the wire");
}

/// And an edit lays nothing in the world — the other half of C7, asked of the
/// obstruction index.
///
/// The tile is the one corner of the grid the initial foundation leaves empty:
/// `initial_foundation`'s stair strip starts one east of the box, so the west
/// end of the stair row has nothing on it. A working copy that leaked into the
/// index would show up there as a tile that is suddenly blocked.
#[test]
fn an_edit_lays_nothing_in_the_world() {
    let mut state = world_with(cottage());
    let (actor, _) = an_open_foundation(&mut state, 1);
    let blocked = |state: &WorldState| state.facet_state(Facet(0)).obstructions().holds_anything(9, 12);
    assert!(!blocked(&state), "the fixture's west stair corner is not empty");

    editing::apply(
        &mut state,
        actor,
        DesignEdit::Build {
            graphic: Graphic(WALL),
            dx:      -1,
            dy:      2,
        },
    )
    .expect("the stair row is inside the grid");

    assert!(!blocked(&state), "an edit reached the obstruction index");
}

/// The storey the editor is on decides the height — the whole of what
/// select-floor is for.
#[test]
fn the_chosen_storey_decides_what_height_a_piece_is_built_at() {
    let mut state = world_with(cottage());
    let (actor, house) = an_open_foundation(&mut state, 1);

    editing::apply(&mut state, actor, DesignEdit::Floor { storey: RawStorey(2) })
        .expect("a three-storey house has a second floor");
    assert_eq!(storey(&state, house), 2);

    editing::apply(
        &mut state,
        actor,
        DesignEdit::Build {
            graphic: Graphic(WALL),
            dx:      0,
            dy:      0,
        },
    )
    .expect("the editor may lay a wall inside their own house");

    assert!(
        working(&state, house).contains(&component(WALL, 0, 0, editing::storey_z(2), true)),
        "the wall was laid at the ground floor's height"
    );
}

/// The reference's one exception, stated outright in `Designer_Build`: the
/// far-south row is the stair strip and everything on it sits at zero.
#[test]
fn a_piece_on_the_stair_row_is_laid_at_the_ground() {
    let mut state = world_with(cottage());
    let (actor, house) = an_open_foundation(&mut state, 1);
    editing::apply(&mut state, actor, DesignEdit::Floor { storey: RawStorey(3) }).expect("the top storey");

    editing::apply(
        &mut state,
        actor,
        DesignEdit::Build {
            graphic: Graphic(STAIR),
            dx:      -1,
            dy:      2,
        },
    )
    .expect("the stair row is inside the grid");

    assert!(
        working(&state, house).contains(&component(STAIR, -1, 2, 0, true)),
        "a piece on the stair row took the storey's height"
    );
}

/// Off the grid is refused, and the grid is the *foundation's* — so erasing a
/// corner does not shrink the house a player is allowed to build in.
#[test]
fn building_off_the_foundations_grid_is_refused() {
    let mut state = world_with(cottage());
    let (actor, house) = an_open_foundation(&mut state, 1);
    let before = working(&state, house);

    for (dx, dy) in [(2, 0), (-2, 0), (0, 3), (0, -2), (i32::from(i16::MAX) + 1, 0)] {
        assert_eq!(
            editing::apply(
                &mut state,
                actor,
                DesignEdit::Build {
                    graphic: Graphic(WALL),
                    dx,
                    dy,
                },
            ),
            Err(editing::EditRefusal::OutsideTheHouse),
            "({dx}, {dy}) is off the grid"
        );
    }
    assert_eq!(
        working(&state, house),
        before,
        "a refused build changed the design"
    );

    // The corner of the stair row comes off, and the grid does not follow it.
    editing::apply(
        &mut state,
        actor,
        DesignEdit::Erase {
            graphic: Graphic(0x0751),
            dx:      1,
            dy:      2,
            dz:      0,
        },
    )
    .expect("the stair strip is the player's to move");
    editing::apply(
        &mut state,
        actor,
        DesignEdit::Build {
            graphic: Graphic(STAIR),
            dx:      1,
            dy:      2,
        },
    )
    .expect("and the tile it came off is still inside the house");
}

/// One wall replaces another on the same tile at the same height; a wall and a
/// floor do not replace each other. ServUO's `MultiComponentList.Add`, and
/// without it a client's repeated clicks stack a hundred copies on one tile.
#[test]
fn a_piece_replaces_only_what_stands_in_the_same_sense() {
    let mut state = world_with(cottage());
    let (actor, house) = an_open_foundation(&mut state, 1);
    let build = |state: &mut WorldState, graphic: u16| {
        editing::apply(
            state,
            actor,
            DesignEdit::Build {
                graphic: Graphic(graphic),
                dx:      0,
                dy:      0,
            },
        )
        .expect("inside the house")
    };
    let at_the_spot = |state: &WorldState| {
        working(state, house)
            .into_iter()
            .filter(|piece| piece.dx == 0 && piece.dy == 0 && piece.dz == editing::FIRST_STOREY_Z)
            .count()
    };

    build(&mut state, WALL);
    build(&mut state, WALL);
    assert_eq!(at_the_spot(&state), 1, "the same wall twice is one wall");

    // A floor has no height and a wall has twenty, so neither is the other's
    // replacement — they are the two things that stand on one tile.
    build(&mut state, FLOOR);
    assert_eq!(at_the_spot(&state), 2, "a floor took a wall's place");
}

/// A roof tile is the roof verb's, which is not built yet. ServUO's
/// `ValidPiece(itemID, roof: false)` refuses the same piece here for the same
/// reason.
#[test]
fn the_build_verb_does_not_lay_a_roof() {
    let mut state = world_with(cottage());
    let (actor, house) = an_open_foundation(&mut state, 1);
    let before = working(&state, house);

    assert_eq!(
        editing::apply(
            &mut state,
            actor,
            DesignEdit::Build {
                graphic: Graphic(ROOF),
                dx:      0,
                dy:      0,
            },
        ),
        Err(editing::EditRefusal::RoofPiece)
    );
    assert_eq!(working(&state, house), before);
}

/// Erase takes off the piece it names and leaves its neighbours alone.
#[test]
fn erasing_takes_off_the_piece_it_names() {
    let mut state = world_with(cottage());
    let (actor, house) = an_open_foundation(&mut state, 1);
    let stair = component(0x0751, 0, 2, 0, true);
    assert!(
        working(&state, house).contains(&stair),
        "the fixture foundation lays a stair strip"
    );

    editing::apply(
        &mut state,
        actor,
        DesignEdit::Erase {
            graphic: Graphic(0x0751),
            dx:      0,
            dy:      2,
            dz:      0,
        },
    )
    .expect("the stair strip is the player's to move");

    let left = working(&state, house);
    assert!(!left.contains(&stair), "the stair is still there");
    assert!(
        left.contains(&component(0x0751, 1, 2, 0, true)),
        "erasing one stair took its neighbour with it"
    );
}

/// The foundation's own floor is not the player's to remove — a hole in it is a
/// house standing on nothing. Every tile at height zero inside the box except
/// the stair row along the south edge.
#[test]
fn the_foundations_own_floor_cannot_be_erased() {
    let mut state = world_with(cottage());
    let (actor, house) = an_open_foundation(&mut state, 1);
    let before = working(&state, house);

    assert_eq!(
        editing::apply(
            &mut state,
            actor,
            DesignEdit::Erase {
                graphic: Graphic(FLOOR),
                dx:      0,
                dy:      0,
                dz:      0,
            },
        ),
        Err(editing::EditRefusal::FixedFloor)
    );
    assert_eq!(
        working(&state, house),
        before,
        "a refused erase changed the design"
    );
}

/// Taking the last piece off an interior tile lays dirt in its place, so a
/// design never has a hole the client draws the ground through. ServUO's
/// `Designer_Delete` does this and it is easy to miss, because the erase looks
/// finished without it.
#[test]
fn erasing_the_last_piece_off_an_interior_tile_leaves_dirt() {
    let mut state = world_with(cottage());
    let (actor, house) = an_open_foundation(&mut state, 1);
    editing::apply(
        &mut state,
        actor,
        DesignEdit::Build {
            graphic: Graphic(WALL),
            dx:      0,
            dy:      0,
        },
    )
    .expect("inside the house");

    editing::apply(
        &mut state,
        actor,
        DesignEdit::Erase {
            graphic: Graphic(WALL),
            dx:      0,
            dy:      0,
            dz:      i32::from(editing::FIRST_STOREY_Z),
        },
    )
    .expect("what was just built comes off again");

    let left = working(&state, house);
    assert!(
        !left.contains(&component(WALL, 0, 0, editing::FIRST_STOREY_Z, true)),
        "the wall is still there"
    );
    assert!(
        left.contains(&component(editing::DIRT, 0, 0, editing::FIRST_STOREY_Z, true)),
        "the tile was left as a hole"
    );
}

/// And nothing is laid where the tile still has a floor, which is the other half
/// of the same rule.
#[test]
fn erasing_off_a_tile_that_still_has_a_floor_lays_nothing() {
    let mut state = world_with(cottage());
    let (actor, house) = an_open_foundation(&mut state, 1);
    for graphic in [WALL, FLOOR] {
        editing::apply(
            &mut state,
            actor,
            DesignEdit::Build {
                graphic: Graphic(graphic),
                dx:      0,
                dy:      0,
            },
        )
        .expect("inside the house");
    }

    editing::apply(
        &mut state,
        actor,
        DesignEdit::Erase {
            graphic: Graphic(WALL),
            dx:      0,
            dy:      0,
            dz:      i32::from(editing::FIRST_STOREY_Z),
        },
    )
    .expect("the wall comes off");

    let left = working(&state, house);
    assert!(
        left.contains(&component(FLOOR, 0, 0, editing::FIRST_STOREY_Z, true)),
        "the floor went with the wall"
    );
    assert!(
        !left.iter().any(|piece| piece.graphic == Graphic(editing::DIRT)),
        "dirt was laid on a tile that still has a floor"
    );
}

/// Erasing what is not there is refused rather than silently ignored: the two
/// are indistinguishable from the client, and only one of them is a test that
/// can fail.
#[test]
fn erasing_nothing_is_refused() {
    let mut state = world_with(cottage());
    let (actor, house) = an_open_foundation(&mut state, 1);
    let before = working(&state, house);

    assert_eq!(
        editing::apply(
            &mut state,
            actor,
            DesignEdit::Erase {
                graphic: Graphic(WALL),
                dx:      0,
                dy:      0,
                dz:      i32::from(editing::FIRST_STOREY_Z),
            },
        ),
        Err(editing::EditRefusal::NothingThere)
    );
    assert_eq!(working(&state, house), before);
}

/// The storey picker clamps rather than refusing — `Designer_Level`'s own
/// answer, because a client showing one button more than this house has storeys
/// is drawing a floor that does not exist rather than asking for a forbidden
/// one.
#[test]
fn the_storey_picker_clamps_to_the_ground_floor() {
    let mut state = world_with(cottage());
    let (actor, house) = an_open_foundation(&mut state, 1);

    for (asked, expected) in [(2, 2), (3, 3), (4, 1), (0, 1), (-1, 1), (i32::MAX, 1)] {
        editing::apply(
            &mut state,
            actor,
            DesignEdit::Floor {
                storey: RawStorey(asked),
            },
        )
        .expect("the storey picker never refuses");
        assert_eq!(storey(&state, house), expected, "asked for storey {asked}");
    }
}

/// An edit from somebody with nothing open changes nothing. The ordinary answer
/// to a packet that crossed a window the shard had already closed.
#[test]
fn an_edit_without_a_session_is_refused() {
    let mut state = world_with(cottage());
    let (actor, _, house) = a_foundation(&mut state, 1);
    let committed = design::shape_of_house(&state, house).expect("a foundation is placed designed");

    assert_eq!(
        editing::apply(
            &mut state,
            actor,
            DesignEdit::Build {
                graphic: Graphic(WALL),
                dx:      0,
                dy:      0,
            },
        ),
        Err(editing::EditRefusal::NoSession)
    );
    assert_eq!(design::shape_of_house(&state, house), Some(committed));
}

/// One editor's verbs reach one editor's house. Two open sessions on one shard
/// is the ordinary case, and `house_of_editor` is what keeps them apart.
#[test]
fn an_edit_reaches_the_house_its_editor_has_open() {
    let mut state = world_with(cottage());
    let (actor, house) = an_open_foundation(&mut state, 1);
    // A second house with its own owner, well out of the first one's yard.
    let (other_actor, other_owner, _) = a_connected_actor(&mut state, 2);
    let other_house = place(
        &mut state,
        other_actor,
        Point::new(24, 24, 0),
        Facet(0),
        FOUNDATION,
        other_owner,
    )
    .expect("open ground");
    session::begin(&mut state, other_actor, other_house).expect("its own owner opens it");
    let untouched = working(&state, other_house);

    editing::apply(
        &mut state,
        actor,
        DesignEdit::Build {
            graphic: Graphic(WALL),
            dx:      0,
            dy:      0,
        },
    )
    .expect("inside the first house");

    assert!(working(&state, house).contains(&component(WALL, 0, 0, editing::FIRST_STOREY_Z, true)));
    assert_eq!(
        working(&state, other_house),
        untouched,
        "one editor's wall landed in another editor's house"
    );
}
