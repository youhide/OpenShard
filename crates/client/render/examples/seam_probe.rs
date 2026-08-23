//! What boxes a tile's own statics stand, and which of them can answer a side face.
//!
//! The last question the picture cannot answer on its own. `View::Normal` shows
//! a dashed one-pixel line along a floor's tile seams, each pixel carrying a side
//! face's normal and naming the *neighbouring* slab
//! (`docs/lighting_rebuild.md`'s floor entry). [`impostor::meets`] refuses a side
//! face to a **lid** — `hi.z > lo.z` guards both of them, since a lid's sides are
//! lines — so a pixel that got one was met against a box that is not a lid. This
//! prints, for the real graphics on the real tiles, exactly which box each static
//! stands and which of them have height.
//!
//! Reads the client's own files and no GPU: `boxes_of` is fed the same
//! [`occlusion::Shape`] the grid feeds it, off the same atlas.
//!
//! ```sh
//! OPENSHARD_CLIENT=… cargo run --release -p openshard-client-render \
//!     --example seam_probe -- 1501 1659
//! ```

use std::path::PathBuf;

use openshard_client_render::atlas::StaticAtlas;
use openshard_client_render::occlusion;
use openshard_protocol::wire::Graphic;
use openshard_uofiles::art::Art;

/// How many tiles either side of the named one to print.
const AROUND: i32 = 1;

fn main() {
    let dir = PathBuf::from(std::env::var_os("OPENSHARD_CLIENT").expect("OPENSHARD_CLIENT"));
    let mut args = std::env::args().skip(1);
    let cx: i32 = args.next().expect("x").parse().expect("x is a number");
    let cy: i32 = args.next().expect("y").parse().expect("y is a number");

    let map = openshard_uofiles::map::read_facet(&dir, 0).expect("Felucca");
    let tiledata = openshard_uofiles::tiledata::load_tiles(dir.join("tiledata.mul")).expect("tiledata.mul");
    let art = Art::open(&dir).expect("artLegacyMUL.uop");

    // The atlas the grid would have: every graphic standing on the tiles below,
    // packed the same way, because `Shape` — the facing, the hole, the prism —
    // is read off it and a graphic that is not packed reads as the whole tile.
    let mut graphics: Vec<Graphic> = Vec::new();
    for x in cx - AROUND..=cx + AROUND {
        for y in cy - AROUND..=cy + AROUND {
            for item in map.statics_at(x as u16, y as u16) {
                graphics.push(item.tile);
            }
        }
    }
    graphics.sort_unstable_by_key(|g| g.0);
    graphics.dedup();
    let atlas = StaticAtlas::build(&art, graphics.iter().copied()).expect("these few fit");

    for x in cx - AROUND..=cx + AROUND {
        for y in cy - AROUND..=cy + AROUND {
            println!("tile {x}, {y}");
            for item in map.statics_at(x as u16, y as u16) {
                let graphic = item.tile;
                let tile = tiledata.static_tile(graphic.0);
                let shape = occlusion::shape_of(
                    Some(openshard_client_render::atlas::StaticArt::Single(&atlas)),
                    graphic,
                );
                print!(
                    "  0x{:04X} z {:>3} {:?} facing {:?} prism {}",
                    graphic.0,
                    item.z,
                    tile.flags,
                    shape.facing,
                    shape.prism.is_some(),
                );
                let opacity = occlusion::opacity(graphic, tile);
                println!("  opacity {opacity}");
                occlusion::boxes_of(x, y, item.z, tile, &shape, |part, edges, solid| {
                    // The one thing this probe exists to say: a box with height
                    // has side faces and a lid does not, and only the first can
                    // answer a fragment with `+x` or `+y`.
                    let flat = solid.min.z == solid.max.z;
                    println!(
                        "      {part:?} {edges:?}  x {:.3}..{:.3}  y {:.3}..{:.3}  z {:.1}..{:.1}  {}",
                        solid.min.x,
                        solid.max.x,
                        solid.min.y,
                        solid.max.y,
                        solid.min.z,
                        solid.max.z,
                        if flat {
                            "a lid: no side face"
                        } else {
                            "**has height: can answer a side face**"
                        },
                    );
                });
            }
        }
    }
}
