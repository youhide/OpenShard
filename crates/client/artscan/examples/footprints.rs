//! How much of the "the art would not say" class the footprint detector reads.
//!
//! `docs/footprints.md`'s S1 gate. The class this plan is about is exactly the
//! pictures [`facing::facing_of`] answers `None` for — `occlusion::edges_of`
//! turns that into `Edges::ANY` and the static is given a whole tile it does not
//! fill. This walks every graphic an install ships, splits it by that verdict,
//! and counts how many of the refused ones [`facing::footprint_of`] measures a
//! box for.
//!
//! What it prints beside the count is the **shape** of what was read, because a
//! detector that answered "the whole tile" for everything would score 100% here
//! and buy nothing: the two spans as histograms in eighths, and the share whose
//! footprint is the whole tile after all.
//!
//! ```sh
//! OPENSHARD_CLIENT=… cargo run --release -p openshard-client-artscan \
//!     --example footprints
//! ```
//!
//! Any arguments are graphics to report individually, which is how a reported
//! picture is turned into a row: `… --example footprints -- 2711 2712 2878`.

use std::collections::BTreeMap;

use openshard_client_render::facing;
use openshard_protocol::wire::Graphic;
use openshard_uofiles::art::Art;

/// How many static graphics an install can hold — the id space, not the count
/// with art in it, which is what the report counts.
const GRAPHICS: u16 = 0x4000;

fn main() {
    let dir = std::path::PathBuf::from(std::env::var_os("OPENSHARD_CLIENT").expect("OPENSHARD_CLIENT"));
    let art = Art::open(&dir).expect("art should open");

    let mut args = std::env::args().skip(1).peekable();
    // **The population that decides this, and it is not the id space.** Most of
    // an install's 16,384 graphics are inventory items — a sword, a potion, a
    // pile of gold — which are never placed in the world and are not boxes by
    // any reading. What `docs/footprints.md` is about is the statics a frame
    // actually draws, weighted by how often each is placed, which is the axis
    // `examples/geometry_census.rs` counts on. `at x y [radius]` is that pass.
    if args.peek().map(String::as_str) == Some("at") {
        args.next();
        let cx: i32 = args.next().expect("x").parse().expect("x is a number");
        let cy: i32 = args.next().expect("y").parse().expect("y is a number");
        let radius: i32 = args.next().map_or(60, |v| v.parse().expect("radius"));
        placed(&dir, &art, cx, cy, radius);
        return;
    }

    let named: Vec<u16> = args
        .map(|arg| arg.parse().expect("a graphic is a number"))
        .collect();
    for id in &named {
        report(&art, *id);
    }
    if !named.is_empty() {
        println!();
    }

    let (mut with_art, mut faced, mut refused, mut measured, mut whole) = (0u32, 0u32, 0u32, 0u32, 0u32);
    // One bucket per eighth of width, `1..=8`; index 0 is unused and stays a
    // visible zero rather than being subtracted somewhere.
    let (mut across_x, mut across_y) = ([0u32; 9], [0u32; 9]);
    // **What was refused and why**, which is the half of a census that says
    // whether a low count is a wrong model or a tight tolerance. The first run
    // of this tool read 4.5% and could not tell those apart.
    let mut reasons: BTreeMap<String, u32> = BTreeMap::new();
    for id in 0..GRAPHICS {
        let Ok(Some(image)) = art.static_art(Graphic(id)) else {
            continue;
        };
        with_art += 1;
        if facing::facing_of(&image).is_some() {
            faced += 1;
            continue;
        }
        refused += 1;
        let footprint = match facing::measure_footprint(&image) {
            Ok(footprint) => footprint,
            Err(why) => {
                *reasons.entry(format!("{why:?}")).or_insert(0u32) += 1;
                continue;
            }
        };
        measured += 1;
        if footprint == facing::Footprint::WHOLE {
            whole += 1;
        }
        across_x[usize::from(footprint.x.max - footprint.x.min)] += 1;
        across_y[usize::from(footprint.y.max - footprint.y.min)] += 1;
    }

    let share = |part: u32, whole: u32| match whole {
        0 => 0.0,
        _ => f64::from(part) / f64::from(whole) * 100.0,
    };
    println!("pictures with art:     {with_art}");
    println!("read as a face:        {faced}  ({:.1}%)", share(faced, with_art));
    println!(
        "the art would not say: {refused}  ({:.1}%)",
        share(refused, with_art)
    );
    println!();
    println!(
        "  measured a footprint: {measured}  ({:.1}% of the refused)",
        share(measured, refused)
    );
    println!(
        "  of those, the whole tile after all: {whole}  ({:.1}%)",
        share(whole, measured)
    );
    println!(
        "  a box narrower than its tile:       {}  ({:.1}%)",
        measured - whole,
        share(measured - whole, measured)
    );
    println!();
    println!("  refused, by reason:");
    for (why, count) in &reasons {
        println!(
            "    {why:<8} {count:>6}  ({:.1}% of the refused)",
            share(*count, refused)
        );
    }
    println!();
    println!("  span in eighths:  1    2    3    4    5    6    7    8");
    println!(
        "             on x: {}",
        across_x[1..]
            .iter()
            .map(|n| format!("{n:<5}"))
            .collect::<String>()
    );
    println!(
        "             on y: {}",
        across_y[1..]
            .iter()
            .map(|n| format!("{n:<5}"))
            .collect::<String>()
    );
}

/// The same question asked of the statics a neighbourhood actually holds.
///
/// The class is `boxes_of`'s own: not climbable, not `BACKGROUND`, and no facing
/// — which is the branch that reaches `edges_of(None)` and gets a whole tile.
/// `examples/geometry_census.rs` in `client/render` counts the same class; this
/// says how much of it a footprint would replace.
fn placed(dir: &std::path::Path, art: &Art, cx: i32, cy: i32, radius: i32) {
    let map = openshard_uofiles::map::read_facet(dir, 0).expect("Felucca");
    let tiledata = openshard_uofiles::tiledata::load(dir.join("tiledata.mul"))
        .expect("tiledata.mul")
        .tiles;

    let (mut total, mut unread, mut measured, mut whole, mut roofs) = (0u32, 0u32, 0u32, 0u32, 0u32);
    let mut reasons: BTreeMap<String, u32> = BTreeMap::new();
    // **Whose sample this is.** A share of a class says nothing about whether it
    // is one graphic placed three thousand times or three thousand graphics; the
    // repair is different in each case, so the census names the pictures.
    let mut refused_by_graphic: BTreeMap<u16, (u32, String)> = BTreeMap::new();
    for x in cx - radius..=cx + radius {
        for y in cy - radius..=cy + radius {
            for item in map.statics_at(x as u16, y as u16) {
                total += 1;
                let tile = tiledata.static_tile(item.tile.0);
                if tile.flags.is_climbable() || tile.flags.is_background() {
                    continue;
                }
                let Ok(Some(image)) = art.static_art(item.tile) else {
                    continue;
                };
                if facing::facing_of(&image).is_some() {
                    continue;
                }
                unread += 1;
                // **A roof is not a box and is not meant to be one.** The single
                // largest thing in this class over Britain is roof pieces — a
                // sloped slab, whose base edge is not two 45° runs and never
                // will be — so counting them against a box detector measures the
                // wrong thing. Named here rather than skipped silently, because
                // `docs/lighting_rebuild.md`'s phase 6i is an open question
                // about exactly these and a share that quietly dropped them
                // would hide it.
                if tile.flags.is_roof() {
                    roofs += 1;
                    continue;
                }
                match facing::measure_footprint(&image) {
                    Ok(footprint) => {
                        measured += 1;
                        whole += u32::from(footprint == facing::Footprint::WHOLE);
                    }
                    Err(why) => {
                        *reasons.entry(format!("{why:?}")).or_insert(0) += 1;
                        let seen = refused_by_graphic
                            .entry(item.tile.0)
                            .or_insert_with(|| (0, format!("{why:?} {:?}", tile.name)));
                        seen.0 += 1;
                    }
                }
            }
        }
    }

    let side = radius * 2 + 1;
    let share = |part: u32, whole: u32| match whole {
        0 => 0.0,
        _ => f64::from(part) / f64::from(whole) * 100.0,
    };
    println!("{total} statics on {side}x{side} tiles around ({cx}, {cy})\n");
    println!(
        "  whole tile, the art would not say: {unread}  ({:.1}% of every static)",
        share(unread, total)
    );
    println!(
        "    of it, ROOF — a sloped slab, no box: {roofs}  ({:.1}%)",
        share(roofs, unread)
    );
    let boxy = unread - roofs;
    println!("    the rest, which a box could be:      {boxy}");
    println!(
        "\n  a footprint measured:  {measured}  ({:.1}% of the rest, {:.1}% of every static)",
        share(measured, boxy),
        share(measured, total)
    );
    println!(
        "    narrower than its tile: {}  ({:.1}% of every static)",
        measured - whole,
        share(measured - whole, total)
    );
    println!("\n  refused, by reason:");
    for (why, count) in &reasons {
        println!("    {why:<8} {count:>6}  ({:.1}%)", share(*count, unread));
    }
    let mut worst: Vec<(u16, u32, String)> = refused_by_graphic
        .into_iter()
        .map(|(graphic, (count, why))| (graphic, count, why))
        .collect();
    worst.sort_by_key(|row| std::cmp::Reverse(row.1));
    println!("\n  the twelve refused most often, and what they are:");
    for (graphic, count, why) in worst.iter().take(12) {
        println!("    0x{graphic:04X}  {count:>5} placed  {why}");
    }
}

/// One graphic, in the shape a person chasing a reported picture wants it.
fn report(art: &Art, id: u16) {
    let Ok(Some(image)) = art.static_art(Graphic(id)) else {
        println!("{id} (0x{id:04X}): no art");
        return;
    };
    println!(
        "{id} (0x{id:04X})  {}x{}  facing {:?}  footprint {:?}",
        image.width(),
        image.height(),
        facing::facing_of(&image),
        facing::measure_footprint(&image),
    );
}
