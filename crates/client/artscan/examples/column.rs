//! Scratch: the whole column at a tile — land, statics, their flags, and what
//! the lighting pass makes of each one.

use openshard_client_render::atlas::StaticAtlas;
use openshard_client_render::occlusion;
use openshard_protocol::wire::Graphic;
use openshard_uofiles::art::Art;
use openshard_uofiles::map::Map;
use openshard_uofiles::tiledata::TileData;

fn main() {
    let dir = std::path::PathBuf::from(std::env::var_os("OPENSHARD_CLIENT").expect("client"));
    let tiledata = TileData::load(dir.join("tiledata.mul")).expect("tiledata");
    let map = Map::load_facet(&dir, 0).expect("felucca");
    let art = Art::open(&dir).expect("art");

    let args: Vec<String> = std::env::args().skip(1).collect();
    let radius: u16 = std::env::var("RADIUS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut spots = Vec::new();
    for pair in args.chunks(2) {
        let cx: u16 = pair[0].parse().expect("x");
        let cy: u16 = pair[1].parse().expect("y");
        for x in cx - radius..=cx + radius {
            for y in cy - radius..=cy + radius {
                spots.push((x, y));
            }
        }
    }
    if std::env::var_os("TALLY").is_some() {
        let (mut lids, mut opaque, mut thick) = (0usize, 0usize, 0usize);
        let mut open: std::collections::BTreeMap<u16, usize> = std::collections::BTreeMap::new();
        for &(x, y) in &spots {
            for item in map.statics_at(x, y) {
                let tile = tiledata.static_tile(item.tile);
                if !tile.flags.is_background() {
                    continue;
                }
                lids += 1;
                match occlusion::opacity(Graphic(item.tile), tile) == 0 {
                    true => *open.entry(item.tile).or_default() += 1,
                    false => opaque += 1,
                }
                if tile.height > 0 {
                    thick += 1;
                }
            }
        }
        println!("lids {lids}  opaque {opaque}  taller than nothing {thick}");
        println!("open lids: {open:?}");
        return;
    }
    for (x, y) in spots {
        let land = map.land(x, y).expect("land");
        let ground = tiledata.land(land.tile);
        println!(
            "tile {x},{y}  land {} (0x{:04X})  z {}  {:?}  {:?}",
            land.tile, land.tile, land.z, ground.name, ground.flags,
        );
        for item in map.statics_at(x, y) {
            let tile = tiledata.static_tile(item.tile);
            let atlas = StaticAtlas::build(&art, [Graphic(item.tile)]).expect("atlas");
            let facing = atlas.sprite(Graphic(item.tile)).and_then(|s| s.facing);
            println!(
                "  static {:>5} (0x{:04X})  z {:>4}  h {:>3}  opacity {:>3}  facing {:?}  edges {:#06b}  {:?}",
                item.tile,
                item.tile,
                item.z,
                tile.height,
                occlusion::opacity(Graphic(item.tile), tile),
                facing,
                match tile.flags.is_background() {
                    true => 0,
                    false => occlusion::edges_of(facing),
                },
                tile.flags,
            );
        }
    }
}
