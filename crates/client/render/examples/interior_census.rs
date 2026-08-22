//! Measure the floor-height differences that an interior floor band must admit.
//!
//! The interior index deliberately does not turn a conventional UO `20 z`
//! storey into a renderer invariant.  Before [`interiors`] gives connected
//! cells a structural `FloorId`, run this against the real map data that will
//! exercise it:
//!
//! ```text
//! OPENSHARD_CLIENT=/path/to/client \
//!   cargo run --release -p openshard-client-render --example interior_census
//! ```
//!
//! With no arguments it samples the documented central Britain and Wrong
//! dungeon regions.  Pass one or more `name:x,y,width,height` regions to
//! inspect a particular building or dungeon room:
//!
//! ```text
//! OPENSHARD_CLIENT=/path/to/client \
//!   cargo run --release -p openshard-client-render --example interior_census -- \
//!   bank:1440,1600,128,128 wrong:1939,215,134,137
//! ```
//!
//! The output is deliberately a distribution, not a proposed constant.  A
//! floor-band tolerance belongs in the index only after this report and the
//! cell-colour debug view agree about which sloped and stair-connected cells
//! form one structural floor.

use std::collections::BTreeMap;
use std::path::PathBuf;

use openshard_client_render::interiors::{BlockRooms, Buildings, Cell, StitchedRooms};
use openshard_map::grid::BlockCoord;
use openshard_map::map::{BLOCK_SIZE, Map};
use openshard_movement::PLAYER_HEIGHT;
use openshard_uofiles::tiledata::TileData;

#[derive(Clone, Debug)]
struct Region {
    name: String,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
}

impl Region {
    fn parse(spec: String) -> Self {
        let (name, numbers) = spec.split_once(':').expect("region is name:x,y,width,height");
        let values: Vec<u16> = numbers
            .split(',')
            .map(|value| value.parse().expect("region coordinate is a u16"))
            .collect();
        let [x, y, width, height] = values.as_slice() else {
            panic!("region is name:x,y,width,height");
        };
        assert!(*width > 0 && *height > 0, "a region has a positive size");
        Self {
            name: name.to_owned(),
            x: *x,
            y: *y,
            width: *width,
            height: *height,
        }
    }

    fn defaults() -> Vec<Self> {
        // `regions.json` calls these Britain and Wrong Entrance.  They are
        // deliberately broad enough to include ordinary floors, slopes and
        // stairs instead of producing a flattering one-house sample.
        vec![
            Self {
                name: "britain".to_owned(),
                x: 1440,
                y: 1600,
                width: 128,
                height: 128,
            },
            Self {
                name: "wrong".to_owned(),
                x: 1939,
                y: 215,
                width: 134,
                height: 137,
            },
        ]
    }

    fn contains(&self, x: u16, y: u16) -> bool {
        let (x, y) = (u32::from(x), u32::from(y));
        let (left, top) = (u32::from(self.x), u32::from(self.y));
        x >= left && x < left + u32::from(self.width) && y >= top && y < top + u32::from(self.height)
    }
}

fn main() {
    let dir = PathBuf::from(std::env::var_os("OPENSHARD_CLIENT").expect("OPENSHARD_CLIENT"));
    let regions: Vec<_> = std::env::args().skip(1).map(Region::parse).collect();
    let regions = (!regions.is_empty())
        .then_some(regions)
        .unwrap_or_else(Region::defaults);
    let map = openshard_uofiles::map::read_facet(&dir, 0).expect("Felucca");
    let tiledata = TileData::load(dir.join("tiledata.mul")).expect("tiledata.mul");

    for region in regions {
        measure(&map, &tiledata, &region);
    }
}

fn measure(map: &Map, tiledata: &TileData, region: &Region) {
    let max_x = region
        .x
        .checked_add(region.width - 1)
        .expect("region stays in map");
    let max_y = region
        .y
        .checked_add(region.height - 1)
        .expect("region stays in map");
    assert!(
        map.contains(region.x, region.y) && map.contains(max_x, max_y),
        "region is on the map"
    );

    let blocks_x = u32::from(region.x) / BLOCK_SIZE..=u32::from(max_x) / BLOCK_SIZE;
    let blocks_y = u32::from(region.y) / BLOCK_SIZE..=u32::from(max_y) / BLOCK_SIZE;
    let mut cells: BTreeMap<_, _> = BTreeMap::new();
    let mut blocks = Vec::new();
    let mut statics = 0usize;
    let mut roofs = 0usize;
    let mut opaque_non_roofs = 0usize;
    for x in blocks_x {
        for y in blocks_y.clone() {
            let block = BlockRooms::bake(map, tiledata, BlockCoord { x, y }).expect("selected map block");
            for cell in block
                .cells()
                .cells()
                .iter()
                .copied()
                .filter(|cell| region.contains(cell.tile.0, cell.tile.1))
            {
                cells.entry(cell.tile).or_insert_with(Vec::new).push(cell);
            }
            blocks.push(block);
        }
    }
    for y in region.y..=max_y {
        for x in region.x..=max_x {
            for item in map.statics_at(x, y) {
                statics += 1;
                let flags = tiledata.static_tile(item.tile.0).flags;
                roofs += usize::from(flags.is_roof());
                opaque_non_roofs += usize::from(
                    !flags.is_roof() && flags.has(openshard_uofiles::tiledata::TileFlags::NO_SHOOT),
                );
            }
        }
    }

    let mut deltas = BTreeMap::<i32, usize>::new();
    let mut joins = 0usize;
    for (&(x, y), column) in &cells {
        for neighbour in [(x.checked_add(1), Some(y)), (Some(x), y.checked_add(1))]
            .into_iter()
            .filter_map(|(x, y)| x.zip(y))
        {
            let Some(other_column) = cells.get(&neighbour) else {
                continue;
            };
            for &one in column {
                for &other in other_column {
                    if body_fits_between(one, other) {
                        joins += 1;
                        *deltas.entry((one.floor_z - other.floor_z).abs()).or_default() += 1;
                    }
                }
            }
        }
    }

    println!(
        "{}: {} cells, {joins} body-compatible cardinal pairs",
        region.name,
        cells.values().map(Vec::len).sum::<usize>()
    );
    let mut seen = 0usize;
    for (delta, count) in deltas {
        seen += count;
        println!(
            "  floor delta {delta:>3}: {count:>7} ({:>6.2}% cumulative)",
            100.0 * seen as f64 / joins.max(1) as f64
        );
    }
    let stitched = StitchedRooms::bake(blocks);
    let sealed_rooms = stitched.rooms().iter().filter(|room| !room.outdoors()).count();
    let sealed_cells: usize = stitched
        .rooms()
        .iter()
        .filter(|room| !room.outdoors())
        .map(|room| room.cells().len())
        .sum();
    let roofed_cells: usize = stitched
        .cells()
        .filter(|cell| roof_above(map, tiledata, *cell))
        .count();
    let roofed_outdoor_cells: usize = stitched
        .rooms()
        .iter()
        .filter(|room| room.outdoors())
        .flat_map(|room| room.cells())
        .filter_map(|&id| stitched.cell(id))
        .filter(|cell| roof_above(map, tiledata, *cell))
        .count();
    let buildings = Buildings::bake(map, tiledata, &stitched);
    let floors: usize = buildings
        .buildings()
        .iter()
        .map(|building| building.floors().len())
        .sum();
    println!(
        "  {statics} statics: {roofs} ROOF, {opaque_non_roofs} opaque non-roof; \
         {roofed_cells} roofed cells ({roofed_outdoor_cells} joined to outdoor); \
         {sealed_rooms} sealed rooms / {sealed_cells} cells; {} indexed buildings, {floors} structural floors",
        buildings.buildings().len(),
    );
}

fn roof_above(map: &Map, tiledata: &TileData, cell: Cell) -> bool {
    map.statics_at(cell.tile.0, cell.tile.1).any(|item| {
        tiledata.static_tile(item.tile.0).flags.is_roof() && i32::from(item.z) - cell.floor_z >= PLAYER_HEIGHT
    })
}

/// The same body-height overlap that admits a horizontal room edge today.
/// This keeps the measurement about structural-floor banding rather than the
/// crawlspaces that `BlockCells` intentionally discarded.
fn body_fits_between(one: Cell, other: Cell) -> bool {
    let floor = one.floor_z.max(other.floor_z);
    let ceiling = one
        .ceiling
        .unwrap_or(i32::MAX)
        .min(other.ceiling.unwrap_or(i32::MAX));
    i64::from(ceiling) - i64::from(floor) >= i64::from(PLAYER_HEIGHT)
}
