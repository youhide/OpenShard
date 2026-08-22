//! Print what the client's own files hold on one tile: the land, every static,
//! and the tiledata flags each of them is read for.
//!
//! A diagnostic, not a feature — the thing to reach for when a step is allowed
//! somewhere it looks like it should not be, or refused where it looks like it
//! should not. Reads `OPENSHARD_CLIENT` like every other test here.
//!
//! ```sh
//! cargo run -p openshard-uofiles --example tile_probe -- 6044 1999
//! ```

use openshard_uofiles::tiledata::TileData;

fn main() {
    let mut args = std::env::args().skip(1);
    let first = args.next().expect("x, or `art <graphic>`");
    if first == "art" {
        // The other question this answers: not what is on a tile, but what one
        // graphic *is* — the same table, read the way a placed item is read.
        let dir = std::path::PathBuf::from(std::env::var_os("OPENSHARD_CLIENT").expect("OPENSHARD_CLIENT"));
        let tiles = TileData::load(dir.join("tiledata.mul")).expect("tiledata should load");
        let graphic: u16 = args
            .next()
            .expect("graphic")
            .parse()
            .expect("graphic is a number");
        let data = tiles.static_tile(graphic);
        println!(
            "static 0x{graphic:04X} ({graphic}) height {} blocking {} platform {} name {:?} flags {:?}",
            data.height,
            data.flags.is_blocking(),
            data.flags.is_platform(),
            data.name,
            data.flags
        );
        return;
    }
    let x: u16 = first.parse().expect("x is a number");
    let y: u16 = args.next().expect("y").parse().expect("y is a number");
    let facet: u8 = args.next().map_or(0, |f| f.parse().expect("facet is a number"));

    let dir = std::path::PathBuf::from(std::env::var_os("OPENSHARD_CLIENT").expect("OPENSHARD_CLIENT"));
    let map = openshard_uofiles::map::read_facet(&dir, facet).expect("the facet should load");
    let tiles = TileData::load(dir.join("tiledata.mul")).expect("tiledata should load");

    if let Some(land) = map.land(x, y) {
        let data = tiles.land(land.tile.0);
        println!(
            "land 0x{:04X} z {} {:?} blocking {} name {:?}",
            land.tile.0,
            land.z,
            data.flags,
            data.flags.is_blocking(),
            data.name
        );
    } else {
        println!("no land: off the map");
    }
    for item in map.statics_at(x, y) {
        let data = tiles.static_tile(item.tile.0);
        println!(
            "static 0x{:04X} z {} height {} blocking {} platform {} name {:?}",
            item.tile.0,
            item.z,
            data.height,
            data.flags.is_blocking(),
            data.flags.is_platform(),
            data.name
        );
    }
}
