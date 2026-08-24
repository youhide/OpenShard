//! Print the baked building label and the wall/door inputs around map tiles.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use openshard_client_artscan::interiors;
use openshard_client_render::doors;
use openshard_movement::bake;
use openshard_protocol::world::Facet;

#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    #[arg(short, long, env = "OPENSHARD_CLIENT", value_name = "DIR")]
    client: PathBuf,
    #[arg(long, default_value_t = 0)]
    facet: u8,
    /// Inspect the artifact of a base-set world instead of the install's.
    ///
    /// The same file `openshard-interiors-bake --base-set` flooded: without it
    /// this reads the install's map and looks for the artifact beside the
    /// install, so a base-set world's flood is invisible to it.
    #[arg(long, value_name = "FILE")]
    base_set: Option<PathBuf>,
    /// Map point as X,Y. May be repeated.
    #[arg(long = "at", required = true, value_parser = parse_point)]
    points: Vec<(u16, u16)>,
    /// Print a square of baked labels around each requested point.
    #[arg(long, default_value_t = 0)]
    radius: u16,
}

fn parse_point(text: &str) -> Result<(u16, u16), String> {
    let (x, y) = text
        .split_once(',')
        .ok_or_else(|| "point must be X,Y".to_string())?;
    Ok((
        x.parse().map_err(|_| "x must be a u16")?,
        y.parse().map_err(|_| "y must be a u16")?,
    ))
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("interiors inspect: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    let facet = Facet(cli.facet);
    let source = cli
        .base_set
        .as_deref()
        .map_or(bake::WorldSource::Install, bake::WorldSource::BaseSet);
    let world = bake::FacetWorld::read(&cli.client, source, facet)?;
    let map = &world.snapshot;
    let stamp = interiors::stamp_of(&cli.client, &world, facet)?;
    let path = interiors::artifact_path(world.artifacts(&cli.client), facet);
    let graph = interiors::load_baked(&path, &stamp)?;
    let tiles = openshard_uofiles::tiledata::load_tiles(cli.client.join("tiledata.mul"))?;
    let table = openshard_client_artscan::load(&cli.client)?;
    // What the interiors bake reads its steps through: this map, this table and
    // the bake over the pair, as one value.
    let spans = openshard_movement::spans::SpanIndex::build(map.map(), &tiles);
    let terrain = openshard_movement::MapTerrain::new(map.map(), &tiles, &spans);
    for (x, y) in cli.points {
        println!(
            "{x},{y}: building {:?}, land {:?}",
            graph.building_at(x, y),
            map.map().land(x, y)
        );
        if graph.building_at(x, y).is_none() {
            if let Some(path) = openshard_client_render::interiors::BuildingMap::exterior_path(
                &terrain,
                &|graphic| table.shape(graphic),
                (x, y),
            ) {
                let last = path.last().expect("non-empty exterior path");
                println!(
                    "  exterior route: {} cardinal steps, exits at {},{}; first steps: {:?}; samples: {:?}",
                    path.len() - 1,
                    last.0,
                    last.1,
                    &path[..path.len().min(80)],
                    path.iter().step_by((path.len() / 8).max(1)).collect::<Vec<_>>(),
                );
            }
        }
        if cli.radius != 0 {
            println!("  labels (. = exterior):");
            for near_y in y.saturating_sub(cli.radius)..=y.saturating_add(cli.radius) {
                print!("  {near_y:>5} ");
                for near_x in x.saturating_sub(cli.radius)..=x.saturating_add(cli.radius) {
                    match graph.building_at(near_x, near_y) {
                        Some(label) => print!("{:02X}", label % 256),
                        None => print!(".."),
                    }
                }
                println!();
            }
        }
        for item in map.map().statics_at(x, y) {
            let tile = tiles.static_tile(item.tile.0);
            println!(
                "  static {:?} z={} flags={:?} height={}{}",
                item.tile,
                item.z,
                tile.flags,
                tile.height,
                if tile.flags.has(openshard_tiles::TileFlags::DOOR) {
                    if doors::is_open(item.tile) {
                        " door=open"
                    } else {
                        " door=closed"
                    }
                } else {
                    ""
                }
            );
        }
        for (name, x, y) in [
            ("north", x, y.checked_sub(1)),
            ("east", x.checked_add(1).unwrap_or(x), Some(y)),
            ("south", x, y.checked_add(1)),
            ("west", x.checked_sub(1).unwrap_or(x), Some(y)),
        ] {
            if let Some(y) = y {
                println!("  {name}: {:?}", graph.building_at(x, y));
            }
        }
    }
    Ok(())
}
