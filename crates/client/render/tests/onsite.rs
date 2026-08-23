//! What the lighting knows about a named place in the world the client ships.
//!
//! The scenes in `lighting.rs` are built, which is what makes them readable and
//! is also what they cannot answer: a report from a player is a *coordinate*, and
//! between that coordinate and a built scene there is a chain of guesses — which
//! graphics stand there, which of them the art names a face for, what the
//! occlusion grid ends up holding. Every one of those is somewhere a synthetic
//! reproduction can quietly differ from the thing complained about, and a scene
//! built from a wrong guess is a green test beside a defect that is still there.
//!
//! So this file is the instrument in between: point it at a tile, and it prints
//! what stands there, what the atlas read off each picture, what the grid ended
//! up with, and what a ray from a given flame does on the way. It asserts almost
//! nothing — it is the eye, in the same sense `frame.rs`'s frame dump is — and
//! what it is *for* is turning a coordinate into the handful of facts a built
//! scene has to reproduce.
//!
//! Ignored and gated on `OPENSHARD_CLIENT`: no client files live in this
//! repository. Run it with one:
//!
//! ```sh
//! OPENSHARD_CLIENT=… cargo test -p openshard-client-render --test onsite -- \
//!     --ignored --nocapture
//! ```
//!
//! `OPENSHARD_AT=x,y` moves it; the default is the corner of the house a lamp
//! was reported leaking round — see `docs/lighting.md`, and the backlog entry
//! "found at a house corner in Britain".

use std::path::PathBuf;

use openshard_client_render::animate::StaticAnimations;
use openshard_client_render::atlas::StaticAtlas;
use openshard_client_render::camera::{Camera, TileBounds};
use openshard_client_render::cutaway::Cutaway;
use openshard_client_render::facing::Face;
use openshard_client_render::geometry::Vec2;
use openshard_client_render::items::GroundItem;
use openshard_client_render::light::{self, Spot};
use openshard_client_render::occlusion;
use openshard_client_render::place::Stance;
use openshard_client_render::statics;
use openshard_protocol::items::ItemAmount;
use openshard_protocol::wire::{Graphic, Hue};
use openshard_protocol::world::Point;
use openshard_uofiles::art::Art;

/// The place the reports came from: the lamp is on `(1441, 1693)` and the house
/// corner it stands against is the tile north of it.
const REPORTED: (u16, u16) = (1441, 1692);

/// And where the flame stands: the tile south of it, in the street.
///
/// Put there by this test rather than found on the map — nothing burns within
/// forty tiles of that corner in Felucca, and what the report was about is a
/// light *a player was carrying*. A ground item is the same flame to everything
/// downstream, and it means the ray this prints is the one complained about
/// rather than one from whichever lamp happened to be nearest.
const LAMP_AT: (u16, u16) = (1441, 1693);

/// What that flame is: a lantern, which the client's own tiledata flags
/// `LIGHT_SOURCE` and gives no `NO_SHOOT` — so it burns rather than being one of
/// decision 19's windows. Printed below, because a graphic that stopped burning
/// in a later client would otherwise turn this whole probe blank.
const LAMP: Graphic = Graphic(0x0A15);

/// How many tiles either side of the place are printed.
const AROUND: i32 = 3;

/// The client's files, or `None` when the environment does not point at any.
fn client_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("OPENSHARD_CLIENT")?))
}

/// The tile to look at: `OPENSHARD_AT=x,y`, or [`REPORTED`].
fn place() -> (u16, u16) {
    let Some(text) = std::env::var_os("OPENSHARD_AT") else {
        return REPORTED;
    };
    let text = text.to_string_lossy().to_string();
    let (x, y) = text.split_once(',').expect("OPENSHARD_AT is x,y");
    (x.trim().parse().expect("an x"), y.trim().parse().expect("a y"))
}

/// Everything the lighting reads about a place, printed.
///
/// Four blocks, and the order is the order a leak is diagnosed in:
///
/// 1. **What stands there** — every static on every tile of the square, with the
///    height and flags `occlusion` reads and the face the atlas measured off the
///    picture. A graphic whose face is `None` is a whole-tile occluder and an
///    `Upright` sprite, which is the answer everything falls back to and the
///    answer two of the reported defects turned out to be made of.
/// 2. **The grid** — one line a tile: the span, the opacity, and the edge mask.
/// 3. **The flames** — what `light::collect` found and where it put them.
/// 4. **A sweep of rays** — the ground around the place, sampled at a third of a
///    tile, printed as how much of the nearest flame reaches it. A leak through a
///    corner is a stripe in that picture and is invisible in a per-tile diagram,
///    which samples one point a tile and would step straight over it.
#[test]
#[ignore = "reads a real install and prints for a person; asserts almost nothing"]
fn what_the_lighting_knows_about_a_place() {
    let Some(dir) = client_dir() else {
        return;
    };
    let (at_x, at_y) = place();
    let map = openshard_uofiles::map::read_facet(&dir, 0).expect("Felucca");
    let tiledata = openshard_uofiles::tiledata::load(dir.join("tiledata.mul"))
        .expect("tiledata.mul")
        .tiles;
    let art = Art::open(&dir).expect("artLegacyMUL.uop");

    // The camera is only here to give `light::collect` the same bounds the client
    // would have: a place is looked at from where a player stands on it.
    let centre = Point::new(at_x, at_y, map.average_land_z(at_x, at_y).unwrap_or(0));
    let camera = Camera::new(centre, 768, 512);
    let atlas = StaticAtlas::build(
        &art,
        statics::visible_graphics(&map, &camera, &StaticAnimations::default()),
    )
    .expect("the statics of one screen fit");

    println!("\n=== what stands at ({at_x}, {at_y}) ± {AROUND} ===");
    for y in at_y as i32 - AROUND..=at_y as i32 + AROUND {
        for x in at_x as i32 - AROUND..=at_x as i32 + AROUND {
            let (x, y) = (x as u16, y as u16);
            for item in map.statics_at(x, y) {
                let graphic = item.tile;
                let tile = tiledata.static_tile(item.tile.0);
                let facing = atlas.sprite(graphic).and_then(|sprite| sprite.facing);
                println!(
                    "({x}, {y}) z {:>4}  {graphic:?}  h {:>3}  facing {:?}  stance {:?}  \
                     opacity {:>3}  burns {}  climbable {}",
                    item.z,
                    tile.height,
                    facing,
                    Stance::of(tile, facing),
                    occlusion::opacity(graphic, tile),
                    light::burns(graphic, tile),
                    tile.flags.is_climbable(),
                );
            }
        }
    }

    // The flame the report is about, standing in the street where the player was.
    let lamp = [GroundItem {
        amount: ItemAmount::ONE,
        at: Point::new(LAMP_AT.0, LAMP_AT.1, 0),
        graphic: LAMP,
        hue: Hue::NONE,
    }];
    println!(
        "\nthe lamp is {LAMP:?} at {LAMP_AT:?}, and it burns: {}",
        light::burns(LAMP, tiledata.static_tile(LAMP.0)),
    );

    let bounds = TileBounds {
        min_x: i32::from(at_x) - AROUND - 8,
        max_x: i32::from(at_x) + AROUND + 8,
        min_y: i32::from(at_y) - AROUND - 8,
        max_y: i32::from(at_y) + AROUND + 8,
    };
    let grid = occlusion::collect(&map, &lamp, bounds, &tiledata, &Cutaway::OPEN, Some(&atlas));
    println!("\n=== the grid ===");
    for y in at_y as i32 - AROUND..=at_y as i32 + AROUND {
        for x in at_x as i32 - AROUND..=at_x as i32 + AROUND {
            let Some(cell) = grid.at(x, y) else {
                continue;
            };
            println!(
                "({x}, {y})  z {}..={}  opacity {:>3}  edges {}",
                cell.bottom,
                cell.top,
                cell.opacity,
                edges(cell.edges),
            );
        }
    }

    let lighting = light::collect(
        &map,
        &lamp,
        &camera,
        &tiledata,
        &Cutaway::OPEN,
        light::NIGHT,
        &light::Tuning::DEFAULT,
        0.0,
        Some(&atlas),
        // No bake: one frame, and the instrument reports what the plain walk
        // built.
        None,
    );
    println!("\n=== the flames, nearest first ===");
    let mut flames: Vec<_> = lighting
        .lights
        .iter()
        .map(|flame| {
            let (dx, dy) = (flame.at.x - f32::from(at_x), flame.at.y - f32::from(at_y));
            ((dx * dx + dy * dy).sqrt(), flame)
        })
        .collect();
    flames.sort_by(|a, b| a.0.total_cmp(&b.0));
    println!("{} in the frame", flames.len());
    for (away, flame) in flames.iter().take(8) {
        println!(
            "({}, {}) z {}  radius {}  {away:.1} tiles away",
            flame.at.x, flame.at.y, flame.z, flame.radius,
        );
    }

    // A third of a tile, because a leak through a corner is thinner than a tile
    // and a per-tile diagram walks straight over it. `#` is a ray that is stopped,
    // a space is one that arrives whole, and the digits in between are tenths.
    println!("\n=== how much of the nearest flame reaches the ground ===");
    let step = 1.0 / 3.0;
    let ramp = |through: f32| match through {
        t if t <= 0.0 => '#',
        t if t >= 1.0 => ' ',
        t => char::from_digit((t * 10.0) as u32, 10).unwrap_or('?'),
    };
    for row in -AROUND * 3..=AROUND * 3 {
        let y = f32::from(at_y) + 0.5 + row as f32 * step;
        let line: String = (-AROUND * 3..=AROUND * 3)
            .map(|column| {
                let x = f32::from(at_x) + 0.5 + column as f32 * step;
                let spot = Spot::at(Vec2::new(x, y), 0.0, (x.floor() as i32, y.floor() as i32));
                let sample = light::sample(spot, &lighting);
                // The nearest flame that reaches this far at all. `reaches` holds
                // one entry per flame in the frame, most of them out of range, so
                // the first is whichever the map listed first and says nothing.
                let nearest = sample
                    .reaches
                    .iter()
                    .filter(|reach| reach.within)
                    .min_by(|a, b| a.distance.total_cmp(&b.distance));
                match nearest {
                    None => '.',
                    Some(reach) => ramp(reach.through),
                }
            })
            .collect();
        println!("{line}");
    }

    // And the *faces* of the tile being asked about, which the sweep above cannot
    // reach: its pixels are on the ground, and a wall's are not. Each face is
    // sampled at the middle of its own plane, halfway up the wall — the point
    // `statics.wgsl` writes for a pixel there — so what comes out is what the
    // frame will draw on that surface and not an average of the tile.
    //
    // This is the half a report about a glowing wall is made of. `through` is
    // what the walk let past and `cone` is how much of the flame the surface is
    // turned towards: a face behind the flame's plane reads a `through` of 1 and a
    // `cone` of 0, and only the two apart tell that from a shadow.
    println!("\n=== and its four faces, halfway up — and its lid ===");
    let inside = 1.0 - 1.0 / 127.0;
    let tile = (i32::from(at_x), i32::from(at_y));
    // The **lid** first: the top of whatever stands here, sampled the way a flat
    // static's pixel is written — with no face at all, because a horizontal
    // surface has no vertical side and the attachment carries no normal for one.
    // Which is the question a bright diamond at a wall's corner asks: a wall's top
    // cap is a `Flat` sprite, so nothing tests which way it looks and every flame
    // in reach lights the whole of it.
    if let Some(cell) = grid.at(i32::from(at_x), i32::from(at_y)) {
        let at = Vec2::new(f32::from(at_x) + 0.5, f32::from(at_y) + 0.5);
        let sample = light::sample(Spot::flat(at, cell.top as f32, tile), &lighting);
        match sample.reaches.iter().find(|reach| reach.within) {
            None => println!("lid at z {}: no flame reaches it", cell.top),
            Some(reach) => println!(
                "lid at z {}: visible {:.3}  delivers {:.3}  from the flame {:.1} tiles away",
                cell.top, reach.through, reach.delivered, reach.distance,
            ),
        }
    }
    for (face, at, z) in [
        (
            Face::North,
            Vec2::new(f32::from(at_x) + 0.5, f32::from(at_y)),
            10.0,
        ),
        (
            Face::East,
            Vec2::new(f32::from(at_x) + inside, f32::from(at_y) + 0.5),
            10.0,
        ),
        (
            Face::South,
            Vec2::new(f32::from(at_x) + 0.5, f32::from(at_y) + inside),
            10.0,
        ),
        (
            Face::West,
            Vec2::new(f32::from(at_x), f32::from(at_y) + 0.5),
            10.0,
        ),
    ] {
        let sample = light::sample(Spot::face(at, z, tile, face), &lighting);
        let nearest = sample
            .reaches
            .iter()
            .filter(|reach| reach.within)
            .min_by(|a, b| a.distance.total_cmp(&b.distance));
        match nearest {
            None => println!("{face:?}: no flame reaches it"),
            Some(reach) => println!(
                "{face:?}: visible {:.3}  delivers {:.3}  from the flame {:.1} tiles away",
                reach.through, reach.delivered, reach.distance,
            ),
        }
    }
}

/// An edge mask as the four letters, for a line a person reads.
fn edges(mask: occlusion::Edges) -> String {
    let named = [
        (occlusion::Edges::NORTH, 'N'),
        (occlusion::Edges::EAST, 'E'),
        (occlusion::Edges::SOUTH, 'S'),
        (occlusion::Edges::WEST, 'W'),
    ];
    let text: String = named
        .iter()
        .map(|(bit, letter)| match mask.contains(*bit) {
            true => *letter,
            false => '-',
        })
        .collect();
    match mask {
        occlusion::Edges::NONE => format!("{text} (a lid)"),
        occlusion::Edges::ANY => format!("{text} (a whole tile)"),
        _ => text,
    }
}
