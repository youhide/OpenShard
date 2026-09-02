//! One static's picture, written out where a person can look at it.
//!
//! The measurements in [`openshard_client_render::facing`] are all made from a
//! sprite's own pixels, and every one of them was argued from a picture first:
//! a wall's base is a 45° run, a window is a hole with stone either side of it.
//! When a new *shape* turns up — a stair, a ramp, a roof — the first question is
//! always what the artist actually drew, and there is no way to answer it from a
//! hex id.
//!
//! So: a graphic in, a picture out, at whatever scale is legible, with the tile's own
//! diamond and its centre column stroked over the top. The overlay is the point —
//! `facing` reasons in *tile* coordinates (a half-column is 22 pixels, one `z` is
//! four), and a picture without those lines on it cannot be compared with what
//! the detector says about it.
//!
//! Ignored and gated on `OPENSHARD_CLIENT`, like every other test that reads an
//! install:
//!
//! ```sh
//! OPENSHARD_CLIENT=… OPENSHARD_ART=1822,1846 \
//!     cargo test -p openshard-client-render --test artshot -- --ignored --nocapture
//! ```
//!
//! `OPENSHARD_ART` is a comma-separated list of graphic ids, decimal or `0x`
//! hex; the default is the staircase this was written for. They land under
//! `target/art/`, or wherever `OPENSHARD_ART_OUT` points.

use std::fs;
use std::path::PathBuf;

use openshard_protocol::wire::Graphic;
use openshard_uofiles::art::Art;
use openshard_uofiles::color::{
    Color16,
    Rgb8,
};
use openshard_uofiles::image::Image;

/// The graphics this was written for: the two stair statics standing at
/// `(1493, 1639)` in Britain — see `docs/archive/render/lighting.md`'s backlog, "found on a
/// staircase in Britain".
const DEFAULT: &[u16] = &[1822, 1846];

/// How many screen pixels one art pixel gets.
///
/// Four: a stair's step is five `z`, which is twenty pixels of picture, and at
/// one-to-one the whole question — where the surface's rise starts and stops —
/// is a smudge twenty pixels tall.
const SCALE: u32 = 4;

/// A tile's width in the drawn image, and one `z` in it. The same numbers
/// `facing` states for itself, repeated here for the overlay only.
const TILE_WIDTH: i32 = 44;
const Z_STEP: i32 = 4;

fn client_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("OPENSHARD_CLIENT")?))
}

/// The graphics to draw: `OPENSHARD_ART=1822,0x0736`, or [`DEFAULT`].
fn wanted() -> Vec<u16> {
    let Some(text) = std::env::var_os("OPENSHARD_ART") else {
        return DEFAULT.to_vec();
    };
    text.to_string_lossy()
        .split(',')
        .map(|part| {
            let part = part.trim();
            match part.strip_prefix("0x").or_else(|| part.strip_prefix("0X")) {
                Some(hex) => u16::from_str_radix(hex, 16).expect("a hex graphic id"),
                None => part.parse().expect("a decimal graphic id"),
            }
        })
        .collect()
}

/// Where the pictures land.
fn out_dir() -> PathBuf {
    match std::env::var_os("OPENSHARD_ART_OUT") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/art"),
    }
}

#[test]
#[ignore = "reads a real install and writes pictures for a person"]
fn what_the_artist_drew() {
    let Some(dir) = client_dir() else {
        return;
    };
    let art = Art::open(&dir).expect("artLegacyMUL.uop");
    let out = out_dir();
    fs::create_dir_all(&out).expect("a place to write");

    for id in wanted() {
        let Some(image) = art.static_art(Graphic(id)).expect("the art reads") else {
            println!("{id} (0x{id:04X}): no picture");
            continue;
        };
        let path = out.join(format!("{id:05}-0x{id:04X}.png"));
        write_png(&path, &image);
        println!(
            "{id} (0x{id:04X}): {}x{} -> {}",
            image.width(),
            image.height(),
            path.display()
        );
        print_columns(&image);
    }
}

/// The sprite, scaled, with the tile's centre column and the diamond's two lower
/// edges drawn over it.
///
/// The overlay is where the sprite *would* sit if it were a floor: the diamond's
/// south corner is the bottom-centre pixel of the picture, which is where the
/// client stands a static's base. A wall rises from that diamond; a stair should
/// climb along one of its axes, and the point of the drawing is to see which.
fn write_png(path: &std::path::Path, image: &Image) {
    let (w, h) = (u32::from(image.width()), u32::from(image.height()));
    let (sw, sh) = (w * SCALE, h * SCALE);
    // The background is a colour no 16-bit art pixel can be, so that transparent
    // and black are two different things in the picture.
    let mut rgb = vec![[64u8, 0, 96]; (sw * sh) as usize];
    for y in 0..sh {
        for x in 0..sw {
            let pixel = image
                .pixel((x / SCALE) as u16, (y / SCALE) as u16)
                .unwrap_or(Color16::TRANSPARENT);
            if !pixel.is_transparent() {
                let Rgb8 { red, green, blue } = pixel.rgb8();
                rgb[(y * sw + x) as usize] = [red, green, blue];
            }
        }
    }

    // The tile's own geometry, in the sprite's coordinates: the centre column,
    // and the two edges of the diamond whose south corner is the bottom middle.
    let centre = (w as i32) / 2;
    let bottom = h as i32 - 1;
    for step in 0..=TILE_WIDTH / 2 {
        // South-east edge and south-west edge, one pixel down per two across.
        for (dx, mark) in [(step, [255u8, 64, 64]), (-step, [255, 64, 64])] {
            let x = centre + dx;
            let y = bottom - step / 2;
            stroke(&mut rgb, sw, sh, x, y, mark);
        }
    }
    for y in 0..h as i32 {
        stroke(&mut rgb, sw, sh, centre, y, [64, 255, 255]);
    }
    // A rung every `z` up the centre column, so a rise can be counted off the
    // picture rather than measured with a ruler.
    let mut y = bottom;
    while y >= 0 {
        stroke(&mut rgb, sw, sh, centre - 1, y, [255, 255, 64]);
        y -= Z_STEP;
    }

    let bytes: Vec<u8> = rgb.iter().flat_map(|p| p.iter().copied()).collect();
    openshard_client_render::png::write(path, sw, sh, &bytes).expect("the picture writes");
}

/// One art pixel of overlay, at art coordinates, in the scaled image.
fn stroke(rgb: &mut [[u8; 3]], sw: u32, sh: u32, x: i32, y: i32, colour: [u8; 3]) {
    if x < 0 || y < 0 {
        return;
    }
    let (x, y) = (x as u32 * SCALE, y as u32 * SCALE);
    for dy in 0..SCALE {
        for dx in 0..SCALE {
            let (px, py) = (x + dx, y + dy);
            if px < sw && py < sh {
                rgb[(py * sw + px) as usize] = colour;
            }
        }
    }
}

/// Where each column of the picture starts and stops, printed.
///
/// The number `facing` reasons about: the lowest drawn pixel of a column is its
/// base, and the run of those bases across the sprite is what says whether a
/// surface is a wall on one edge (a 45° line) or something else. Printed as an
/// offset from the bottom of the picture so that a climb reads as a climb.
fn print_columns(image: &Image) {
    let (w, h) = (image.width(), image.height());
    let mut bases = String::new();
    let mut tops = String::new();
    for x in 0..w {
        let mut lowest = None;
        let mut highest = None;
        for y in 0..h {
            if !image.pixel(x, y).unwrap_or(Color16::TRANSPARENT).is_transparent() {
                if highest.is_none() {
                    highest = Some(y);
                }
                lowest = Some(y);
            }
        }
        match (lowest, highest) {
            (Some(low), Some(high)) => {
                bases.push_str(&format!("{:>4}", h - 1 - low));
                tops.push_str(&format!("{:>4}", h - 1 - high));
            }
            _ => {
                bases.push_str("   .");
                tops.push_str("   .");
            }
        }
    }
    println!("  base: {bases}");
    println!("  top:  {tops}");
}
