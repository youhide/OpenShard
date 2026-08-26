//! The shard-owned catalogue of imported custom-house templates.
//!
//! The same JSON document feeds the map editor and the shard.  A local preview
//! alone cannot become a world object: the shard must hold the exact component
//! list it persists and sends to every watcher.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use openshard_protocol::wire::Graphic;
use openshard_uofiles::multi::Component;
use serde::Deserialize;

#[derive(Deserialize)]
struct TemplateFile {
    format: String,
    components: Vec<TemplateComponent>,
}

#[derive(Deserialize)]
struct TemplateComponent {
    graphic: u16,
    dx: i16,
    dy: i16,
    dz: i16,
    flags: u64,
}

/// Read every `openshard-house-design/v1` JSON file in `directory`.
///
/// The file stem is the command-safe template name, for example
/// `legacy-cathedral.json` becomes `@legacy-cathedral` in the map editor's
/// placement command.  A whole catalogue is refused on a malformed document:
/// a shard must not silently run a different catalogue from its editor.
pub fn load_directory(directory: &Path) -> Result<BTreeMap<String, Vec<Component>>, String> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(format!("could not read {}: {error}", directory.display())),
    };
    let mut paths = entries
        .map(|entry| entry.map(|entry| entry.path()).map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();
    let mut templates = BTreeMap::new();
    for path in paths.into_iter().filter(|path| {
        path.extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
    }) {
        let name = path
            .file_stem()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| format!("{} has no UTF-8 file name", path.display()))?
            .to_owned();
        if !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            return Err(format!("{name:?} is not a command-safe template name"));
        }
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("could not read {}: {error}", path.display()))?;
        let template: TemplateFile = serde_json::from_str(&source)
            .map_err(|error| format!("could not decode {}: {error}", path.display()))?;
        if template.format != "openshard-house-design/v1" {
            return Err(format!("{} has an unknown house-template format", path.display()));
        }
        if template.components.is_empty() {
            return Err(format!("{} has no components", path.display()));
        }
        let components = template
            .components
            .into_iter()
            .map(|component| {
                if i8::try_from(component.dx).is_err()
                    || i8::try_from(component.dy).is_err()
                    || i8::try_from(component.dz).is_err()
                {
                    return Err(format!(
                        "{} has a component the custom-house wire format cannot carry",
                        path.display()
                    ));
                }
                Ok(Component {
                    graphic: Graphic(component.graphic),
                    dx: component.dx,
                    dy: component.dy,
                    dz: component.dz,
                    flags: component.flags,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if !components.iter().any(|component| component.drawn()) {
            return Err(format!("{} has no drawn components", path.display()));
        }
        if templates.insert(name.clone(), components).is_some() {
            return Err(format!("duplicate house template name {name:?}"));
        }
    }
    Ok(templates)
}
