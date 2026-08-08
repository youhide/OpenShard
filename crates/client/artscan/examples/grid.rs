//! Scratch: what the occlusion grid actually holds around a place — how many
//! surfaces of each kind, at what opacities and spans.

use openshard_client_render::atlas::StaticAtlas;
use openshard_client_render::camera::Camera;
use openshard_client_render::cutaway::Cutaway;
use openshard_client_render::occlusion::{self, EDGE_ANY};
use openshard_protocol::wire::Graphic;
use openshard_protocol::world::Point;
use openshard_uofiles::art::Art;
use openshard_uofiles::map::Map;
use openshard_uofiles::tiledata::TileData;

fn main() {
    let dir = std::path::PathBuf::from(std::env::var_os("OPENSHARD_CLIENT").expect("client"));
    let tiledata = TileData::load(dir.join("tiledata.mul")).expect("tiledata");
    let map = Map::load_facet(&dir, 0).expect("felucca");
    let art = Art::open(&dir).expect("art");

    let args: Vec<String> = std::env::args().skip(1).collect();
    let x: u16 = args[0].parse().expect("x");
    let y: u16 = args[1].parse().expect("y");
    let camera = Camera::new(Point::new(x, y, 0), 1200, 900);
    let bounds = openshard_client_render::light::lit_tiles(&camera);

    let mut wanted = Vec::new();
    for tx in bounds.min_x..=bounds.max_x {
        for ty in bounds.min_y..=bounds.max_y {
            for item in map.statics_at(tx as u16, ty as u16) {
                wanted.push(Graphic(item.tile));
            }
        }
    }
    let atlas = StaticAtlas::build(&art, wanted).expect("atlas");
    let grid = occlusion::collect(&map, &[], bounds, &tiledata, &Cutaway::OPEN, Some(&atlas));

    let (mut lids, mut panels, mut bodies) = (0usize, 0usize, 0usize);
    let mut opacities: std::collections::BTreeMap<u8, usize> = std::collections::BTreeMap::new();
    let mut spans: std::collections::BTreeMap<i32, usize> = std::collections::BTreeMap::new();
    for tx in bounds.min_x..=bounds.max_x {
        for ty in bounds.min_y..=bounds.max_y {
            for solid in grid.solids_at(tx, ty) {
                match solid.edges {
                    0 => lids += 1,
                    EDGE_ANY => bodies += 1,
                    _ => panels += 1,
                }
                *opacities.entry(solid.opacity).or_default() += 1;
                *spans.entry(solid.top() - solid.bottom()).or_default() += 1;
            }
        }
    }
    println!("lids {lids}  panels {panels}  bodies {bodies}");
    println!("opacities {opacities:?}");
    println!("spans {spans:?}");
}
