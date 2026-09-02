//! Scratch: **how sure the prism search is about which way a picture climbs,
//! and how far its own steps sit from the joints the art actually draws.**
//!
//! Two questions grew out of one staircase in Britain — see
//! `docs/render/evidence/pitfalls.md` and `docs/render/README.md`'s 🚩 entry "A fit is
//! scored on its outline, and the surfaces are inside it":
//!
//! - **The axis.** `facing::best_prism` returns one prism and one score, and a
//!   score alone cannot say whether the winner won by a landslide or a coin
//!   flip. The `report` mode below counts both, over every picture the wall
//!   detector reads as a corner (the population `facing::best_prism` is ever
//!   run against — see `occlusion::Shape::of`'s own comment).
//! - **The interior.** `facing::silhouettes_agree` compares two *filled*
//!   silhouettes, so a riser standing a tile's worth of pixels away from the
//!   art's own joint line scores exactly as well as one that lands on it — the
//!   surfaces are inside the outline, where the measure cannot see. `report`
//!   projects every internal step boundary a fitted prism has into the
//!   sprite's own pixel space and measures how far the nearest strong
//!   brightness edge in the art sits from it.
//!
//! ```sh
//! OPENSHARD_CLIENT=… cargo run -p openshard-client-artscan --example prism_axis
//! OPENSHARD_CLIENT=… cargo run -p openshard-client-artscan --example prism_axis -- 1873 1874
//! OPENSHARD_CLIENT=… cargo run -p openshard-client-artscan --example prism_axis -- --debug 1873 1874
//! ```
//!
//! No arguments runs the population report. One or more graphic ids (decimal or
//! `0x`-prefixed hex) runs the original per-picture ranking, unchanged. `--debug`
//! followed by ids writes a marked picture per graphic instead — the art, scaled
//! up, with each internal boundary's own row in red and the strongest brightness
//! edge nearby in lime — under `OPENSHARD_ART_OUT` (`target/art` by default).
//! **The report's own doc says why this exists**: a brightness-edge detector is
//! noisy on stone, and the residual distribution below is not to be trusted
//! until a handful of its worst offenders have been looked at with this.
//!
//! The candidate sweep is `facing::candidates`' own, restated here because that
//! function is private and this is a scratch tool rather than a second reader of
//! the search: boxes to `MAX_PRISM`, then an even climb per (face, treads, top).
//! `MAX_PRISM` is private too and is the one number copied — a disagreement
//! shows up as a candidate this tool ranks and the search never saw, which is
//! visible in the printed count.
//!
//! **The interior projection is no longer a second copy.** It moved into
//! `facing.rs` as `boundary_columns`/`strongest_edge`/`art_xy`/`interiors_agree`
//! this session — `best_prism` now uses the same math as a tie-break between
//! rival climb axes — and this tool calls those instead of restating them, the
//! `silhouettes_agree` precedent for why a second copy of an alignment rule is
//! worth avoiding.

use std::path::{
    Path,
    PathBuf,
};

use openshard_client_render::facing::{
    self,
    Face,
    Facing,
    Prism,
};
use openshard_protocol::wire::Graphic;
use openshard_uofiles::art::Art;
use openshard_uofiles::image::Image;

/// `facing::MAX_PRISM`, which is private. See the module doc.
const MAX_PRISM: u8 = 20;

/// How small the margin over the best rival axis has to be before a fit counts
/// as a coin flip between climb directions.
///
/// Ten thousandths — an order of magnitude under the smallest *confident*
/// margin the handoff measured on a real flight (`0x0751`'s `+0.0775`) and two
/// above the one picture that showed the signature (`0x0756`'s `+0.0024`). The
/// open question this exists to settle: whether "almost a tie between axes" is
/// its own shape class rather than noise. See `docs/render/README.md`'s 🚩
/// entry.
const TIE_MARGIN: f32 = 0.01;

fn main() {
    let dir = PathBuf::from(std::env::var_os("OPENSHARD_CLIENT").expect("client"));
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.first().map(String::as_str) == Some("--debug") {
        let ids: Vec<u16> = args[1..].iter().map(|arg| parse_id(arg)).collect();
        let out = PathBuf::from(std::env::var_os("OPENSHARD_ART_OUT").unwrap_or_else(|| "target/art".into()));
        debug_pictures(&dir, &out, &ids);
        return;
    }

    if args.is_empty() {
        report(&dir);
        return;
    }

    let art = Art::open(&dir).expect("art");
    let sweep = sweep();
    println!("{} candidates in the sweep", sweep.len());

    for arg in args {
        let id = parse_id(&arg);
        let Ok(Some(image)) = art.static_art(Graphic(id)) else {
            println!("{id}: no art");
            continue;
        };
        let ranked = rank(&sweep, &image);
        println!(
            "\n{id} (0x{id:04X})  {}x{}  fits {}",
            image.width(),
            image.height(),
            match ranked[0].0 >= facing::PRISM_FITS {
                true => "yes",
                false => "NO — the table holds no prism for this picture",
            },
        );
        println!(
            "  margin over the best candidate that climbs another way: {:+.4}",
            margin(&ranked),
        );
        for (score, prism) in ranked.iter().take(6) {
            println!(
                "   {score:.4}  {:<9} {:?}",
                match axis(prism) {
                    Some(face) => format!("{face:?}"),
                    None => "box".to_string(),
                },
                prism.treads(),
            );
        }
    }
}

/// A graphic id off the command line, decimal or `0x`-prefixed hex.
fn parse_id(arg: &str) -> u16 {
    match arg.strip_prefix("0x").or_else(|| arg.strip_prefix("0X")) {
        Some(hex) => u16::from_str_radix(hex, 16).expect("graphic in hex"),
        None => arg.parse().expect("graphic"),
    }
}

/// Every prism the search considers, in the order `facing::candidates` does —
/// restated because that function is private. See the module doc.
fn sweep() -> Vec<(Prism, Image)> {
    let mut sweep: Vec<Prism> = (0..=MAX_PRISM).map(Prism::box_of).collect();
    for up in [Face::North, Face::East, Face::South, Face::West] {
        for treads in 2..=facing::MAX_TREADS {
            for top in 1..=u16::from(MAX_PRISM) {
                let profile: Vec<u8> = (1..=treads).map(|i| (top * i / treads) as u8).collect();
                sweep.push(Prism::new(up, &profile).expect("a legal profile by construction"));
            }
        }
    }
    sweep
        .into_iter()
        .map(|prism| {
            let silhouette = facing::prism_silhouette(&prism);
            (prism, silhouette)
        })
        .collect()
}

/// Which way a prism climbs, or `None` for a box — [`Prism::up`] answers for a
/// one-tread prism the way it happened to be constructed, and nothing about the
/// shape depends on it, so a box is its own kind rather than a fifth direction.
fn axis(prism: &Prism) -> Option<Face> {
    match prism.treads().len() {
        1 => None,
        _ => Some(prism.up()),
    }
}

/// Every candidate scored against `image`, best first.
fn rank<'a>(sweep: &'a [(Prism, Image)], image: &Image) -> Vec<(f32, &'a Prism)> {
    let mut ranked: Vec<(f32, &Prism)> = sweep
        .iter()
        .map(|(prism, silhouette)| (facing::silhouettes_agree(image, silhouette), prism))
        .collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
    ranked
}

/// How far the winner's score sits above the best candidate that climbs a
/// *different* axis — the number the open question is about.
///
/// # Panics
///
/// If `ranked` holds no rival axis to the winner, which cannot happen for
/// [`sweep`]'s own output: it always carries boxes and all four faces.
fn margin(ranked: &[(f32, &Prism)]) -> f32 {
    let winner = ranked[0].1;
    let rival = ranked
        .iter()
        .find(|(_, prism)| axis(prism) != axis(winner))
        .expect("a sweep with boxes and four faces in it always has a rival");
    ranked[0].0 - rival.0
}

/// Every column of one internal boundary's residual against the art, in view
/// pixels — empty where the picture has nothing there or nothing strong enough
/// to count as an edge (see [`facing::STRONG_EDGE`]).
///
/// The projection and the edge search are [`facing::boundary_columns`],
/// [`facing::art_xy`] and [`facing::strongest_edge`]'s own — [`facing::interiors_agree`]
/// is built from the same three calls, counting confident columns rather than
/// measuring how far off they are. This is the population-report's own use of
/// that math, not a second copy of it.
fn edge_residuals(image: &Image, up: Face, run: f32, z: u8) -> Vec<(u16, f32)> {
    let mut out = Vec::new();
    for (column, model_row) in facing::boundary_columns(up, run, z) {
        let (x, model_y) = facing::art_xy(image, column, model_row);
        if let Some((y, strength)) = facing::strongest_edge(image, x, model_y) {
            if strength >= facing::STRONG_EDGE {
                out.push((column, (y - model_y).abs() as f32));
            }
        }
    }
    out
}

/// A fitted prism's mean residual across every internal boundary it has, under
/// **both readings of where a joint is**, and how many columns the second was
/// taken over — `None` if not one column found a confident edge anywhere.
///
/// - `crest` is the original: the distance to the top of the riser, `z` of the
///   tread being climbed onto. It is what `docs/render/README.md`'s 🚩 entry's
///   own "mean 8.35 view px" was measured with, kept so the two are comparable.
/// - `riser` is the reading [`facing::interiors_agree`] now takes: a joint is a
///   vertical face with **two** drawn ends, and the residual is to the nearer of
///   them. A single line was a convention, and half the time it was the other
///   one.
///
/// A box has no internal boundary and is never offered here — see
/// [`report`]'s own gate.
fn measure_residual(prism: &Prism, image: &Image) -> Option<(f32, f32, usize)> {
    let treads = prism.treads();
    let (mut crest_all, mut riser_all) = (Vec::new(), Vec::new());
    for index in 1..treads.len() {
        let run = index as f32 / treads.len() as f32;
        let crest = edge_residuals(image, prism.up(), run, treads[index]);
        let foot = edge_residuals(image, prism.up(), run, treads[index - 1]);
        crest_all.extend(crest.iter().map(|&(_, px)| px));
        // Per column, whichever end of the riser the art drew nearer to. A
        // column that found an edge for only one end keeps that one.
        for &(column, px) in &crest {
            let nearer = match foot.iter().find(|(other, _)| *other == column) {
                Some(&(_, other_px)) => px.min(other_px),
                None => px,
            };
            riser_all.push(nearer);
        }
        for &(column, px) in &foot {
            if !crest.iter().any(|(other, _)| *other == column) {
                riser_all.push(px);
            }
        }
    }
    match riser_all.is_empty() {
        true => None,
        false => {
            Some((
                crest_all.iter().sum::<f32>() / crest_all.len().max(1) as f32,
                riser_all.iter().sum::<f32>() / riser_all.len() as f32,
                riser_all.len(),
            ))
        }
    }
}

/// Step 1 of the handoff's plan: every picture the wall detector reads as a
/// corner — the population [`facing::best_prism`] is ever run against, per
/// `occlusion::Shape::of`'s own gate — scored, tied off against its rival axis,
/// and, where it fits, measured against the art at every internal step.
fn report(dir: &Path) {
    let art = Art::open(dir).expect("art");
    let tiledata = openshard_uofiles::tiledata::load_tiles(dir.join("tiledata.mul")).expect("tiledata.mul");
    let sweep = sweep();

    let (mut accepted, mut rejected) = (0usize, 0usize);
    let (mut accepted_ties, mut rejected_ties) = (0usize, 0usize);
    let mut flipped = 0usize;
    let mut hit_cap = 0usize;
    let mut multi_tread = 0usize;
    // How many treads the accepted fits actually use — the shape of the demand
    // on `facing::MAX_TREADS`, which a single count at the cap cannot show: a
    // cap worth raising is one where the population piles up against it rather
    // than one a few pictures reach.
    let mut profile_sizes: std::collections::BTreeMap<usize, usize> = std::collections::BTreeMap::new();

    struct Row {
        graphic:  u16,
        name:     String,
        treads:   usize,
        crest_px: f32,
        riser_px: f32,
        columns:  usize,
    }
    let mut rows: Vec<Row> = Vec::new();
    let mut undetected: Vec<(u16, String)> = Vec::new();

    for id in 0..=u16::MAX {
        let Ok(Some(image)) = art.static_art(Graphic(id)) else {
            continue;
        };
        // Only what the wall detector calls a corner is ever offered to the
        // prism search — see `occlusion::Shape::of`'s own comment — so this is
        // the population the report's whole errand is about, not a filter of
        // convenience.
        let Some(Facing::Corner { .. }) = facing::facing_of(&image) else {
            continue;
        };

        let ranked = rank(&sweep, &image);
        let (score, winner) = (ranked[0].0, *ranked[0].1);
        let is_fit = score >= facing::PRISM_FITS;
        match is_fit {
            true => accepted += 1,
            false => rejected += 1,
        }
        if margin(&ranked) < TIE_MARGIN {
            match is_fit {
                true => accepted_ties += 1,
                false => rejected_ties += 1,
            }
            // Whether `facing::best_prism`'s own tie-break actually moves the
            // answer for this near-tied picture — the number
            // `docs/render/README.md`'s 🚩 entry wants to know once
            // `interiors_agree` exists: does the fix do anything on real
            // content, not just in principle. Only meaningful where the
            // plain-silhouette winner has an axis to disagree about — the
            // tie-break's own scope.
            if is_fit && winner.treads().len() >= 2 && facing::best_prism(&image).0.up() != winner.up() {
                flipped += 1;
            }
        }
        if !is_fit {
            continue;
        }
        if winner.treads().len() == usize::from(facing::MAX_TREADS) {
            hit_cap += 1;
        }
        *profile_sizes.entry(winner.treads().len()).or_default() += 1;
        // A box has no internal boundary — nothing for the interior term to
        // measure — so it counts towards the axis tally above and stops here.
        if winner.treads().len() < 2 {
            continue;
        }
        multi_tread += 1;
        let name = tiledata.static_tile(id).name.clone();
        match measure_residual(&winner, &image) {
            Some((crest_px, riser_px, columns)) => {
                rows.push(Row {
                    graphic: id,
                    name,
                    treads: winner.treads().len(),
                    crest_px,
                    riser_px,
                    columns,
                })
            }
            None => undetected.push((id, name)),
        }
    }

    rows.sort_by(|a, b| b.riser_px.total_cmp(&a.riser_px));

    println!("{} pictures read as a corner", accepted + rejected);
    println!(
        "  {accepted:>5} fit a prism (score >= {:.2}) — 'the table holds a prism for them'",
        facing::PRISM_FITS,
    );
    println!("  {rejected:>5} did not");

    println!("\ncoin-flip axis margin (< {TIE_MARGIN:.3}), see docs/render/README.md's 🚩 entry:");
    println!("  {accepted_ties:>5} of the {accepted} fitted");
    println!("  {rejected_ties:>5} of the {rejected} refused");
    println!(
        "  {flipped:>5} of those {accepted_ties} accepted near-ties flip axis under facing::best_prism's own interiors_agree tie-break",
    );

    println!(
        "\n{hit_cap:>5} of {multi_tread} multi-tread prisms use every one of facing::MAX_TREADS = {} treads",
        facing::MAX_TREADS,
    );
    for (treads, count) in &profile_sizes {
        // And how well each size answers for its own interior: a tread count
        // that only ever improves the *outline* shows up here as a residual no
        // better than the coarser profiles', which is what says the extra
        // treads are fitting a shape rather than finding drawn steps.
        let of_size: Vec<f32> = rows
            .iter()
            .filter(|row| row.treads == *treads)
            .map(|row| row.riser_px)
            .collect();
        match of_size.is_empty() {
            true => println!("  {count:>5} fits use {treads} treads"),
            false => {
                println!(
                    "  {count:>5} fits use {treads} treads — mean residual {:.2} px",
                    of_size.iter().sum::<f32>() / of_size.len() as f32,
                )
            }
        }
    }

    println!("\ninternal-boundary residual — model joint vs. nearest strong art edge, view px:");
    println!(
        "  {:>5} multi-tread prisms measured\n  {:>5} found no confident edge anywhere on their own steps",
        rows.len(),
        undetected.len(),
    );
    if !rows.is_empty() {
        // Both readings of where a joint is, side by side — see
        // `measure_residual`. The `crest` row is the number
        // `docs/render/README.md`'s 🚩 entry carries; the `riser` row is what
        // the same pictures say once a joint is allowed to be the face it is
        // rather than one of its two edges by convention.
        for (label, values) in [
            (
                "crest only",
                rows.iter().map(|row| row.crest_px).collect::<Vec<f32>>(),
            ),
            (
                "nearer riser end",
                rows.iter().map(|row| row.riser_px).collect::<Vec<f32>>(),
            ),
        ] {
            let mut values = values;
            let mean = values.iter().sum::<f32>() / values.len() as f32;
            values.sort_by(f32::total_cmp);
            let percentile = |p: f32| values[((values.len() - 1) as f32 * p).round() as usize];
            println!(
                "  {label:<17}  mean {mean:.2}  median {:.2}  p90 {:.2}  max {:.2}",
                percentile(0.5),
                percentile(0.9),
                percentile(1.0),
            );
        }
        println!("\n  worst by residual to the nearer riser end:");
        for row in rows.iter().take(15) {
            println!(
                "   {:>6.2} px (crest {:>6.2})  0x{:04X}  {} treads  {:>3} cols  {}",
                row.riser_px, row.crest_px, row.graphic, row.treads, row.columns, row.name,
            );
        }
    }
    if !undetected.is_empty() {
        println!("\n  no confident edge at all:");
        for (id, name) in undetected.iter().take(15) {
            println!("   0x{id:04X}  {name}");
        }
    }
}

/// Scale a marked picture is drawn at — `tests/artshot.rs`'s own, so a person
/// comparing the two tools is comparing the same picture at the same size.
const SCALE: u32 = 6;

/// One picture no real 16-bit art pixel can be, so transparency and the marked
/// background are never confused with a colour the artist drew — the same
/// background `tests/artshot.rs` uses.
const CANVAS: [u8; 3] = [64, 0, 96];

/// `--debug`: one marked picture per graphic, art scaled up with each internal
/// boundary's own row in red and the nearest strong brightness edge found for
/// it in lime — the eyeball check [`report`]'s own doc says has to happen
/// before its distribution is believed.
fn debug_pictures(dir: &Path, out_dir: &Path, ids: &[u16]) {
    let art = Art::open(dir).expect("art");
    let tiledata = openshard_uofiles::tiledata::load_tiles(dir.join("tiledata.mul")).expect("tiledata.mul");
    let sweep = sweep();
    std::fs::create_dir_all(out_dir).expect("the output directory");

    for &id in ids {
        let Ok(Some(image)) = art.static_art(Graphic(id)) else {
            println!("0x{id:04X}: no art");
            continue;
        };
        let ranked = rank(&sweep, &image);
        let (score, prism) = (ranked[0].0, *ranked[0].1);
        println!(
            "0x{id:04X} {}  score {score:.4}  margin {:+.4}  {:?} {:?}",
            tiledata.static_tile(id).name,
            margin(&ranked),
            prism.up(),
            prism.treads(),
        );
        if prism.treads().len() < 2 {
            println!("  a box: no internal boundary to mark");
            continue;
        }

        let (w, h) = (u32::from(image.width()), u32::from(image.height()));
        let (sw, sh) = (w * SCALE, h * SCALE);
        let mut rgb = vec![CANVAS; (sw * sh) as usize];
        for y in 0..h {
            for x in 0..w {
                let Some(pixel) = image.pixel(x as u16, y as u16) else {
                    continue;
                };
                if pixel.is_transparent() {
                    continue;
                }
                let openshard_uofiles::color::Rgb8 { red, green, blue } = pixel.rgb8();
                stroke(&mut rgb, sw, sh, x as i32, y as i32, [red, green, blue]);
            }
        }

        let treads = prism.treads();
        let mut columns_marked = 0usize;
        let mut columns_confident = 0usize;
        for (index, &crest) in treads.iter().enumerate().skip(1) {
            let run = index as f32 / treads.len() as f32;
            for (column, model_row) in facing::boundary_columns(prism.up(), run, crest) {
                let (x, model_y) = facing::art_xy(&image, column, model_row);
                stroke(&mut rgb, sw, sh, x, model_y, [255, 64, 64]);
                columns_marked += 1;
                if let Some((edge_y, strength)) = facing::strongest_edge(&image, x, model_y) {
                    if strength >= facing::STRONG_EDGE {
                        stroke(&mut rgb, sw, sh, x, edge_y, [64, 255, 64]);
                        columns_confident += 1;
                    }
                }
            }
        }
        println!("  {columns_confident} of {columns_marked} columns found a confident edge");

        let bytes: Vec<u8> = rgb.iter().flat_map(|p| p.iter().copied()).collect();
        let path = out_dir.join(format!("0x{id:04X}_interior.png"));
        openshard_client_render::png::write(&path, sw, sh, &bytes).expect("the picture writes");
        println!("  -> {}", path.display());
    }
}

/// One art pixel of overlay, at art coordinates, in the scaled image —
/// `tests/artshot.rs`'s own `stroke`, restated because that one is not `pub`.
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
