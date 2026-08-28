//! Convert a legacy ready-made-house export to an OpenShard house-template
//! document.
//!
//! Unlike `wsc_to_design`, this recognises the three content-only formats found
//! in old Sphere house packs: world saves (`.wsc`), MultiScripter component
//! exports, and numeric Sphere `COMPONENT=` definitions.  A `.wsc` has no local
//! origin, so its lowest x/y/z becomes `(0, 0, 0)` automatically.
//!
//! ```sh
//! cargo run -p openshard-housing --example legacy_house_to_design -- \
//!   Cathedral.wsc --output "$OPENSHARD_CLIENT/openshard-houses/cathedral.json"
//! ```

use std::fs::{File, read_to_string};
use std::io::Write;
use std::path::PathBuf;

use openshard_housing::wsc::{Origin, design_at, multiscripter_design, sphere_component_design, world_items};
use serde_json::json;

struct Cli {
    source: PathBuf,
    output: PathBuf,
    itemdef: Option<String>,
}

fn cli() -> Result<Cli, String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let Some(source) = args.first() else {
        return Err("usage: legacy_house_to_design SOURCE --output DESIGN.json".into());
    };
    let output_at = args
        .iter()
        .position(|arg| arg == "--output")
        .ok_or("missing --output DESIGN.json")?;
    let output = args.get(output_at + 1).ok_or("missing path after --output")?;
    let itemdef = args
        .iter()
        .position(|arg| arg == "--itemdef")
        .map(|itemdef_at| {
            args.get(itemdef_at + 1)
                .cloned()
                .ok_or("missing name after --itemdef")
        })
        .transpose()?;
    Ok(Cli {
        source: PathBuf::from(source),
        output: PathBuf::from(output),
        itemdef,
    })
}

fn itemdef_source<'a>(source: &'a str, requested: &str) -> Result<&'a str, String> {
    let mut starts = source.match_indices("[ITEMDEF").filter_map(|(start, _)| {
        let header = source[start..].lines().next()?.trim();
        let name = header.strip_prefix("[ITEMDEF")?.trim().strip_suffix(']')?.trim();
        name.eq_ignore_ascii_case(requested).then_some(start)
    });
    let start = starts
        .next()
        .ok_or_else(|| format!("no [ITEMDEF {requested}] section"))?;
    let after_header = &source[start + 1..];
    let end = after_header
        .match_indices("[ITEMDEF")
        .next()
        .map_or(source.len(), |(offset, _)| start + 1 + offset);
    Ok(&source[start..end])
}

fn wsc_design(
    source: &str,
) -> Result<(Origin, Vec<openshard_uofiles::multi::Component>), Box<dyn std::error::Error>> {
    let items = world_items(source)?;
    let origin = items
        .iter()
        .fold(None, |lowest: Option<Origin>, item| match lowest {
            None => Some(item.at),
            Some(lowest) => Some(Origin {
                x: lowest.x.min(item.at.x),
                y: lowest.y.min(item.at.y),
                z: lowest.z.min(item.at.z),
            }),
        });
    let origin = origin.ok_or("the .wsc has no WORLDITEM sections")?;
    Ok((origin, design_at(source, origin)?))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = cli().map_err(std::io::Error::other)?;
    let source = read_to_string(&cli.source)?;
    let source = cli
        .itemdef
        .as_deref()
        .map_or(Ok(source.as_str()), |itemdef| itemdef_source(&source, itemdef))?;
    let extension = cli
        .source
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    let (origin, components) = if extension.eq_ignore_ascii_case("wsc") {
        wsc_design(source)?
    } else if source
        .lines()
        .any(|line| line.trim_end().ends_with("num components"))
    {
        (Origin { x: 0, y: 0, z: 0 }, multiscripter_design(source)?)
    } else {
        (Origin { x: 0, y: 0, z: 0 }, sphere_component_design(source)?)
    };
    let document = json!({
        "format": "openshard-house-design/v1",
        "revision": 1,
        "origin": { "x": origin.x, "y": origin.y, "z": origin.z },
        "components": components.iter().map(|component| json!({
            "graphic": component.graphic.0,
            "dx": component.dx,
            "dy": component.dy,
            "dz": component.dz,
            "flags": component.flags,
        })).collect::<Vec<_>>(),
    });
    let mut output = File::options().write(true).create_new(true).open(&cli.output)?;
    serde_json::to_writer_pretty(&mut output, &document)?;
    output.write_all(b"\n")?;
    println!(
        "wrote {} components to {}",
        components.len(),
        cli.output.display()
    );
    Ok(())
}
