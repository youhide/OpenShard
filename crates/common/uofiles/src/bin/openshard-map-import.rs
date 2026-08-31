//! Import a facet out of a UO install and write it as a base set.
//!
//! The one binary that stands on both sides of the line this track is drawing:
//! it reads the client's files through [`openshard_uofiles::map`] and writes our
//! own format through [`openshard_basemap`]. Run it once per facet and the shard
//! needs neither the install nor this program again.
//!
//! It is not a bake. A navigation graph is derived data that a rebuild can throw
//! away; a base set is **the world**, which is why it lands beside the shard by
//! default rather than beside the client's files.

use std::path::{
    Path,
    PathBuf,
};
use std::process::ExitCode;
use std::time::Instant;

use clap::Parser;
use openshard_protocol::world::Facet;

#[derive(Debug, Parser)]
#[command(version, about = "Import a UO facet into an OpenShard base set")]
struct Cli {
    /// Ultima Online Classic install directory.
    #[arg(short, long, env = "OPENSHARD_CLIENT", value_name = "DIR")]
    client:  PathBuf,
    /// Facet to import; may be repeated. Defaults to 0.
    #[arg(long, value_name = "N")]
    facet:   Vec<u8>,
    /// Explicit destination (valid with exactly one facet).
    #[arg(long, value_name = "FILE")]
    out:     Option<PathBuf>,
    /// Import and report, but do not write.
    #[arg(long)]
    dry_run: bool,
    /// After writing, read the file back and check it is the same world.
    ///
    /// Off by default because it costs a second facet in memory and a second
    /// pass over every tile. It is what the acceptance test does, available to
    /// an operator who wants the same assurance about the file they just made.
    #[arg(long)]
    verify:  bool,
}

fn main() -> ExitCode {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("map import: {error}");
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn run(mut cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    if cli.facet.is_empty() {
        cli.facet.push(0);
    }
    if cli.out.is_some() && cli.facet.len() != 1 {
        return Err("--out requires exactly one --facet".into());
    }
    for facet in cli.facet.clone() {
        import(&cli, Facet(facet))?;
    }
    Ok(())
}

fn import(cli: &Cli, facet: Facet) -> Result<(), Box<dyn std::error::Error>> {
    let started = Instant::now();
    eprintln!("map import: reading facet {facet} from {}", cli.client.display());
    let snapshot = openshard_uofiles::map::load_facet(&cli.client, facet)?;
    let map = snapshot.map();
    eprintln!(
        "map import +{:.3}s: {} ({}x{}), {} statics",
        started.elapsed().as_secs_f64(),
        map.facet_name(),
        map.width(),
        map.height(),
        map.static_count(),
    );

    let path = cli
        .out
        .clone()
        .unwrap_or_else(|| openshard_basemap::default_path(facet));
    if cli.dry_run {
        eprintln!(
            "map import +{:.3}s: dry run, would write {}",
            started.elapsed().as_secs_f64(),
            path.display()
        );
        return Ok(());
    }

    // An import is where a world begins, so this is the one caller that mints an
    // identity rather than carrying one. Printed, because it is what a client's
    // cache and every later squash of this world will be filed under.
    let written = openshard_basemap::write(&path, &snapshot, openshard_basemap::Identity::Mint)?;
    eprintln!(
        "map import +{:.3}s: {} chunks, {} statics, {} bytes, world {:016x}; wrote {}",
        started.elapsed().as_secs_f64(),
        written.chunks,
        written.statics,
        written.bytes,
        written.world.0,
        path.display(),
    );

    if cli.verify {
        verify(&path, &snapshot, started)?;
    }
    Ok(())
}

/// Read the file back and check every tile of it against the facet it came
/// from.
///
/// Every tile, not a sample: a transposed read is the failure mode this whole
/// format is shaped against, and it puts the *right* answers in the *wrong*
/// places — so a sample of a few thousand tiles is exactly what it survives.
fn verify(
    path: &Path,
    original: &openshard_map::snapshot::MapSnapshot,
    started: Instant,
) -> Result<(), Box<dyn std::error::Error>> {
    let read = openshard_basemap::read(path)?;
    if read.facet() != original.facet() || read.revision() != original.revision() {
        return Err("the file came back as a different facet or revision".into());
    }
    let (was, is) = (original.map(), read.map());
    if (was.width(), was.height()) != (is.width(), is.height()) {
        return Err("the file came back a different size".into());
    }

    let mut tiles = 0u64;
    for y in 0..is.height() as u16 {
        for x in 0..is.width() as u16 {
            if was.land(x, y) != is.land(x, y) {
                return Err(format!("the ground at ({x}, {y}) came back different").into());
            }
            if !was.statics_at(x, y).eq(is.statics_at(x, y)) {
                return Err(format!("the statics at ({x}, {y}) came back different").into());
            }
            tiles += 1;
        }
    }
    eprintln!(
        "map import +{:.3}s: verified {tiles} tiles against {}",
        started.elapsed().as_secs_f64(),
        path.display(),
    );
    Ok(())
}
