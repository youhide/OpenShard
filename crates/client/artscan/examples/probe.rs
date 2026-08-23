//! Scratch: what a point of a real house actually receives, and what stopped
//! the ray that did not reach it.
//!
//! The instrument for "the floor is dark but the wall above it is not": a column
//! of spots up one tile, each asked as the surface it is — a face, a flat — with
//! the flame that reached it and the cell that stopped the ones that did not.

use openshard_client_render::atlas::StaticAtlas;
use openshard_client_render::camera::Camera;
use openshard_client_render::cutaway::Cutaway;
use openshard_client_render::facing::Face;
use openshard_client_render::geometry::Vec2;
use openshard_client_render::light::{self, Spot};
use openshard_protocol::world::Point;
use openshard_uofiles::art::Art;

fn main() {
    let dir = std::path::PathBuf::from(std::env::var_os("OPENSHARD_CLIENT").expect("client"));
    let tiledata = openshard_uofiles::tiledata::load_tiles(dir.join("tiledata.mul")).expect("tiledata");
    let map = openshard_uofiles::map::read_facet(&dir, 0).expect("felucca");
    let art = Art::open(&dir).expect("art");

    let args: Vec<String> = std::env::args().skip(1).collect();
    let px: f32 = args[0].parse().expect("x");
    let py: f32 = args[1].parse().expect("y");
    let (x, y) = (px as u16, py as u16);

    let camera = Camera::new(Point::new(x, y, 0), 1200, 900);
    // The pictures the client would have packed for this frame: every static the
    // camera can see. Without them every occluder is a whole tile, which is a
    // different branch of the walk from the one a real client takes.
    let mut wanted = Vec::new();
    for tile_x in x - 12..=x + 12 {
        for tile_y in y - 12..=y + 12 {
            for item in map.statics_at(tile_x, tile_y) {
                wanted.push(item.tile);
            }
        }
    }
    let atlas = StaticAtlas::build(&art, wanted).expect("atlas");

    let lighting = light::collect(
        &map,
        &[],
        &camera,
        &tiledata,
        &Cutaway::OPEN,
        light::NIGHT,
        &light::Tuning::DEFAULT,
        0.0,
        Some(&atlas),
        None,
    );
    println!("{} flames in the frame", lighting.lights.len());
    for (index, flame) in lighting.lights.iter().enumerate() {
        println!("  [{index}] at {:?} z {}", flame.at, flame.z);
    }

    let at = Vec2::new(px, py);
    let tile = (px.floor() as i32, py.floor() as i32);
    println!("probing at ({px}, {py})");
    let heights: Vec<f32> = match std::env::var("ZS") {
        Ok(list) => list.split(',').map(|z| z.trim().parse().expect("z")).collect(),
        Err(_) => vec![20.0, 30.0, 38.0, 40.0, 42.0, 45.0, 50.0, 55.0],
    };
    for z in heights {
        for (name, spot) in [
            ("flat  ", Spot::flat(at, z, tile)),
            ("uprght", Spot::at(at, z, tile)),
            ("south ", Spot::face(at, z, tile, Face::South)),
            ("east  ", Spot::face(at, z, tile, Face::East)),
        ] {
            let sample = light::sample(spot, &lighting);
            let reached: Vec<String> = sample
                .reaches
                .iter()
                .filter(|reach| reach.within)
                .map(|reach| {
                    format!(
                        "[{}] visible {:.2} delivers {:.2}{}",
                        reach.light.position(),
                        reach.through,
                        reach.delivered,
                        match reach.stopped_by {
                            Some(cell) => format!(" stopped by {cell:?}"),
                            None => String::new(),
                        }
                    )
                })
                .collect();
            println!(
                "z {z:>5}  {name}  brightness {:.4}   {}",
                sample.brightness(),
                reached.join("  "),
            );
        }
    }
}
