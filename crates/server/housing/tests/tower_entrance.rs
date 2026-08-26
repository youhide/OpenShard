//! The imported five-storey tower, walked where it was reported on Felucca.
//!
//! This is deliberately an install-gated acceptance test.  A scene with a
//! made-up ground plane can prove the generic stair arithmetic, but it cannot
//! prove that the actual tower template has an approach tread on the actual
//! map tile where the house was placed.

use std::collections::HashMap;
use std::path::PathBuf;

use openshard_housing::template;
use openshard_map::grid::Tile;
use openshard_map::overlay::{Cover, Doors, Overlay};
use openshard_movement::{Footing, MapTerrain, can_step};
use openshard_protocol::wire::Graphic;
use openshard_protocol::world::{Facet, Point};
use openshard_uofiles::multi::{
    Component, is_closed_door_graphic, is_house_banister_graphic, is_house_sign_graphic,
};

/// The new tower placed for the report.  Its selected `0x073C` component is at
/// `(1339, 1900, 3)`, so the approach is directly south of it.
const TOWER_ORIGIN: Point = Point::new(1333, 1882, 0);
const STREET: Point = Point::new(1339, 1902, 0);
const BOTTOM_TREAD: Point = Point::new(1339, 1901, 0);
const REPORTED_TREAD: Point = Point::new(1339, 1900, 3);

fn client_dir() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("OPENSHARD_CLIENT")?);
    dir.join("tiledata.mul").exists().then_some(dir)
}

/// The exact collision projection `housing::footprint_of` makes for an
/// imported design, kept here rather than using a fixture so the test walks
/// the content a player installs.
fn tower_overlay(tiles: &openshard_tiles::TileData, components: &[Component]) -> Overlay {
    let mut by_tile = HashMap::<Tile, Vec<Cover>>::new();
    for component in components.iter().filter(|component| {
        component.drawn()
            && !is_closed_door_graphic(component.graphic)
            && !is_house_banister_graphic(component.graphic)
            && !is_house_sign_graphic(component.graphic)
    }) {
        let spot = component
            .placed_at(TOWER_ORIGIN)
            .expect("the reported tower fits on facet 0");
        by_tile
            .entry(Tile::new(spot.x, spot.y))
            .or_default()
            .extend(Cover::of_static(tiles.static_tile(component.graphic.0)).based_at(spot.z));
    }
    let mut overlay = Overlay::default();
    for (tile, covers) in by_tile {
        overlay.set(tile, covers);
    }
    overlay
}

/// A player can take the two real steps from Felucca ground onto the selected
/// `wooden stairs` (`0x073C`) in `legacy-five-story-tower`.
#[test]
#[ignore = "reads an installed facet and its local custom-house template"]
fn legacy_five_story_tower_entrance_is_walkable_on_the_reported_map_tile() {
    let Some(dir) = client_dir() else {
        eprintln!("OPENSHARD_CLIENT is unset — no installed tower to walk");
        return;
    };
    let templates = template::load_directory(&dir.join("openshard-houses"))
        .expect("the installed custom-house catalogue should decode");
    let tower = templates
        .get("legacy-five-story-tower")
        .expect("the reported tower template should be installed");
    assert!(
        tower.iter().any(|component| component.graphic == Graphic(0x073C)
            && component.placed_at(TOWER_ORIGIN) == Some(REPORTED_TREAD)),
        "the reported wooden stair must stay at its declared world tile"
    );
    assert!(
        tower.iter().any(|component| component.graphic == Graphic(0x073C)
            && component.placed_at(TOWER_ORIGIN) == Some(BOTTOM_TREAD)),
        "a ground-level approach tread is required before the z=3 wooden stair"
    );

    let snapshot = openshard_uofiles::map::load_facet(&dir, Facet(0)).expect("facet 0 should load");
    let tiles =
        openshard_uofiles::tiledata::load_tiles(dir.join("tiledata.mul")).expect("tiledata should load");
    let spans = openshard_movement::spans::SpanIndex::build(snapshot.map(), &tiles);
    let terrain = MapTerrain::new(snapshot.map(), &tiles, &spans);
    let overlay = tower_overlay(&tiles, tower);
    let footing = Footing::new(Some(terrain), &overlay, Doors::AsTheyStand);

    let on_bottom = can_step(&footing, STREET, BOTTOM_TREAD)
        .expect("a player must be able to step from the street onto the tower's first stair tread");
    assert_eq!(on_bottom, Point::new(BOTTOM_TREAD.x, BOTTOM_TREAD.y, 2));
    assert_eq!(
        can_step(&footing, on_bottom, REPORTED_TREAD),
        Some(Point::new(REPORTED_TREAD.x, REPORTED_TREAD.y, 5)),
        "the selected z=3 stair must be reachable from the approach tread"
    );
}
