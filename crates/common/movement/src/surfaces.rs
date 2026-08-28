//! Standable surfaces in one map column.
//!
//! Where a body *could* stand is a movement rule, and it lives here with the
//! rest of them. It was parked beside the file reader for as long as that
//! reader was the only thing owning a tile table; a map column is shared input
//! to movement and the interior index, and this walk is what keeps a floor a
//! body can stand on from being rediscovered with slightly different height
//! arithmetic by each reader.

use openshard_map::map::WorldMap;
use openshard_map::overlay::Cover;
use openshard_tiles::TileData;

/// Every height a body could stand at on one map tile.
///
/// This is deliberately a candidate list rather than a movement decision.
/// Walls, doors and the space above the surface are the caller's additional
/// questions. The order is the map file's own static order, with land first.
pub fn stand_surfaces(map: &WorldMap, tiledata: &TileData, x: u16, y: u16, swimming: bool) -> Vec<i32> {
    let mut surfaces = Vec::new();
    if let Some(land) = map.land(x, y) {
        let flags = tiledata.land(land.tile).flags;
        if (flags.is_water() && swimming) || (!flags.is_water() && !flags.is_blocking()) {
            surfaces.push(i32::from(
                map.average_land_z(x, y).expect("land was just present"),
            ));
        }
    }
    for item in map.statics_at(x, y) {
        // The same reading of the same table both ends of the wire lay a
        // *placed* static's cover with — a house's floor, a ship's deck — so a
        // shipped platform and a built one are one surface rule rather than
        // two. `Cover::of_static` is where the halved climbable lives.
        let stands = Cover::of_static(tiledata.static_tile(item.tile.0))
            .based_at(item.z)
            .stands();
        surfaces.extend(stands.map(Cover::surface));
    }
    surfaces
}
