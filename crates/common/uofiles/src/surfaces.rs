//! Standable surfaces in one map column.
//!
//! A map column is shared input to movement and the interior index. Keeping
//! this walk beside the file types means a floor that a body can stand on is
//! not rediscovered with slightly different height arithmetic by each reader.

use openshard_map::map::Map;

use crate::tiledata::TileData;

/// Every height a body could stand at on one map tile.
///
/// This is deliberately a candidate list rather than a movement decision.
/// Walls, doors and the space above the surface are the caller's additional
/// questions. The order is the map file's own static order, with land first.
pub fn stand_surfaces(map: &Map, tiledata: &TileData, x: u16, y: u16, swimming: bool) -> Vec<i32> {
    let mut surfaces = Vec::new();
    if let Some(land) = map.land(x, y) {
        let flags = tiledata.land(land.tile.0).flags;
        if (flags.is_water() && swimming) || (!flags.is_water() && !flags.is_blocking()) {
            surfaces.push(i32::from(
                map.average_land_z(x, y).expect("land was just present"),
            ));
        }
    }
    for item in map.statics_at(x, y) {
        let tile = tiledata.static_tile(item.tile.0);
        if tile.flags.is_platform() {
            let height = i32::from(tile.height);
            let stand = i32::from(item.z)
                + if tile.flags.is_climbable() {
                    height / 2
                } else {
                    height
                };
            surfaces.push(stand);
        }
    }
    surfaces
}
