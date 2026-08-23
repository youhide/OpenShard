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

use openshard_client_render::facing::{Face, PRISM_FITS, Prism, best_prism, interiors_agree};
use openshard_protocol::wire::Graphic;
use openshard_uofiles::art::Art;
use openshard_uofiles::color::Color16;
use openshard_uofiles::image::Image;

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
    let tiledata = openshard_uofiles::tiledata::load(dir.join("tiledata.mul"))
        .expect("tiledata.mul")
        .tiles;

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
        check(id, &best, score, &picture);
    }
}

/// What the fit is expected to say about the graphics in [`DEFAULT`].
///
/// Only for those: a run with `OPENSHARD_ART` set is a person looking at
/// something, and asserting about whatever they typed would be asserting about a
/// number nobody has seen yet.
fn check(id: u16, best: &Prism, score: f32, picture: &Image) {
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
            // A box has no interior boundary to ask about.
            assert_eq!(
                interiors_agree(picture, best),
                None,
                "a box has nothing interior to check"
            );
        }
        // The flight: three treads climbing west, five `z` in all. Here the
        // tiledata height *is* what was drawn, which is the other half of the
        // reason the number cannot be trusted: the same field means two things.
        1846 => {
            assert!(score > PRISM_FITS, "the flight fits its stair: {score}");
            assert_eq!(best.top(), 5, "five z of climb");
            assert_eq!(best.treads().len(), 3, "three treads");
            assert_eq!(best.up(), Face::West, "climbing west");
            // The floor the residual measurement set: over the whole install
            // every one of 373 multi-tread fits finds confident edges on its own
            // steps, a mean of five view px from where the model puts them,
            // which is `1 - 5/16` ≈ 0.7 of this measure. Half leaves room for a
            // picture worse than the mean (`SEARCH`/`STRONG_EDGE` are chosen,
            // not measured) while still catching a regression that breaks the
            // projection outright. The wider gate is
            // [`the_flight_this_track_opened_on_still_measures_the_same`], which
            // holds this graphic's neighbours to four decimal places.
            let agree = interiors_agree(picture, best).expect("a stair has an interior to check");
            assert!(agree > 0.5, "the flight's own risers agree with the art: {agree}");
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
    let tiledata = openshard_uofiles::tiledata::load(dir.join("tiledata.mul"))
        .expect("tiledata.mul")
        .tiles;

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

// ---------------------------------------------------------------------------
// The gate under `interiors_agree`: what a regression in it would look like.
//
// `docs/lighting_rebuild.md`'s backlog step 3. The measurement itself
// (`interiors_agree`, and `best_prism`'s tie-break on top of it) was calibrated
// against a report a person ran and read — which is how it *should* have been
// built and is not something a later change has to re-run to stay honest. What
// follows is the pair that makes a change to it fail out loud instead:
//
// - a **hermetic** check, run by `cargo test` with no install, on a drawing this
//   file makes itself — with fault injection into both the art and the model, so
//   the number is shown to move when either side is wrong rather than only when
//   both are right;
// - an **install** check, over the six stair graphics of the flight at
//   `(1454, 1728)` this whole track opened on, holding the numbers the tool
//   printed the day the tie-break landed.
//
// A gate that only asserts a good case passes cannot tell a working detector
// from one that says yes to everything, which is exactly what a coverage
// fraction over a *searched* window is at risk of being.
// ---------------------------------------------------------------------------

/// The projection constants [`shaded_prism`] draws with, restated here on
/// purpose.
///
/// They are private to `facing.rs` and this is a *second statement of the
/// forward drawing* — the whole point of the fixture. `facing`'s own inverse
/// (`boundary_columns` → `art_xy` → `strongest_edge`, and `interiors_agree`
/// over the three) is what the test exercises, so a fixture built by calling
/// that inverse would agree with itself no matter how wrong both were. The same
/// argument `tests/prism.rs`'s own module doc makes for scoring against real
/// sprites, one level down.
///
/// A drift between these and `facing.rs`'s own shows up here as a fixture whose
/// joints the detector cannot find — a failure, not a silence.
const TILE_WIDTH: u16 = 44;
/// Half of it: the tile diamond's own half-width, in view pixels.
const HALF_TILE_WIDTH: f32 = 22.0;
/// View pixels per `z`.
const Z_STEP: f32 = 4.0;

/// The five-bit grey the lowest tread of a [`shaded_prism`] is painted, and the
/// step between one tread's own brightness and the next.
///
/// Six levels of five-bit channel is 50 of eight-bit, so a joint between two
/// treads is a 150-of-765 step in [`facing::STRONG_EDGE`]'s own summed units —
/// comfortably over the 60 it calls confident, and nothing else in the picture
/// steps at all. Chosen to be unambiguous: this fixture is about *where* an edge
/// is found, not about how faint one may be.
const TREAD_GREY: u16 = 10;
/// See [`TREAD_GREY`].
const TREAD_GREY_STEP: u16 = 6;

/// A five-bit grey as a [`Color16`], never transparent.
fn grey(level: u16) -> Color16 {
    Color16((level << 10) | (level << 5) | level)
}

/// A prism drawn the way a client artist would draw one: solid, and with each
/// tread's own material in its own brightness, so the picture has a real
/// brightness edge at every internal joint and nowhere else.
///
/// The sweep is [`facing::prism_silhouette`]'s — a point `(u, v)` of the tile
/// projects to a column and a row, and the material above it rises [`Z_STEP`]
/// per `z` — with two differences that are the fixture's whole substance:
///
/// - samples are painted **far to near** (`u + v` ascending), so where two
///   treads overlap in a column the nearer one covers the further, which is what
///   puts the brightness step at the joint rather than wherever the loop order
///   happened to end;
/// - the colour is the tread's, not one flat fill.
fn shaded_prism(prism: &Prism) -> Image {
    const SAMPLES: i32 = 128;
    let rows = 45 + u16::from(prism.top()) * Z_STEP as u16;
    let mut pixels = vec![Color16::TRANSPARENT; usize::from(TILE_WIDTH) * usize::from(rows)];
    let bottom = f32::from(rows) - 1.0;

    let mut order: Vec<(i32, i32)> = (0..=SAMPLES)
        .flat_map(|i| (0..=SAMPLES).map(move |j| (i, j)))
        .collect();
    order.sort_by_key(|(i, j)| i + j);

    let treads = prism.treads();
    for (i, j) in order {
        let (u, v) = (i as f32 / SAMPLES as f32, j as f32 / SAMPLES as f32);
        let run = match prism.up() {
            Face::East => u,
            Face::West => 1.0 - u,
            Face::South => v,
            Face::North => 1.0 - v,
        };
        let column = ((u - v) * HALF_TILE_WIDTH + f32::from(TILE_WIDTH) / 2.0).floor();
        if column < 0.0 || column >= f32::from(TILE_WIDTH) {
            continue;
        }
        let down = (u + v - 1.0) * HALF_TILE_WIDTH;
        let foot = bottom + down - HALF_TILE_WIDTH;
        let head = foot - f32::from(prism.height_at(run)) * Z_STEP;
        // Which tread this sample stands on — `Prism::height_at`'s own index,
        // which is what makes the colour change exactly where the height does.
        let tread = ((run.clamp(0.0, 1.0) * treads.len() as f32) as usize).min(treads.len() - 1);
        let colour = grey(TREAD_GREY + TREAD_GREY_STEP * tread as u16);
        for row in head.max(0.0).round() as u16..=foot.max(0.0).round() as u16 {
            if row >= rows {
                continue;
            }
            pixels[usize::from(row) * usize::from(TILE_WIDTH) + column as usize] = colour;
        }
    }
    Image::new(TILE_WIDTH, rows, pixels)
}

/// A picture of the same size with no brightness edge anywhere in it — one flat
/// opaque rectangle.
///
/// The art-side fault injection. [`interiors_agree`] searches a window for the
/// art's own strongest step, so the one thing it must never do is find something
/// where the art draws nothing: on this picture every step is zero and the only
/// honest answer is `0.0`.
fn flat_as_a_wall(image: &Image) -> Image {
    let pixels = vec![grey(TREAD_GREY); image.pixels().len()];
    Image::new(image.width(), image.height(), pixels)
}

/// The three ways [`interiors_agree`] can be wrong, each shown moving the
/// number: the right model on the right art, the right model on art with no
/// joints in it, and a *rotated* model on the right art.
///
/// Hermetic — no install, so this one runs in CI beside everything else. The
/// numbers it holds are of the fixture, not of the client's own art;
/// [`the_flight_this_track_opened_on_still_measures_the_same`] is the half that
/// answers for real sprites.
#[test]
fn a_drawn_joint_is_found_where_the_model_puts_it() {
    // `0x0736`'s own profile, the flight `tests/prism.rs` already fits: three
    // treads climbing west, five `z` in all.
    let prism = Prism::new(Face::West, &[1, 3, 5]).expect("a legal profile");
    let art = shaded_prism(&prism);

    let agree = interiors_agree(&art, &prism).expect("a stair has an interior to check");
    // `0.9375` is `1 - 1/16` — every column finds its joint one row off, which
    // is `strongest_edge` naming a step by its upper row while the painter names
    // the same step by its lower one. A half-pixel convention between two
    // statements of one edge, and the only systematic error left in a fixture
    // whose joints are exactly where the model says they are.
    assert!(
        agree > 0.9,
        "every joint of a drawing of this prism is within a row of where the prism says: {agree}"
    );

    // The art side: nothing drawn, nothing found.
    let flat = interiors_agree(&flat_as_a_wall(&art), &prism).expect("still a stair");
    assert_eq!(flat, 0.0, "a picture with no edge in it agrees with no joint");

    // The model side. A rotated prism is a *legal* shape with the same outline
    // this drawing has — `silhouettes_agree` cannot separate the two at all on a
    // symmetric profile, which is the whole reason the interior term exists.
    // The three rivals scored `0.764` (north), `0.750` (east) and `0.774`
    // (south) against the right prism's `0.938` when this was written. The
    // margin asked for is a tenth — well inside that gap, and far enough above
    // zero that a measure which has stopped discriminating fails here rather
    // than passing quietly. **East is the one that matters**: the same stair
    // mirrored, whose outline is identical and which the presence-counting first
    // version of this measure scored a perfect `1.0`.
    for up in [Face::North, Face::East, Face::South] {
        let rotated = Prism::new(up, &[1, 3, 5]).expect("a legal profile");
        let wrong = interiors_agree(&art, &rotated).expect("still a stair");
        println!("climbing {up:?} instead: {wrong}");
        assert!(
            agree - wrong > 0.1,
            "a prism climbing {up:?} agrees with a west-climbing drawing materially less: {wrong} against {agree}"
        );
    }

    // And the whole chain: what the search returns for a picture of a known
    // shape is that shape.
    let (best, score) = best_prism(&art);
    assert_eq!(best.up(), Face::West, "the search reads the drawn climb");
    assert_eq!(best.treads(), [1, 3, 5], "and the drawn profile");
    assert!(score > PRISM_FITS, "a drawing of a prism fits a prism: {score}");
}

/// The six stair graphics standing at `(1454, 1728)` in Britain — the flight
/// `docs/lighting_state.md`'s 🚩 entry was reported on — and what the fit says
/// about each.
///
/// Every number was
/// printed by this test's own run on 2026-08-11, with the sharpened
/// `interiors_agree` step 3 arrived at; they are a *record of what the detector
/// said*, not a target it was tuned to. A change that moves one is not
/// necessarily wrong — it is required to say so out loud.
///
/// Two of the six are worth naming. **`0x0750` is a box** — the landing at the
/// top, five `z` of stone with no steps drawn on it — and it is here so the
/// fixture holds the shape the interior term must answer `None` for rather than
/// a zero. **`0x0756` does not fit at all** (`0.8929`, under [`PRISM_FITS`]):
/// it is the picture whose rival axes came within `+0.0024` of each other, the
/// coin flip this whole track started from, and the fit it is refused *for* is
/// recorded so that a change which quietly starts accepting it has to say so.
const FLIGHT: &[Recorded] = &[
    Recorded::new(0x0750, Face::East, &[5], 0.9773, None),
    Recorded::new(0x0751, Face::North, &[1, 3, 5], 0.9752, Some(0.7976)),
    Recorded::new(0x0752, Face::West, &[1, 3, 5], 0.9752, Some(0.8152)),
    Recorded::new(0x0754, Face::East, &[1, 3, 5], 0.9726, Some(0.8288)),
    Recorded::new(0x0756, Face::North, &[1, 2, 3, 4], 0.8929, Some(0.6947)),
    Recorded::new(0x0758, Face::East, &[1, 3, 5], 0.9726, Some(0.7514)),
];

/// One graphic of [`FLIGHT`] and what the detector said about it: the climb axis
/// and profile `best_prism` returned, the outline score it returned them with,
/// and what its own art agrees with — `None` for a box, which has no interior to
/// agree about.
struct Recorded {
    graphic: u16,
    up: Face,
    treads: &'static [u8],
    score: f32,
    interiors: Option<f32>,
}

impl Recorded {
    const fn new(graphic: u16, up: Face, treads: &'static [u8], score: f32, interiors: Option<f32>) -> Self {
        Self {
            graphic,
            up,
            treads,
            score,
            interiors,
        }
    }
}

/// How far a golden above may drift before it counts as a change.
///
/// Half of the last digit the table is written to. Both numbers are pure
/// functions of a fixed sprite, so a run that reads the same art computes the
/// same value bit for bit — this is the width of the rounding in the table
/// itself, not room for the detector to move in.
const GOLDEN_SLACK: f32 = 0.00005;

/// The install half of step 3's gate: the real flight, its recorded numbers, and
/// the same two fault injections the hermetic half makes.
#[test]
#[ignore = "reads a real install and prints for a person"]
fn the_flight_this_track_opened_on_still_measures_the_same() {
    let Some(dir) = client_dir() else {
        return;
    };
    let art = Art::open(&dir).expect("artLegacyMUL.uop");

    for recorded in FLIGHT {
        let (id, score_was, agree_was) = (recorded.graphic, recorded.score, recorded.interiors);
        let picture = art
            .static_art(Graphic(id))
            .expect("the art reads")
            .expect("a graphic the flight is built of");
        let (best, score) = best_prism(&picture);
        let agree = interiors_agree(&picture, &best);
        println!(
            "0x{id:04X}  {:?} {:?}  score {score:.4}  interiors {}",
            best.up(),
            best.treads(),
            match agree {
                Some(value) => format!("{value:.4}"),
                None => "n/a (a box)".to_string(),
            },
        );

        assert_eq!(best.up(), recorded.up, "0x{id:04X} climbs the way it did");
        assert_eq!(
            best.treads(),
            recorded.treads,
            "0x{id:04X} has the profile it did"
        );
        assert!(
            (score - score_was).abs() < GOLDEN_SLACK,
            "0x{id:04X} fits at {score}, recorded {score_was}"
        );
        match (agree, agree_was) {
            (Some(agree), Some(was)) => assert!(
                (agree - was).abs() < GOLDEN_SLACK,
                "0x{id:04X} agrees with its own art at {agree}, recorded {was}"
            ),
            (None, None) => {}
            (agree, was) => panic!("0x{id:04X} has an interior to check now ({agree:?}), recorded {was:?}"),
        }
        // A box stops here: the two injections below are about a joint, and it
        // has none.
        if best.treads().len() < 2 {
            continue;
        }

        // Art-side fault injection, the same one the hermetic half makes: a
        // picture with no brightness step in it must agree with nothing.
        assert_eq!(
            interiors_agree(&flat_as_a_wall(&picture), &best),
            Some(0.0),
            "0x{id:04X} with its own shading flattened agrees with no joint"
        );

        // Model-side: the same profile climbing every other way. Printed for a
        // person, and asserted only as an *ordering* — the winner agrees with
        // the art at least as well as any rotation of itself. That is the
        // property the tie-break rests on, stated over real sprites rather than
        // over this file's own drawing; a measure that has stopped
        // discriminating fails it.
        //
        // **Only where the picture fits.** `0x0756` is the counter-example and
        // it is the informative one: refused at `0.8929`, its own returned
        // profile agrees with the art at `0.6947` while the same profile
        // climbing *east* agrees at `0.7120`. A shape no prism holds has no
        // interior claim to be right about, which is the same thing the whole
        // population said when a coin-flip axis margin turned out to be a
        // refused picture's signature (45.9% of them) rather than an accepted
        // one's (10.4%).
        for rotated in [Face::North, Face::East, Face::South, Face::West] {
            if score < PRISM_FITS {
                break;
            }
            if rotated == best.up() {
                continue;
            }
            let other = Prism::new(rotated, best.treads()).expect("the same profile, another way");
            let value = interiors_agree(&picture, &other).expect("still a stair");
            println!("   climbing {rotated:?} instead: {value:.4}");
            assert!(
                agree.expect("a multi-tread fit has an interior") >= value,
                "0x{id:04X} agrees with its own art at least as well as it does climbing {rotated:?}: {agree:?} against {value}"
            );
        }
    }
}
