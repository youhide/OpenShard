//! The instrument step 4 of `docs/lighting.md`'s 23 asks for: a graphic and a
//! table in, and what a person needs to author a row by hand out — `tests/
//! artshot.rs`'s picture and `tests/prism.rs`'s score, joined into the one loop
//! authoring a candidate actually takes: draw it, score it, see where it
//! disagrees with the art, edit the row, look again.
//!
//! **What "the table" means here is [`OPENSHARD_TABLE`], not the derived one a
//! shard keeps beside its install.** A `prism` is still mostly derived — decision
//! 41's `block` is the one kind of row nothing ever proposes automatically — so
//! this is aimed at `data/overrides.table` by default: the sheet a person edits
//! by hand, and the only file a hand-placed box belongs in.
//!
//! For each graphic this prints what the table currently says — `none`, a
//! `prism`, or a list of `block`s — draws that candidate's silhouette
//! ([`facing::prism_silhouette`] or [`facing::blocks_silhouette`]) and scores it
//! against the real art with [`facing::silhouettes_agree`], the same measurement
//! `tests/prism.rs` already trusts. The picture it writes is the art's own
//! colours where the two agree, and a flat colour for each direction of
//! disagreement: cyan where the art draws and the row does not claim it, red
//! where the row claims ground the art leaves transparent — the worse of the
//! two, per `silhouettes_agree`'s own doc, because it is a shadow with nothing in
//! the picture casting it.
//!
//! With nothing authored yet, this is exactly `tests/artshot.rs`'s picture: the
//! starting point for the first row, before there is anything to score.
//!
//! Ignored and gated on `OPENSHARD_CLIENT`, like every other test that reads an
//! install:
//!
//! ```sh
//! OPENSHARD_CLIENT=… OPENSHARD_ART=1846 OPENSHARD_TABLE=… \
//!     cargo test -p openshard-client-render --test author -- --ignored --nocapture
//! ```
//!
//! `OPENSHARD_ART` is a comma-separated list of graphic ids, decimal or `0x`
//! hex; the default is the staircase `docs/lighting.md`'s backlog was written
//! against. `OPENSHARD_TABLE` picks the table file; the default is the checked-in
//! `data/overrides.table`. Pictures land under `target/art/`, or wherever
//! `OPENSHARD_ART_OUT` points — the same variable `tests/artshot.rs` reads, so
//! the two tools' output sits side by side.

use std::fs;
use std::path::PathBuf;

use openshard_client_render::arttable::ArtTable;
use openshard_client_render::facing;
use openshard_protocol::wire::Graphic;
use openshard_uofiles::art::Art;
use openshard_uofiles::color::{Color16, Rgb8};
use openshard_uofiles::image::Image;

/// The staircase's two statics — see `docs/lighting.md`'s backlog, "found on a
/// staircase in Britain". The two graphics decision 41 was written to give a
/// format to author *into*; nothing about them is authored as blocks yet.
const DEFAULT: &[u16] = &[1822, 1846];

/// How many screen pixels one art pixel gets — `tests/artshot.rs`'s own number,
/// so a picture from either tool reads at the same scale.
const SCALE: u32 = 4;

/// A tile's width in the drawn image, and one `z` in it — `facing`'s own
/// numbers, repeated here for the overlay only.
const TILE_WIDTH: i32 = 44;
const Z_STEP: i32 = 4;

fn client_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("OPENSHARD_CLIENT")?))
}

/// The graphics to look at: `OPENSHARD_ART=1822,0x0736`, or [`DEFAULT`].
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

/// The table to read a candidate off: `OPENSHARD_TABLE=…`, or the checked-in
/// overrides sheet — decision 41's own row grammar has nowhere else to be
/// authored into, since nothing derives a `block`.
fn table_path() -> PathBuf {
    match std::env::var_os("OPENSHARD_TABLE") {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../artscan/data/overrides.table"),
    }
}

/// Where the pictures land — `tests/artshot.rs`'s own variable, so both tools'
/// output ends up in one place.
fn out_dir() -> PathBuf {
    match std::env::var_os("OPENSHARD_ART_OUT") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/art"),
    }
}

#[test]
#[ignore = "reads a real install and writes pictures for a person"]
fn what_the_table_says_against_what_the_artist_drew() {
    let Some(dir) = client_dir() else {
        return;
    };
    let art = Art::open(&dir).expect("artLegacyMUL.uop");
    let tiledata = openshard_uofiles::tiledata::load_tiles(dir.join("tiledata.mul")).expect("tiledata.mul");
    let path = table_path();
    let text = fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    let table = ArtTable::parse(&text).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    let out = out_dir();
    fs::create_dir_all(&out).expect("a place to write");

    println!("table: {} ({} rows)", path.display(), table.len());

    for id in wanted() {
        let Some(picture) = art.static_art(Graphic(id)).expect("the art reads") else {
            println!("\n{id} (0x{id:04X}): no picture");
            continue;
        };
        let tile = tiledata.static_tile(id);
        let shape = table.shape(Graphic(id));
        println!(
            "\n{id} (0x{id:04X})  {}x{}  tiledata height {}  climbable {}",
            picture.width(),
            picture.height(),
            tile.height,
            tile.flags.is_climbable(),
        );

        let mut candidates: Vec<(&str, Image)> = Vec::new();
        if let Some(prism) = shape.prism {
            println!("  prism {:?} treads {:?}", prism.up(), prism.treads());
            candidates.push(("prism", facing::prism_silhouette(&prism)));
        }
        if !shape.blocks.is_empty() {
            println!("  {} block(s):", shape.blocks.blocks().len());
            for block in shape.blocks.blocks() {
                println!(
                    "    x {}..{} y {}..{} z {}..{}",
                    block.x.min, block.x.max, block.y.min, block.y.max, block.z.min, block.z.max
                );
            }
            candidates.push(("blocks", facing::blocks_silhouette(&shape.blocks)));
        }
        if candidates.is_empty() {
            println!("  nothing authored — this is what a first row starts from");
            let path = out.join(format!("{id:05}-0x{id:04X}.author.png"));
            write_png(&path, &picture, None);
            println!("  picture: {}", path.display());
        }
        // Both may be authored at once — decision 41's own point, a stair's base
        // still misreads as a corner independently of whether some other graphic
        // needs an arch's shape — so each candidate is scored and drawn on its
        // own rather than one silently winning.
        let several = candidates.len() > 1;
        for (kind, candidate) in &candidates {
            let score = facing::silhouettes_agree(&picture, candidate);
            println!("  {kind} agreement: {score:.3}");
            let suffix = if several {
                format!(".{kind}")
            } else {
                String::new()
            };
            let path = out.join(format!("{id:05}-0x{id:04X}{suffix}.author.png"));
            write_png(&path, &picture, Some(candidate));
            println!("  picture: {}", path.display());
        }

        // Canvas space: the same width and the same two offsets `write_png` just
        // drew with, so a column printed here is the column that was coloured
        // there — an art narrower than the tile (nearly every stair) is centred,
        // and printing its own columns flush left would silently misalign it
        // against a candidate's.
        let w = picture.width().max(TILE_WIDTH as u16);
        let art_offset = (i32::from(w) - i32::from(picture.width())) / 2;
        print_columns("art", &picture, w, art_offset);
        for (kind, candidate) in &candidates {
            let offset = (i32::from(w) - i32::from(candidate.width())) / 2;
            print_columns(kind, candidate, w, offset);
        }
    }
}

/// The picture a row is edited against: the art's own colours where they agree
/// with `candidate`, and a flat colour for each direction of disagreement —
/// [`facing::silhouettes_agree`]'s own two, seen rather than only scored.
/// `candidate` is `None` before anything is authored, which draws exactly what
/// `tests/artshot.rs` does.
///
/// Aligned the way every measurement in `facing` aligns two silhouettes: the
/// bottom row and the centre column, never a fit slid until it agrees.
fn write_png(path: &std::path::Path, art: &Image, candidate: Option<&Image>) {
    let (aw, ah) = (u32::from(art.width()), u32::from(art.height()));
    let ch = candidate.map_or(0, |image| u32::from(image.height()));
    let w = aw.max(TILE_WIDTH as u32);
    let h = ah.max(ch);
    let (sw, sh) = (w * SCALE, h * SCALE);
    // The background is a colour no 16-bit art pixel can be — `tests/
    // artshot.rs`'s own choice, so transparent and black stay two different
    // things in the picture.
    let mut rgb = vec![[64u8, 0, 96]; (sw * sh) as usize];

    let art_offset = (w as i32 - aw as i32) / 2;
    let cand_offset = (w as i32 - TILE_WIDTH) / 2;
    for y in 0..h as i32 {
        // Rows counted up from the picture's own bottom — `facing::drawn_at`'s
        // own convention, so a mismatch printed here is the mismatch scored.
        let row = h as i32 - 1 - y;
        for x in 0..w as i32 {
            let art_pixel = {
                let ax = x - art_offset;
                (row >= 0 && (row as u32) < ah && ax >= 0 && (ax as u32) < aw)
                    .then(|| art.pixel(ax as u16, (ah as i32 - 1 - row) as u16))
                    .flatten()
            }
            .filter(|pixel| !pixel.is_transparent());
            let candidate_drawn = candidate.is_some_and(|image| {
                let cx = x - cand_offset;
                row >= 0
                    && (row as u32) < ch
                    && cx >= 0
                    && (cx as u32) < u32::from(image.width())
                    && !image
                        .pixel(cx as u16, (ch as i32 - 1 - row) as u16)
                        .unwrap_or(Color16::TRANSPARENT)
                        .is_transparent()
            });
            let colour = match (art_pixel, candidate_drawn) {
                (Some(pixel), true) => {
                    let Rgb8 { red, green, blue } = pixel.rgb8();
                    Some([red, green, blue])
                }
                // the art drew it, the row does not claim it
                (Some(_), false) => Some([80, 200, 255]),
                // the row claims it, the art draws air there — the worse of the
                // two, per `facing::silhouettes_agree`'s own doc
                (None, true) => Some([255, 48, 48]),
                (None, false) => None,
            };
            if let Some(colour) = colour {
                stroke(&mut rgb, sw, sh, x, y, colour);
            }
        }
    }

    // The tile's own geometry, `tests/artshot.rs`'s own overlay: the centre
    // column, the diamond's two lower edges, and a rung every `z` up the column.
    let centre = w as i32 / 2;
    let bottom = h as i32 - 1;
    for step in 0..=TILE_WIDTH / 2 {
        for (dx, mark) in [(step, [255u8, 64, 64]), (-step, [255, 64, 64])] {
            stroke(&mut rgb, sw, sh, centre + dx, bottom - step / 2, mark);
        }
    }
    for y in 0..h as i32 {
        stroke(&mut rgb, sw, sh, centre, y, [64, 255, 255]);
    }
    let mut y = bottom;
    while y >= 0 {
        stroke(&mut rgb, sw, sh, centre - 1, y, [255, 255, 64]);
        y -= Z_STEP;
    }

    let bytes: Vec<u8> = rgb.iter().flat_map(|pixel| pixel.iter().copied()).collect();
    openshard_client_render::png::write(path, sw, sh, &bytes).expect("the picture writes");
}

/// One pixel of overlay, at unscaled picture coordinates — `tests/artshot.rs`'s
/// own helper, duplicated rather than shared: each of these tools is a single
/// binary written out for a person, and the two have never needed to agree on
/// more than the numbers `facing` states for itself.
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

/// Where each column of `image` starts, printed the way `tests/artshot.rs`
/// prints it — an offset from the bottom, so a climb reads as a climb — with
/// `label` naming which picture the numbers are about.
///
/// `image` is read in a canvas `w` wide, `offset` to its left — the same two
/// numbers [`write_png`] just coloured pixels with, so a column printed here is
/// the column that was coloured there. Printing an image's own columns flush
/// left instead would silently misalign it against another image centred
/// differently in the same canvas — nearly every stair, which is narrower than
/// the tile it stands on.
fn print_columns(label: &str, image: &Image, w: u16, offset: i32) {
    let h = image.height();
    let mut bases = String::new();
    for x in 0..w {
        let ix = i32::from(x) - offset;
        let base = (ix >= 0 && (ix as u32) < u32::from(image.width())).then(|| {
            let ix = ix as u16;
            let mut lowest = None;
            for y in 0..h {
                if !image
                    .pixel(ix, y)
                    .unwrap_or(Color16::TRANSPARENT)
                    .is_transparent()
                {
                    lowest = Some(y);
                }
            }
            lowest.map(|low| h - 1 - low)
        });
        match base.flatten() {
            Some(n) => bases.push_str(&format!("{n:>4}")),
            None => bases.push_str("   ."),
        }
    }
    println!("  {label:<6}base: {bases}");
}
