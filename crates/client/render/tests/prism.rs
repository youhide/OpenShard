//! How well the [`Prism`] model fits the pictures the client actually ships.
//!
//! [`openshard_client_render::facing::prism_silhouette`] is the forward
//! direction: a shape in, the drawing the projection makes of it out. This is the
//! measurement that says whether that shape is the shape the artist drew — every
//! candidate prism is scored against a real sprite by how much of the two
//! silhouettes agree, and the best one is printed with its score.
//!
//! **It is the non-circular check the whole model rests on.** Everything else
//! about a stair — its normals, its occluder, where a pixel of it stands — is
//! derived from the prism, and a prism derived from nothing but our own
//! projection would agree with itself perfectly while describing a shape no
//! client ever drew.
//!
//! Ignored and gated on `OPENSHARD_CLIENT`, like every other test that reads an
//! install:
//!
//! ```sh
//! OPENSHARD_CLIENT=… cargo test -p openshard-client-render --test prism -- \
//!     --ignored --nocapture
//! ```
//!
//! `OPENSHARD_ART=1822,0x0736` picks the graphics; the default is the staircase
//! of `docs/lighting.md`'s backlog entry, a plain wall for contrast, and the
//! floor lid that stands over both.

use openshard_client_render::facing::{Face, PRISM_FITS, Prism, best_prism};
use openshard_protocol::wire::Graphic;
use openshard_uofiles::art::Art;
use openshard_uofiles::tiledata::TileData;

use std::path::PathBuf;

/// What to fit, and why each one is in the list:
///
/// - `1822` and `1846` are the two statics a flight of stairs in Britain is made
///   of — the report this model came from.
/// - `200` is a plain wall, which is *not* a prism, and its score is what says
///   the fit means anything: a measure that likes everything measures nothing.
const DEFAULT: &[u16] = &[1822, 1846, 200];

fn client_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("OPENSHARD_CLIENT")?))
}

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

#[test]
#[ignore = "reads a real install and prints for a person"]
fn which_prism_the_art_is_a_picture_of() {
    let Some(dir) = client_dir() else {
        return;
    };
    let art = Art::open(&dir).expect("artLegacyMUL.uop");
    let tiledata = TileData::load(dir.join("tiledata.mul")).expect("tiledata.mul");

    for id in wanted() {
        let Some(picture) = art.static_art(Graphic(id)).expect("the art reads") else {
            println!("{id} (0x{id:04X}): no picture");
            continue;
        };
        let tile = tiledata.static_tile(id);
        let (best, score) = best_prism(&picture);
        println!(
            "\n{id} (0x{id:04X})  {}x{}  tiledata height {}  climbable {}",
            picture.width(),
            picture.height(),
            tile.height,
            tile.flags.is_climbable(),
        );
        println!("  best fit: {best:?}");
        println!("  agreement: {:.3}", score);
        // What the height *would* be if it came from tiledata, so that the two
        // numbers can be compared without arithmetic in the reader's head. A
        // climbable static's stated height is the full one and a walker stands
        // half way up it — see `movement::scene::stair`.
        println!(
            "  tiledata says {} z, or {} climbed; the art says {} z",
            tile.height,
            tile.height / 2,
            best.top(),
        );
        check(id, &best, score);
    }
}

/// What the fit is expected to say about the graphics in [`DEFAULT`].
///
/// Only for those: a run with `OPENSHARD_ART` set is a person looking at
/// something, and asserting about whatever they typed would be asserting about a
/// number nobody has seen yet.
fn check(id: u16, best: &Prism, score: f32) {
    if std::env::var_os("OPENSHARD_ART").is_some() {
        return;
    }
    match id {
        // The landing: a plain box five `z` tall. Its tiledata height is ten,
        // which is the *full* height a climbable static states and twice what the
        // artist drew — so a model that took its height from the flags would be
        // twice as tall as the picture. The art is what the height comes from.
        1822 => {
            assert!(score > PRISM_FITS, "the landing fits its box: {score}");
            assert_eq!(best.treads(), [5], "a box and not a stair");
        }
        // The flight: three treads climbing west, five `z` in all. Here the
        // tiledata height *is* what was drawn, which is the other half of the
        // reason the number cannot be trusted: the same field means two things.
        1846 => {
            assert!(score > PRISM_FITS, "the flight fits its stair: {score}");
            assert_eq!(best.top(), 5, "five z of climb");
            assert_eq!(best.treads().len(), 3, "three treads");
            assert_eq!(best.up(), Face::West, "climbing west");
        }
        // And the wall, which is the control: no prism is a good picture of it.
        200 => assert!(score < PRISM_FITS, "a wall is not a prism: {score}"),
        _ => {}
    }
}

/// **How much of the install's own `CLIMBABLE` art the model actually covers.**
///
/// A stair whose picture scores below [`PRISM_FITS`] never reaches
/// `tread_top_box_of`/`tread_riser_box_of` at all — `Builder::add` falls back to
/// reading it as a wall corner, `PANEL_THICKNESS`-inset panels and all, which is
/// the exact geometry that reads as a seam short of the tile it stands on. The
/// two graphics [`DEFAULT`] checks (`1822`, `1846`) are known to clear the bar;
/// this is the number for *every* `CLIMBABLE` picture the install ships, so a gap
/// in coverage shows up as a count instead of being inferred from one report.
///
/// Prints one line per miss — the graphic id and its score — so a real failure
/// names the picture to go look at rather than only the tally.
#[test]
#[ignore = "reads a real install and prints for a person"]
fn how_much_of_the_climbable_art_the_prism_model_covers() {
    let Some(dir) = client_dir() else {
        return;
    };
    let art = Art::open(&dir).expect("artLegacyMUL.uop");
    let tiledata = TileData::load(dir.join("tiledata.mul")).expect("tiledata.mul");

    let mut climbable = 0;
    let mut fits = 0;
    let mut misses = Vec::new();
    for id in 0..=u16::MAX {
        let tile = tiledata.static_tile(id);
        if !tile.flags.is_climbable() {
            continue;
        }
        let Ok(Some(picture)) = art.static_art(Graphic(id)) else {
            continue;
        };
        climbable += 1;
        let (_, score) = best_prism(&picture);
        if score > PRISM_FITS {
            fits += 1;
        } else {
            misses.push((id, score));
        }
    }
    println!("climbable pictures: {climbable}");
    println!(
        "fit the prism model: {fits}  ({:.1}%)",
        100.0 * fits as f64 / climbable.max(1) as f64
    );
    for (id, score) in &misses {
        println!("  miss: {id} (0x{id:04X})  agreement {score:.3}");
    }
}
