//! A sea with one island in it, and a table that knows one ship.
//!
//! The multi is a fixture rather than a client file, for `openshard-housing`'s
//! own reason: what is under test is the arithmetic — an offset added to an
//! origin, a tiledata flag deciding hull from deck — and a real carrack would
//! test the same arithmetic a hundred times over while making the expected
//! answer impossible to write down.

use std::collections::{BTreeMap, HashMap};

use openshard_entities::Registry;
use openshard_events::EventBus;
use openshard_movement::scene::Scene;
// `Terrain` is in scope for its *methods*: the tests below ask a `LiveTerrain`
// whether a step is allowed. Nothing here implements it any more.
use openshard_movement::{Doors, Walker};
use openshard_protocol::direction::{Direction, Facing};
use openshard_protocol::serial::SerialKind;
use openshard_state::rng::Rng;
use openshard_state::{Dialogue, FacetState, Gameplay, QuestDefs};
use openshard_tiles::TileFlags;
use openshard_uofiles::multi::{Component, Multi, Multis};

use super::*;

/// A small sea.
const SIZE: u32 = 64;
/// The one ship the fixture terrain knows.
const SLOOP: u16 = 0x0C;
/// A hull plank: impassable, ten tall.
const HULL: u16 = 0x3E4E;
/// A deck plank: walked on, three tall, and *not* impassable — the component
/// that must not be folded into the hull, because a ship whose deck blocked
/// would be a solid block of wood.
const DECK: u16 = 0x3E4A;

/// The land id this sea is made of. Water is not a kind of tile the map knows —
/// it is a flag on the tiledata row the tile points at, which is why the id and
/// the flag are two statements below rather than one.
const WATER: u16 = 0x00A8;
/// The shore's land id: tile `0`, which [`Scene`] leaves unflagged, so it is
/// ordinary walkable ground.
const SHORE: u16 = 0;

/// A sea with one strip of shore along y = 0.
///
/// **Real ground, not a fixture that answers for it.** The water is a
/// [`TileFlags::WATER`] row in the tiledata and the shard's own `land_is_water`
/// reads it there, so a change to what water *means* reaches these tests
/// instead of being agreed with by a double that had reimplemented the rule.
/// What a sloop is made of stays a table — [`multis`] — because that is a fact
/// about the install rather than about this water.
fn sea() -> Scene {
    let mut scene = Scene::flat_holding(
        u16::try_from(SIZE - 1).unwrap(),
        u16::try_from(SIZE - 1).unwrap(),
        0,
    );
    scene.land_art(WATER, TileFlags::WATER);
    scene.land_everywhere(WATER);
    for x in 0..scene.width() {
        scene.land(x, 0, SHORE);
    }
    // The hull is impassable and ten tall, the deck is walked on and three.
    scene.art(HULL, TileFlags::WALL | TileFlags::BLOCK, 10);
    scene.art(DECK, TileFlags::PLATFORM, 3);
    scene
}

/// A real multi table holding the sloop under the one id these tests place.
fn multis() -> Multis {
    Multis::of([Multi::new(SLOOP, sloop())])
}

fn component(graphic: u16, dx: i16, dy: i16, dz: i16, drawn: bool) -> Component {
    Component {
        graphic,
        dx,
        dy,
        dz,
        // `1` is the `.mul`'s drawn value and `0` its skip — the sense that
        // reads backwards from its name.
        flags: u64::from(drawn),
    }
}

/// A sloop: two deck tiles with a hull tile either side, and the signature tile
/// no client draws.
fn sloop() -> Vec<Component> {
    vec![
        component(1, 0, 0, 0, false),
        component(HULL, -1, 0, 0, true),
        component(DECK, 0, 0, 0, true),
        component(DECK, 0, 1, 0, true),
        component(HULL, 1, 0, 0, true),
    ]
}

fn a_sea() -> WorldState {
    // The pair the shard holds: the ground, and the table that ground reads. They
    // come from one scene so a hull's height cannot disagree with the tiledata
    // the terrain is looking at.
    let (map, tiles) = sea().into_shard(Facet(0));
    let mut facets = BTreeMap::new();
    facets.insert(Facet(0), FacetState::new(Some(map), None, SIZE, SIZE));
    WorldState {
        registry: Registry::new(),
        bus: EventBus::new(),
        facets,
        default_facet: Facet(0),
        tiles,
        multis: multis(),
        players: HashMap::new(),
        connections: HashMap::new(),
        seen: HashMap::new(),
        start: (0, 0),
        rng: Rng::new(1),
        ticks: 0,
        hour: 0,
        worn: Default::default(),
        outbox: Vec::new(),
        open_containers: HashMap::new(),
        trades: Vec::new(),
        quests: QuestDefs::default(),
        dialogue: Dialogue::default(),
        guilds: openshard_state::Guilds::default(),
        alliances: openshard_state::Alliances::default(),
        parties: openshard_state::Parties::default(),
        gameplay: Gameplay::default(),
        save_requested: false,
    }
}

fn a_captain(state: &mut WorldState) -> (EntityId, Serial) {
    state.registry.spawn_with_serial(SerialKind::Mobile).unwrap()
}

/// A mobile standing at `at`, on the sector grid where the manifest looks for
/// it. `Movement` is what makes it a body rather than a crate: the manifest
/// carries mobiles, and cargo waits for B4's hold.
fn a_walker(state: &mut WorldState, at: Point) -> EntityId {
    let (entity, _) = state.registry.spawn_with_serial(SerialKind::Mobile).unwrap();
    state.registry.insert(entity, Position(at));
    state.registry.insert(entity, Facet(0));
    state.registry.insert(
        entity,
        openshard_state::components::Movement(Walker::new(at, Facing::walking(Direction::North))),
    );
    state.facet_state_mut(Facet(0)).sectors.insert(entity, at);
    entity
}

/// A ship is an item whose graphic is the multi, exactly as a house is — so
/// everything that already walks items draws it with no change.
#[test]
fn a_boat_is_an_item_whose_graphic_is_the_multi() {
    let mut state = a_sea();
    let (actor, owner) = a_captain(&mut state);
    let at = Point::new(20, 20, 0);
    let boat = place(&mut state, actor, at, Facet(0), SLOOP, owner).expect("open water");

    assert_eq!(
        state.registry.get::<Drawn>(boat).map(|drawn| drawn.id),
        Some(Graphic(MULTI_FLAG | SLOOP)),
        "the wire carries a ship as 0x4000 above its id",
    );
    assert_eq!(state.registry.get::<Position>(boat).map(|p| p.0), Some(at));
    assert_eq!(
        state.registry.get::<Boat>(boat),
        Some(&Boat { multi: SLOOP, owner }),
    );
    assert_eq!(boat_at(&state, at, Facet(0)), Some(boat));
}

/// **The split that makes a ship a ship.** The tiledata flag decides, so a deck
/// carries a body and a hull stops one — folding either into the other gives a
/// solid block of wood or a ghost ship.
#[test]
fn the_hull_blocks_and_the_deck_carries() {
    let mut state = a_sea();
    let (actor, owner) = a_captain(&mut state);
    place(&mut state, actor, Point::new(20, 20, 0), Facet(0), SLOOP, owner).expect("open water");

    let boats = &state.facet_state(Facet(0)).boats();
    assert_eq!(boats.deck_at(20, 20, 0), Some(3), "the deck plank's own top");
    assert_eq!(boats.deck_at(20, 21, 0), Some(3), "and the tile behind it");
    assert!(boats.hull_blocks(19, 20, 0), "the port hull");
    assert!(boats.hull_blocks(21, 20, 0), "the starboard hull");
    assert_eq!(boats.deck_at(19, 20, 0), None, "a hull is not a floor");
}

/// The undrawn signature tile is not part of the ship, the same way it is not
/// part of a house's footprint.
#[test]
fn the_signature_tile_is_not_a_plank() {
    let mut state = a_sea();
    let (actor, owner) = a_captain(&mut state);
    place(&mut state, actor, Point::new(20, 20, 0), Facet(0), SLOOP, owner).expect("open water");

    // Four drawn components, so four planks — not five.
    let boats = &state.facet_state(Facet(0)).boats();
    let covered = [(19, 20), (20, 20), (20, 21), (21, 20)];
    let total: usize = covered.iter().map(|&(x, y)| boats.at(x, y).len()).sum();
    assert_eq!(total, 4, "the signature tile was launched as part of the ship");
}

/// Half on the beach is not afloat. Every tile of the berth is checked, not
/// just the origin — the same reason a house's region check walks its whole
/// footprint.
#[test]
fn a_ship_half_on_the_beach_is_refused() {
    let mut state = a_sea();
    let (actor, owner) = a_captain(&mut state);

    // At y = 0 the sloop's port, starboard and midships planks are all on the
    // shore and only the aft one is afloat.
    assert_eq!(
        place(&mut state, actor, Point::new(20, 0, 0), Facet(0), SLOOP, owner),
        Err(Refusal::NotOnWater),
        "the shore runs along y = 0",
    );
    assert!(
        state.facet_state(Facet(0)).boats().is_empty(),
        "a refused launch left a ship in the index",
    );

    // One tile further out and every plank is over water. The contrast is the
    // point: the check is per tile, so it is the *beached* plank that refuses
    // and not the ship's proximity to land.
    assert!(
        place(&mut state, actor, Point::new(20, 1, 0), Facet(0), SLOOP, owner).is_ok(),
        "a ship moored against the shore is still afloat",
    );
}

/// **The one the index exists to make possible.** Two hulls are not in
/// `Obstructions`, so they do not see each other through the mechanism that
/// stops everything else; the berth check is what stops them, and this is the
/// test that fails if it is dropped.
#[test]
fn two_boats_do_not_occupy_one_tile() {
    let mut state = a_sea();
    let (actor, owner) = a_captain(&mut state);
    let at = Point::new(20, 20, 0);
    place(&mut state, actor, at, Facet(0), SLOOP, owner).expect("open water");

    assert_eq!(
        place(&mut state, actor, at, Facet(0), SLOOP, owner),
        Err(Refusal::Occupied),
    );
    assert_eq!(
        place(&mut state, actor, Point::new(21, 20, 0), Facet(0), SLOOP, owner),
        Err(Refusal::Occupied),
        "and an overlap of one tile is still an overlap",
    );
    assert_eq!(state.facet_state(Facet(0)).boats().len(), 1);
}

/// Far enough apart is fine, which is the other half of the same check.
#[test]
fn two_boats_moor_side_by_side_when_they_do_not_touch() {
    let mut state = a_sea();
    let (actor, owner) = a_captain(&mut state);
    place(&mut state, actor, Point::new(20, 20, 0), Facet(0), SLOOP, owner).expect("open water");
    place(&mut state, actor, Point::new(30, 20, 0), Facet(0), SLOOP, owner).expect("open water");

    assert_eq!(state.facet_state(Facet(0)).boats().len(), 2);
}

/// Staff skip the *judgements* about the berth and nothing else — housing's D10
/// split, with the same reasoning. A game master may put a ship in a fountain.
#[test]
fn staff_may_launch_a_ship_onto_dry_land() {
    let mut state = a_sea();
    let (actor, owner) = a_captain(&mut state);
    state.registry.insert(actor, openshard_state::components::Staff);

    assert!(
        place(&mut state, actor, Point::new(20, 0, 0), Facet(0), SLOOP, owner).is_ok(),
        "the exemption did not reach the water check",
    );
}

/// And they are not exempt from arithmetic: there is no tile off the edge of the
/// world to float on, whoever is asking.
#[test]
fn staff_are_still_refused_a_ship_off_the_edge_of_the_world() {
    let mut state = a_sea();
    let (actor, owner) = a_captain(&mut state);
    state.registry.insert(actor, openshard_state::components::Staff);

    assert_eq!(
        place(&mut state, actor, Point::new(0, 20, 0), Facet(0), SLOOP, owner),
        Err(Refusal::OffTheMap),
        "the port hull would stand at x -1",
    );
}

/// A multi no client knows is a fact about the id, so it is refused for staff
/// too — and it does not leave an entity behind.
#[test]
fn a_multi_that_is_not_a_ship_is_refused_and_leaves_nothing() {
    let mut state = a_sea();
    let (actor, owner) = a_captain(&mut state);
    let before = state.registry.query::<Position>().count();

    assert_eq!(
        place(
            &mut state,
            actor,
            Point::new(20, 20, 0),
            Facet(0),
            SLOOP + 1,
            owner
        ),
        Err(Refusal::NoSuchMulti),
    );
    assert_eq!(
        state.registry.query::<Position>().count(),
        before,
        "a refused launch left an entity on the water",
    );
}

/// Sinking one takes it out of all three places it was put.
#[test]
fn sinking_a_ship_clears_the_index_the_grid_and_the_registry() {
    let mut state = a_sea();
    let (actor, owner) = a_captain(&mut state);
    let at = Point::new(20, 20, 0);
    let boat = place(&mut state, actor, at, Facet(0), SLOOP, owner).expect("open water");

    sink(&mut state, boat);

    assert!(state.facet_state(Facet(0)).boats().is_empty());
    assert_eq!(boat_at(&state, at, Facet(0)), None);
    assert!(state.registry.get::<Position>(boat).is_none());
}

/// A shard with no client files knows no ships, so there is nothing to launch.
///
/// **An empty table, not an absent one.** This test used to say it by clearing
/// [`FacetState::terrain`], which is a different configuration wearing the same
/// words — a facet with no *map*. What "no client files" means to a launch is
/// that `multi.mul` said nothing, and `planks_of` is where that is refused.
#[test]
fn a_shard_with_no_client_files_launches_nothing() {
    let mut state = a_sea();
    state.multis = Multis::default();
    let (actor, owner) = a_captain(&mut state);

    assert_eq!(
        place(&mut state, actor, Point::new(20, 20, 0), Facet(0), SLOOP, owner),
        Err(Refusal::NoSuchMulti),
    );
}

/// A facet with no map has no sea either, and `check_berth` is where that is
/// caught — the other half of the pair above, and the one that is about the
/// *ground* rather than about the tables.
#[test]
fn a_facet_with_no_map_moors_nothing() {
    let mut state = a_sea();
    state.facet_state_mut(Facet(0)).map = None;
    let (actor, owner) = a_captain(&mut state);

    assert_eq!(
        place(&mut state, actor, Point::new(20, 20, 0), Facet(0), SLOOP, owner),
        Err(Refusal::NoSuchMulti),
    );
}

/// A ship sails, and its index and its position agree about where it went.
#[test]
fn a_ship_sails_a_tile() {
    let mut state = a_sea();
    let (actor, owner) = a_captain(&mut state);
    let boat = place(&mut state, actor, Point::new(20, 20, 0), Facet(0), SLOOP, owner).expect("open water");

    assert_eq!(
        step(&mut state, boat, Direction::South),
        Ok(Point::new(20, 21, 0)),
    );

    assert_eq!(
        state.registry.get::<Position>(boat).map(|p| p.0),
        Some(Point::new(20, 21, 0)),
    );
    let boats = &state.facet_state(Facet(0)).boats();
    assert_eq!(boats.deck_at(20, 21, 0), Some(3), "the deck came with it");
    assert!(boats.hull_blocks(19, 21, 0), "and so did the port hull");
    assert!(
        boats.at(20, 20).is_empty() && boats.at(19, 20).is_empty(),
        "the ship left planks behind in the tiles it sailed out of",
    );
    assert_eq!(boats.len(), 1, "one ship, moved — not two");
}

/// **Everyone standing on the deck arrives with it**, moved absolutely rather
/// than carried: this engine has no parent transform, and B1 refused to invent
/// one.
#[test]
fn the_deck_carries_whoever_is_standing_on_it() {
    let mut state = a_sea();
    let (actor, owner) = a_captain(&mut state);
    let boat = place(&mut state, actor, Point::new(20, 20, 0), Facet(0), SLOOP, owner).expect("open water");

    // Amidships, feet on the deck at z 3.
    let sailor = a_walker(&mut state, Point::new(20, 20, 3));

    step(&mut state, boat, Direction::South).expect("open water ahead");

    assert_eq!(
        state.registry.get::<Position>(sailor).map(|p| p.0),
        Some(Point::new(20, 21, 3)),
        "the sailor stayed on the tile of the ship they were standing on",
    );
    assert_eq!(
        state.facet_state(Facet(0)).sectors.position_of(sailor),
        Some(Point::new(20, 21, 3)),
        "and the sector grid was told, or every nearby query still finds them astern",
    );
}

/// A body beside the ship is not aboard it, even when it is on a tile the ship
/// covers — a swimmer at the waterline is under the gunwale, not on the deck.
#[test]
fn someone_in_the_water_beside_the_hull_is_left_behind() {
    let mut state = a_sea();
    let (actor, owner) = a_captain(&mut state);
    let boat = place(&mut state, actor, Point::new(20, 20, 0), Facet(0), SLOOP, owner).expect("open water");

    // On a covered tile, but at the waterline rather than on a plank.
    let swimmer = a_walker(&mut state, Point::new(20, 20, 0));

    step(&mut state, boat, Direction::South).expect("open water ahead");

    assert_eq!(
        state.registry.get::<Position>(swimmer).map(|p| p.0),
        Some(Point::new(20, 20, 0)),
        "the ship dragged somebody who was not standing on it",
    );
}

/// A ship steered into a rock stops, and stops **whole**: nothing is written
/// until every tile of the course has answered, so a refused move leaves the
/// crew where they were too.
#[test]
fn a_ship_steered_into_the_shore_stops_and_moves_nobody() {
    let mut state = a_sea();
    let (actor, owner) = a_captain(&mut state);
    // Bow one tile off the beach: north is the shore at y = 0.
    let boat = place(&mut state, actor, Point::new(20, 1, 0), Facet(0), SLOOP, owner).expect("open water");
    let sailor = a_walker(&mut state, Point::new(20, 1, 3));

    assert_eq!(step(&mut state, boat, Direction::North), Err(Refusal::NotOnWater));

    assert_eq!(
        state.registry.get::<Position>(boat).map(|p| p.0),
        Some(Point::new(20, 1, 0)),
    );
    assert_eq!(
        state.registry.get::<Position>(sailor).map(|p| p.0),
        Some(Point::new(20, 1, 3)),
        "the crew walked ashore without the ship",
    );
    assert_eq!(
        state.facet_state(Facet(0)).boats().deck_at(20, 1, 0),
        Some(3),
        "the berth it was refused out of is still the berth it is in",
    );
}

/// **The hull is taken off the screens that had it.** There is no packet that
/// relocates a drawn item, so a ship that moved and was not forgotten stays
/// drawn where it was for everyone who could already see it — the ghost hull.
///
/// Only the forget half is asserted here: putting it back is `refresh_around`,
/// which needs a watcher with a connection and is that function's own test.
#[test]
fn sailing_takes_the_hull_off_the_screens_that_had_it() {
    let mut state = a_sea();
    let (actor, owner) = a_captain(&mut state);
    let boat = place(&mut state, actor, Point::new(20, 20, 0), Facet(0), SLOOP, owner).expect("open water");

    let watcher = a_walker(&mut state, Point::new(20, 0, 0));
    state.seen.entry(watcher).or_default().insert(boat);

    step(&mut state, boat, Direction::South).expect("open water ahead");

    assert!(
        !state.seen[&watcher].contains(&boat),
        "the ship sailed away and stayed drawn where it was",
    );
}

/// **The test this phase owes by name, against a moving hull.** The placement
/// half is `two_boats_do_not_occupy_one_tile` above; this is the other end of
/// the same hole. Neither hull is in `Obstructions`, so nothing but the course
/// check stands between them.
#[test]
fn two_boats_do_not_occupy_one_tile_when_one_is_under_way() {
    let mut state = a_sea();
    let (actor, owner) = a_captain(&mut state);
    let under_way =
        place(&mut state, actor, Point::new(20, 20, 0), Facet(0), SLOOP, owner).expect("open water");
    // Two tiles south — clear at anchor (the sloops' berths do not touch), and
    // one step south puts the moving ship's aft deck into the moored one's
    // midships.
    place(&mut state, actor, Point::new(20, 22, 0), Facet(0), SLOOP, owner).expect("open water");

    assert_eq!(
        step(&mut state, under_way, Direction::South),
        Err(Refusal::Occupied),
        "one hull sailed straight through another",
    );
    assert_eq!(
        state.registry.get::<Position>(under_way).map(|p| p.0),
        Some(Point::new(20, 20, 0)),
    );
}

/// And a ship is not blocked by *itself*. The course check differs from the
/// berth check by exactly this comparison, which is what lets a ship move at
/// all: every step overlaps the tiles it is standing on.
#[test]
fn a_ship_is_not_blocked_by_the_tiles_it_is_leaving() {
    let mut state = a_sea();
    let (actor, owner) = a_captain(&mut state);
    let boat = place(&mut state, actor, Point::new(20, 20, 0), Facet(0), SLOOP, owner).expect("open water");

    // South by one is a two-tile overlap with where it already is: the deck at
    // (20, 21) is both the tile it is moving out of and the tile it is moving
    // into.
    assert!(step(&mut state, boat, Direction::South).is_ok());
    assert!(step(&mut state, boat, Direction::South).is_ok());
    assert_eq!(
        state.registry.get::<Position>(boat).map(|p| p.0),
        Some(Point::new(20, 22, 0)),
    );
}

/// A ship under way steps on its own cadence and not every tick.
#[test]
fn a_ship_under_way_steps_on_its_cadence() {
    let mut state = a_sea();
    let (actor, owner) = a_captain(&mut state);
    let boat = place(&mut state, actor, Point::new(20, 20, 0), Facet(0), SLOOP, owner).expect("open water");

    set_course(&mut state, boat, Direction::South, false);

    // Every tick up to the interval: nothing, because the first step is due on
    // `ticks + every` rather than now.
    for _ in 0..SLOW_TICKS {
        assert!(sail(&mut state).is_empty());
        assert_eq!(
            state.registry.get::<Position>(boat).map(|p| p.0),
            Some(Point::new(20, 20, 0)),
            "the ship moved before its cadence was up",
        );
        state.ticks += 1;
    }

    assert!(sail(&mut state).is_empty());
    assert_eq!(
        state.registry.get::<Position>(boat).map(|p| p.0),
        Some(Point::new(20, 21, 0)),
        "the cadence came up and the ship did not move",
    );

    // And it holds the course rather than needing to be told again.
    state.ticks += SLOW_TICKS;
    sail(&mut state);
    assert_eq!(
        state.registry.get::<Position>(boat).map(|p| p.0),
        Some(Point::new(20, 22, 0)),
    );
}

/// Fast is the reference's other interval and it is four times as often.
#[test]
fn a_fast_ship_steps_four_times_as_often() {
    let mut state = a_sea();
    let (actor, owner) = a_captain(&mut state);
    let boat = place(&mut state, actor, Point::new(20, 20, 0), Facet(0), SLOOP, owner).expect("open water");

    set_course(&mut state, boat, Direction::South, true);
    state.ticks += FAST_TICKS;
    sail(&mut state);

    assert_eq!(
        state.registry.get::<Position>(boat).map(|p| p.0),
        Some(Point::new(20, 21, 0)),
    );
    assert_eq!(SLOW_TICKS, FAST_TICKS * 4);
}

/// **A ship whose way is blocked furls rather than grinding.** It is reported
/// back so the tick can tell the owner, which is where the message belongs — the
/// same split a collapsing house already uses.
#[test]
fn a_ship_that_cannot_go_on_stops_and_says_so() {
    let mut state = a_sea();
    let (actor, owner) = a_captain(&mut state);
    // Bow one tile off the beach, pointed at it.
    let boat = place(&mut state, actor, Point::new(20, 1, 0), Facet(0), SLOOP, owner).expect("open water");

    set_course(&mut state, boat, Direction::North, false);
    state.ticks += SLOW_TICKS;

    assert_eq!(sail(&mut state), vec![boat], "the shore was not reported");
    assert!(
        state
            .registry
            .get::<openshard_state::components::Sailing>(boat)
            .is_none(),
        "the ship kept grinding against the shore",
    );
    assert_eq!(
        state.registry.get::<Position>(boat).map(|p| p.0),
        Some(Point::new(20, 1, 0)),
    );
}

/// "Stop" is safe to say twice, and safe to say to a ship that never moved.
#[test]
fn furling_a_moored_ship_is_nothing() {
    let mut state = a_sea();
    let (actor, owner) = a_captain(&mut state);
    let boat = place(&mut state, actor, Point::new(20, 20, 0), Facet(0), SLOOP, owner).expect("open water");

    furl(&mut state, boat);
    furl(&mut state, boat);

    state.ticks += SLOW_TICKS;
    assert!(sail(&mut state).is_empty());
    assert_eq!(
        state.registry.get::<Position>(boat).map(|p| p.0),
        Some(Point::new(20, 20, 0)),
    );
}

/// **The step check, end to end.** The map refuses the sea, the deck overturns
/// it, and the hull refuses again — which is the whole of what B1 promises a
/// player.
#[test]
fn a_body_walks_from_the_shore_onto_the_deck_and_not_through_the_hull() {
    let mut state = a_sea();
    let (actor, owner) = a_captain(&mut state);
    // Bow against the shore: the deck at (20, 1), hulls at (19, 1) and (21, 1).
    place(&mut state, actor, Point::new(20, 1, 0), Facet(0), SLOOP, owner).expect("staff-free water");

    let live = state.footing(Facet(0), Doors::AsTheyStand);
    assert_eq!(
        openshard_movement::can_step(&live, Point::new(20, 0, 0), Point::new(20, 1, 0)),
        Some(Point::new(20, 1, 3)),
        "stepping aboard lands on the deck and not in the water",
    );
    assert_eq!(
        openshard_movement::can_step(&live, Point::new(20, 1, 3), Point::new(20, 2, 3)),
        Some(Point::new(20, 2, 3)),
        "and walking aft stays on it",
    );
    assert!(
        openshard_movement::can_step(&live, Point::new(20, 1, 3), Point::new(21, 1, 3)).is_none(),
        "walked straight through the hull",
    );
    assert!(
        openshard_movement::can_step(&live, Point::new(20, 0, 0), Point::new(30, 1, 0)).is_none(),
        "open water with no ship on it is still not walkable",
    );
}
