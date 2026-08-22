use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use clap::Parser;
use openshard_map::snapshot::MapSnapshot;
use openshard_movement::{Doors, Footing, MapTerrain, NavigationGraph, Overlay, bake};
use openshard_protocol::world::Facet;
use openshard_uofiles::tiledata::TileData;

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
    let tiles = TileData::load(cli.client.join("tiledata.mul"))?;
    for facet in cli.facet.clone() {
        bake_one(&cli, Facet(facet), &tiles)?;
    }
    Ok(())
}

/// Where a facet's world comes from, and how a graph built over it is stamped.
///
/// The two cases differ in more than which reader runs: they name different
/// inputs, and they put the artifact in different places. Keeping that in one
/// place is what stops a base-set graph from being written beside an install it
/// has nothing to do with.
fn source(
    cli: &Cli,
    facet: Facet,
) -> Result<(MapSnapshot, bake::Stamp, PathBuf), Box<dyn std::error::Error>> {
    let tiledata = cli.client.join("tiledata.mul");
    match cli.base_set.as_deref() {
        Some(base_set) => {
            eprintln!(
                "navigation bake: loading facet {facet} from {}",
                base_set.display()
            );
            // The base set *and* the log beside it, through the one call the
            // shard resolves a world with: a graph built over the base alone
            // would be a graph of a world the shard is not running.
            let openshard_basemap::Loaded {
                snapshot: map,
                log,
                patches,
                ..
            } = openshard_basemap::load(base_set)?;
            if map.facet() != facet {
                return Err(format!(
                    "{} is facet {}, and --facet says {facet}",
                    base_set.display(),
                    map.facet().0
                )
                .into());
            }
            if patches != 0 {
                eprintln!(
                    "navigation bake: {patches} patch(es) applied; facet {facet} is at revision {}",
                    map.revision().get()
                );
            }
            let stamp = bake::stamp_of_base_set(base_set, log.as_deref(), &tiledata, facet, map.revision())?;
            // Beside the base set: the world is what the graph is derived from,
            // and an artifact in the install directory would be found by a
            // shard reading the install and refused for reasons it cannot see.
            let beside = base_set.parent().unwrap_or_else(|| Path::new("."));
            let path = bake::artifact_path(beside, facet);
            Ok((map, stamp, path))
        }
        None => {
            eprintln!("navigation bake: loading facet {facet}");
            // The map first, then the stamp: the revision recorded in the
            // artifact is the revision the graph is about to be built from.
            let map = openshard_uofiles::map::load_facet(&cli.client, facet)?;
            let stamp = bake::stamp_of(&cli.client, facet, map.revision())?;
            let path = bake::artifact_path(&cli.client, facet);
            Ok((map, stamp, path))
        }
    }
}

fn bake_one(cli: &Cli, facet: Facet, tiles: &TileData) -> Result<(), Box<dyn std::error::Error>> {
    let started = Instant::now();
    let (map, stamp, default_path) = source(cli, facet)?;
    let (width, height) = (map.map().width(), map.map().height());
    eprintln!(
        "navigation bake +{:.3}s: building facet {facet} ({width}x{height})",
        started.elapsed().as_secs_f64(),
    );
    // Nothing live: a baked graph is the *static* connectivity of a facet, and
    // a door that happened to be shut when the bake ran is not a property of
    // the ground. See `docs/map/navigation_graph_bake.md`.
    let nothing_placed = Overlay::default();
    let footing = Footing::new(
        Some(MapTerrain::new(map.map(), tiles)),
        &nothing_placed,
        Doors::AsTheyStand,
    );
    let graph =
        NavigationGraph::build(&footing, width, height).ok_or("facet dimensions cannot be represented")?;
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
