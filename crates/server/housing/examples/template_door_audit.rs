//! List every art tile that the installed client marks as a door in imported
//! house-template JSON files.
//!
//! ```sh
//! cargo run -p openshard-housing --example template_door_audit -- \
//!   "$OPENSHARD_CLIENT" "$OPENSHARD_CLIENT/openshard-houses"
//! ```

use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

#[derive(Deserialize)]
struct Template {
    components: Vec<Component>,
}

#[derive(Deserialize)]
struct Component {
    graphic: u16,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let client = PathBuf::from(
        args.next()
            .ok_or("usage: template_door_audit CLIENT_DIR TEMPLATE_DIR")?,
    );
    let directory = PathBuf::from(
        args.next()
            .ok_or("usage: template_door_audit CLIENT_DIR TEMPLATE_DIR")?,
    );
    let tiles = openshard_uofiles::tiledata::load_tiles(client.join("tiledata.mul"))?;
    let mut paths = fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();
    for path in paths
        .into_iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
    {
        let template: Template = serde_json::from_str(&fs::read_to_string(&path)?)?;
        let mut doors = template
            .components
            .into_iter()
            .filter(|component| {
                tiles
                    .static_tile(component.graphic)
                    .flags
                    .has(openshard_tiles::TileFlags::DOOR)
            })
            .map(|component| component.graphic)
            .collect::<Vec<_>>();
        doors.sort_unstable();
        doors.dedup();
        if !doors.is_empty() {
            println!(
                "{}: {}",
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("<non-UTF-8>"),
                doors
                    .iter()
                    .map(|graphic| format!("{graphic:#06x}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }
    Ok(())
}
