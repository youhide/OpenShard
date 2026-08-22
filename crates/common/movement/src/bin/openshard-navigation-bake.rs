use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use clap::Parser;
use openshard_movement::{MapTerrain, NavigationGraph, bake};
use openshard_protocol::world::Facet;
use openshard_uofiles::tiledata::TileData;

#[derive(Debug, Parser)]
#[command(version, about = "Build navigation graphs outside shard startup")]
struct Cli {
    /// Ultima Online Classic install directory.
    #[arg(short, long, env = "OPENSHARD_CLIENT", value_name = "DIR")]
    client: PathBuf,
    /// Facet to build; may be repeated. Defaults to 0.
    #[arg(long, value_name = "N")]
    facet: Vec<u8>,
    /// Explicit destination (valid with exactly one facet).
    #[arg(long, value_name = "FILE")]
    out: Option<PathBuf>,
    /// Build and report, but do not write.
    #[arg(long)]
    dry_run: bool,
}

fn main() -> ExitCode {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("navigation bake: {error}");
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
    eprintln!(
        "navigation bake: reading tiledata.mul from {}",
        cli.client.display()
    );
    let tiles = TileData::load(cli.client.join("tiledata.mul"))?;
    for facet in cli.facet {
        bake_one(&cli.client, Facet(facet), &tiles, cli.out.as_deref(), cli.dry_run)?;
    }
    Ok(())
}

fn bake_one(
    client: &Path,
    facet: Facet,
    tiles: &TileData,
    out: Option<&Path>,
    dry_run: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let started = Instant::now();
    eprintln!("navigation bake: loading facet {facet}");
    // The map first, then the stamp: the revision recorded in the artifact is
    // the revision the graph is about to be built from.
    let map = openshard_uofiles::map::load_facet(client, facet)?;
    let stamp = bake::stamp_of(client, facet, map.revision())?;
    let (width, height) = (map.map().width(), map.map().height());
    eprintln!(
        "navigation bake +{:.3}s: building facet {facet} ({width}x{height})",
        started.elapsed().as_secs_f64(),
    );
    let graph = NavigationGraph::build(&MapTerrain::new(map.map(), tiles), width, height)
        .ok_or("facet dimensions cannot be represented")?;
    let (regions, nodes, edges) = graph.counts();
    let path = out
        .map(Path::to_owned)
        .unwrap_or_else(|| bake::artifact_path(client, facet));
    if dry_run {
        eprintln!(
            "navigation bake +{:.3}s: facet {facet}: {regions} regions, {nodes} nodes, {edges} edges; dry run, would write {}",
            started.elapsed().as_secs_f64(),
            path.display()
        );
    } else {
        let bytes = bake::save(&path, &graph, &stamp)?;
        eprintln!(
            "navigation bake +{:.3}s: facet {facet}: {regions} regions, {nodes} nodes, {edges} edges, {bytes} bytes; wrote {}",
            started.elapsed().as_secs_f64(),
            path.display()
        );
    }
    Ok(())
}
