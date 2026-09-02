//! Read an install's art, measure every picture in it, and write the table the
//! client loads.
//!
//! ```sh
//! OPENSHARD_CLIENT=… cargo run -p openshard-client-artscan
//! ```
//!
//! **Eleven seconds** on a 2D client of 39,189 pictures, in an ordinary
//! `cargo run` — measured, and lower than this file first guessed, because the
//! two crates that do the work are already at `opt-level = 1` in the dev profile
//! (the root `Cargo.toml` says why). Worth knowing precisely, because the budget
//! is the whole point of `docs/archive/render/lighting.md`'s decision 31: what a measurement may
//! cost here is a minute, and today's spends eleven seconds of it.
//!
//! Four of those seconds were the faces and the windows; the prism search
//! (`facing::best_prism`, step 22's climbable statics) is the other seven, over
//! the 4,362 pictures the face detector calls corners. It was **more than ten
//! minutes** when it landed, and this is the one place that number is visible:
//! the search redrew the same 261 candidate silhouettes for every one of those
//! pictures, and the atlas pays the same cost per graphic it packs when there is
//! no table beside the install to read instead — which was a client that took
//! half a minute to put a window up. The candidates are drawn once now and a
//! candidate that cannot beat the best score found is not walked at all; see
//! `facing::candidates`.
//!
//! What it prints is the coverage, because a detector with no coverage count is a
//! green light for having checked nothing: how many pictures were looked at, how
//! many were read, the breakdown by face, and the corners and windows as their
//! own numbers rather than as shares — a tail hidden in a percentage is a tail
//! that can go to zero unnoticed.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use openshard_client_artscan::{
    self as artscan,
    ScanError,
};
use openshard_client_render::arttable::ArtTable;
use openshard_client_render::facing::Facing;
use openshard_protocol::wire::Graphic;
use openshard_uofiles::art::Art;

/// Measure a client's art and write the table the renderer reads.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Cli {
    /// The client directory to measure — the one with `artLegacyMUL.uop` in it.
    #[arg(short, long, env = "OPENSHARD_CLIENT", value_name = "DIR")]
    client: PathBuf,

    /// Where to write the table. Beside the install by default, which is what
    /// the client looks at.
    #[arg(short, long, value_name = "FILE")]
    out: Option<PathBuf>,

    /// Measure and report, and write nothing.
    #[arg(long)]
    dry_run: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<(), ScanError> {
    let art = Art::open(&cli.client).map_err(ScanError::Art)?;
    let stamp = artscan::stamp_of(&cli.client)?;
    let out = cli
        .out
        .clone()
        .unwrap_or_else(|| artscan::table_path(&cli.client));

    // Both sources of authored rows, and they are the same kind of thing: the
    // sheet this repository ships, and whatever a person has already edited into
    // the table beside this install. Decision 31.2 is one file format and one
    // precedence rule, so this is one merge over two tables rather than two code
    // paths that have to agree.
    let mut keep = vec![artscan::overrides()];
    if let Some(beside) = artscan::authored_beside(&out)? {
        keep.push(beside);
    }
    let kept: usize = keep.iter().map(ArtTable::authored).sum();

    let table = artscan::measure(&art, stamp.clone(), &keep);
    report(&table, kept);

    if cli.dry_run {
        println!("dry run: {} not written", out.display());
        return Ok(());
    }
    artscan::save(&out, &table)?;
    println!("written: {}", out.display());
    println!(
        "stamp:   {} of {} bytes, detector {}",
        stamp.art, stamp.bytes, stamp.detector
    );
    Ok(())
}

/// What the sweep found, in the shape `tests/facing.rs` prints it — because the
/// two are asking the same question of the same install and a person comparing
/// them should not have to translate.
fn report(table: &ArtTable, kept: usize) {
    let examined = table.examined();
    let decided = table.decided();
    let share = match examined {
        0 => 0.0,
        _ => decided as f64 / examined as f64,
    };
    println!("pictures with art: {examined}");
    println!("read:              {decided}  ({:.1}%)", share * 100.0);
    println!("corners:           {}", table.corners());
    println!("windows:           {}", table.holed());
    // The stairs, and the number that says the seconds this run spent searching
    // for them bought something: the prism search is nearly the whole cost of a
    // scan, so a gate that stopped admitting prisms would show up here and
    // nowhere else in this report.
    println!("solids:            {}", table.prisms());
    println!(
        "authored rows:     {} kept, {} in the table",
        kept,
        table.authored()
    );

    // By verdict, because a run of one face and none of the others would be a
    // detector reading its own bias back, and a total cannot say so.
    let mut faces: BTreeMap<String, usize> = BTreeMap::new();
    for id in 0..=u16::MAX {
        let Some(facing) = table.facing(Graphic(id)) else {
            continue;
        };
        let label = match facing {
            Facing::One(face) => format!("{face:?}"),
            Facing::Corner { right, left } => format!("{right:?}+{left:?}"),
        };
        *faces.entry(label).or_default() += 1;
    }
    for (label, count) in &faces {
        println!("  {label:<12} {count}");
    }
}
