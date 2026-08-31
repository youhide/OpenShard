//! Convert a legacy Sphere `.wsc` house layout into an OpenShard house-design
//! JSON document.
//!
//! The source records absolute world positions; the explicit origin becomes
//! `(0, 0, 0)` in the generated design.  Review the result before attaching it
//! to a foundation or placing it in a shard save.
//!
//! ```sh
//! cargo run -p openshard-housing --example wsc_to_design -- \
//!   Marble-Bungalow.wsc --origin 5455 1178 0 --output marble-bungalow.json
//! ```

use std::fs::{
    File,
    read_to_string,
};
use std::io::Write;
use std::path::PathBuf;

use openshard_housing::wsc::{
    Origin,
    design_at,
};
use serde_json::json;

struct Cli {
    source: PathBuf,
    origin: Origin,
    output: PathBuf,
}

fn integer(text: &str) -> Result<i32, String> {
    let hexadecimal = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X"));
    hexadecimal
        .map_or_else(|| text.parse(), |digits| i32::from_str_radix(digits, 16))
        .map_err(|error| error.to_string())
}

fn cli() -> Result<Cli, String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let Some(source) = args.first() else {
        return Err("usage: wsc_to_design SOURCE.wsc --origin X Y Z --output DESIGN.json".into());
    };
    let origin_at = args
        .iter()
        .position(|arg| arg == "--origin")
        .ok_or("missing --origin X Y Z")?;
    let values = args
        .get(origin_at + 1..origin_at + 4)
        .ok_or("--origin needs X Y Z")?;
    let origin = Origin {
        x: integer(&values[0]).map_err(|error| format!("invalid origin X: {error}"))?,
        y: integer(&values[1]).map_err(|error| format!("invalid origin Y: {error}"))?,
        z: integer(&values[2]).map_err(|error| format!("invalid origin Z: {error}"))?,
    };
    let output_at = args
        .iter()
        .position(|arg| arg == "--output")
        .ok_or("missing --output DESIGN.json")?;
    let output = args.get(output_at + 1).ok_or("missing path after --output")?;
    Ok(Cli {
        source: PathBuf::from(source),
        origin,
        output: PathBuf::from(output),
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = cli().map_err(std::io::Error::other)?;
    let source = read_to_string(&cli.source)?;
    let components = design_at(&source, cli.origin)?;
    let document = json!({
        "format": "openshard-house-design/v1",
        "revision": 1,
        "origin": { "x": cli.origin.x, "y": cli.origin.y, "z": cli.origin.z },
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
