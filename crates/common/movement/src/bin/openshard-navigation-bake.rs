use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use clap::Parser;
use openshard_map::snapshot::MapSnapshot;
use openshard_movement::bake;
use openshard_protocol::world::Facet;
use openshard_tiles::TileData;

#[derive(Debug, Parser)]
#[command(version, about = "Build navigation graphs outside shard startup")]
struct Cli {
    /// Ultima Online Classic install directory.
    ///
    /// Still required with `--base-set`: a base set holds the map, and
    /// `tiledata.mul` holds what a tile is, which the graph is built out of.
    #[arg(short, long, env = "OPENSHARD_CLIENT", value_name = "DIR")]
    client: PathBuf,
    /// Facet to build; may be repeated. Defaults to 0.
    #[arg(long, value_name = "N")]
    facet: Vec<u8>,
    /// Build over a base set instead of the install's map and statics.
    ///
    /// What `world.base_sets` names in the shard's config. The artifact lands
    /// beside the base set rather than beside the install, and is stamped
    /// against the base set — a graph built over one world must not validate
    /// against the files of another.
    #[arg(long, value_name = "FILE")]
    base_set: Option<PathBuf>,
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
    // One file names one facet's world, so it cannot stand for several — and
    // baking the *same* base set under two facet numbers would write two
    // artifacts of one world, one of them stamped with a facet it is not.
    if cli.base_set.is_some() && cli.facet.len() != 1 {
        return Err("--base-set requires exactly one --facet".into());
    }
    eprintln!(
        "navigation bake: reading tiledata.mul from {}",
        cli.client.display()
    );
    let tiles = openshard_uofiles::tiledata::load_tiles(cli.client.join("tiledata.mul"))?;
    for facet in cli.facet.clone() {
        bake_one(&cli, Facet(facet), &tiles)?;
    }
    Ok(())
}

/// Where a facet's world comes from, and how a graph built over it is stamped.
///
/// The two cases differ in more than which reader runs: they name different
/// inputs, and they put the artifact in different places. That is
/// [`bake::FacetWorld`]'s, shared with the shard's boot and the client; what is
/// left here is the reporting, which is this binary's alone.
fn source(
    cli: &Cli,
    facet: Facet,
) -> Result<(MapSnapshot, bake::Stamp, PathBuf), Box<dyn std::error::Error>> {
    match cli.base_set.as_deref() {
        Some(base_set) => eprintln!(
            "navigation bake: loading facet {facet} from {}",
            base_set.display()
        ),
        None => eprintln!("navigation bake: loading facet {facet}"),
    }
    let source = cli
        .base_set
        .as_deref()
        .map_or(bake::WorldSource::Install, bake::WorldSource::BaseSet);
    // The map first, then the stamp: the revision recorded in the artifact is
    // the revision the graph is about to be built from.
    let world = bake::FacetWorld::read(&cli.client, source, facet)?;
    if world.patches != 0 {
        eprintln!(
            "navigation bake: {} patch(es) applied; facet {facet} is at revision {}",
            world.patches,
            world.snapshot.revision().get()
        );
    }
    let stamp = world.stamp(&cli.client, facet)?;
    let path = world.navigation_path(&cli.client);
    Ok((world.snapshot, stamp, path))
}

fn bake_one(cli: &Cli, facet: Facet, tiles: &TileData) -> Result<(), Box<dyn std::error::Error>> {
    let started = Instant::now();
    let (map, stamp, default_path) = source(cli, facet)?;
    let (width, height) = (map.map().width(), map.map().height());
    eprintln!(
        "navigation bake +{:.3}s: building facet {facet} ({width}x{height})",
        started.elapsed().as_secs_f64(),
    );
    // The construction itself is `bake::build` — the span index, nothing live
    // over it, and the flood — because a client that was handed a world off the
    // wire bakes one too, and two spellings of "what a baked graph is built
    // from" are two graphs that disagree about the same facet.
    let graph = bake::build(&map, tiles).ok_or("facet dimensions cannot be represented")?;
    let (regions, nodes, edges) = graph.counts();
    let path = cli.out.clone().unwrap_or(default_path);
    if cli.dry_run {
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
