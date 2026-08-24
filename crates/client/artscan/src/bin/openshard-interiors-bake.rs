//! Bake facet-wide positive building space from the measured wall catalogue.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use clap::Parser;
use openshard_client_artscan::interiors;
use openshard_movement::bake;
use openshard_protocol::world::Facet;

#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    /// Ultima Online Classic install directory.
    #[arg(short, long, env = "OPENSHARD_CLIENT", value_name = "DIR")]
    client: PathBuf,
    /// Facet to bake. Defaults to Britannia (0).
    #[arg(long, default_value_t = 0, value_name = "N")]
    facet: u8,
    /// Flood a base set instead of the install's map and statics.
    ///
    /// What `world.base_sets` names in the shard's config, and what the client's
    /// own `--base-set` names. The artifact lands beside the base set rather
    /// than beside the install, and is stamped against it — a flood over one
    /// world must not validate against the files of another. `--client` is
    /// still required: the wall catalogue and `tiledata.mul` are the install's
    /// either way.
    #[arg(long, value_name = "FILE")]
    base_set: Option<PathBuf>,
    /// Explicit destination instead of the file beside the world.
    #[arg(long, value_name = "FILE")]
    out: Option<PathBuf>,
    /// Calculate and report without writing an artifact.
    #[arg(long)]
    dry_run: bool,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("interiors bake: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    let started = Instant::now();
    let facet = Facet(cli.facet);
    match cli.base_set.as_deref() {
        Some(base_set) => eprintln!(
            "interiors bake: reading wall catalogue, and facet {facet} from {}",
            base_set.display()
        ),
        None => eprintln!("interiors bake: reading wall catalogue and facet {facet}"),
    }
    let source = cli
        .base_set
        .as_deref()
        .map_or(bake::WorldSource::Install, bake::WorldSource::BaseSet);
    // The stamp is taken after the build, not before: the world it records is
    // the one the flood actually ran over, which is what `build` hands back.
    let (graph, world) = interiors::build(&cli.client, source, facet)?;
    let stamp = interiors::stamp_of(&cli.client, &world, facet)?;
    let (width, height) = graph.dimensions();
    let buildings = graph.building_count();
    let path = cli
        .out
        .unwrap_or_else(|| interiors::artifact_path(world.artifacts(&cli.client), facet));
    if cli.dry_run {
        eprintln!(
            "interiors bake +{:.3}s: facet {facet} ({width}x{height}), {buildings} buildings; dry run, would write {}",
            started.elapsed().as_secs_f64(),
            path.display(),
        );
    } else {
        let bytes = interiors::save(Path::new(&path), &graph, &stamp)?;
        eprintln!(
            "interiors bake +{:.3}s: facet {facet} ({width}x{height}), {buildings} buildings, {bytes} bytes; wrote {}",
            started.elapsed().as_secs_f64(),
            path.display(),
        );
    }
    Ok(())
}
