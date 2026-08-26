//! Import the world-item part of a legacy Sphere `.wsc` save as house-design
//! components.
//!
//! A `.wsc` records individual items at absolute world coordinates.  It is not
//! a multi, and it does not say which foundation should own the result.  The
//! caller therefore supplies the intended house origin; [`design_at`] subtracts
//! it and returns the local coordinates a [`HouseDesign`][openshard_state::components::HouseDesign]
//! carries.
//!
//! This module deliberately reads only `SECTION WORLDITEM`: serials, names,
//! containers, item types and hues are not part of a `HouseDesign`.  The
//! importer does not execute SphereScript and cannot write a shard save.

use std::fmt;

use openshard_protocol::wire::Graphic;
use openshard_uofiles::multi::Component;

/// A coordinate in the legacy world-save format.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Origin {
    /// East-west world coordinate.
    pub x: i32,
    /// North-south world coordinate.
    pub y: i32,
    /// Height in UO z units.
    pub z: i32,
}

/// One static item read from `SECTION WORLDITEM`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WorldItem {
    /// The static art id.
    pub graphic: u16,
    /// Its absolute position in the source world.
    pub at: Origin,
}

/// A malformed or unsupported `.wsc` record.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Error {
    line: usize,
    message: String,
}

impl Error {
    fn new(line: usize, message: impl Into<String>) -> Self {
        Self {
            line,
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for Error {}

#[derive(Default)]
struct ItemFields {
    started: usize,
    graphic: Option<u16>,
    x: Option<i32>,
    y: Option<i32>,
    z: Option<i32>,
}

impl ItemFields {
    fn finish(self, line: usize) -> Result<WorldItem, Error> {
        let graphic = self
            .graphic
            .ok_or_else(|| Error::new(self.started, "WORLDITEM has no ID"))?;
        let x = self
            .x
            .ok_or_else(|| Error::new(self.started, "WORLDITEM has no X"))?;
        let y = self
            .y
            .ok_or_else(|| Error::new(self.started, "WORLDITEM has no Y"))?;
        let z = self
            .z
            .ok_or_else(|| Error::new(self.started, "WORLDITEM has no Z"))?;
        if line == self.started {
            return Err(Error::new(line, "WORLDITEM is missing its body"));
        }
        Ok(WorldItem {
            graphic,
            at: Origin { x, y, z },
        })
    }
}

fn number(text: &str, line: usize) -> Result<i32, Error> {
    let text = text.trim();
    let digits = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X"));
    digits
        .map_or_else(|| text.parse::<i32>(), |digits| i32::from_str_radix(digits, 16))
        .map_err(|_| Error::new(line, format!("{text:?} is not an integer")))
}

fn component(graphic: u16, dx: i32, dy: i32, dz: i32, line: usize) -> Result<Component, Error> {
    Ok(Component {
        graphic: Graphic(graphic),
        dx: i16::from(i8::try_from(dx).map_err(|_| {
            Error::new(
                line,
                format!("item {graphic:#06x} is too far east/west of the origin"),
            )
        })?),
        dy: i16::from(i8::try_from(dy).map_err(|_| {
            Error::new(
                line,
                format!("item {graphic:#06x} is too far north/south of the origin"),
            )
        })?),
        dz: i16::from(i8::try_from(dz).map_err(|_| {
            Error::new(
                line,
                format!("item {graphic:#06x} is too far above/below the origin"),
            )
        })?),
        flags: 1,
    })
}

fn section_is_world_item(line: &str) -> bool {
    let mut words = line.split_whitespace();
    matches!(words.next(), Some(section) if section.eq_ignore_ascii_case("SECTION"))
        && matches!(words.next(), Some(kind) if kind.eq_ignore_ascii_case("WORLDITEM"))
}

/// Read the statics in a legacy Sphere `.wsc` file.
///
/// Any non-`WORLDITEM` section is deliberately ignored.  This lets one extract
/// a house from a broader world-save while refusing incomplete item records;
/// silently dropping one wall would make an imported building unsafe to trust.
pub fn world_items(source: &str) -> Result<Vec<WorldItem>, Error> {
    let mut items = Vec::new();
    let mut active: Option<ItemFields> = None;
    let mut opened = false;

    for (offset, raw_line) in source.lines().enumerate() {
        let line_number = offset + 1;
        let line = raw_line.trim();
        if section_is_world_item(line) {
            if active.is_some() {
                return Err(Error::new(
                    line_number,
                    "a WORLDITEM started before the previous one closed",
                ));
            }
            active = Some(ItemFields {
                started: line_number,
                ..ItemFields::default()
            });
            opened = false;
            continue;
        }
        let Some(fields) = active.as_mut() else { continue };
        if line.is_empty() {
            continue;
        }
        if !opened {
            if line == "{" {
                opened = true;
                continue;
            }
            return Err(Error::new(line_number, "WORLDITEM must start with `{`"));
        }
        if line == "}" {
            let complete = active
                .take()
                .expect("active item was checked above")
                .finish(line_number)?;
            items.push(complete);
            opened = false;
            continue;
        }
        let mut words = line.split_whitespace();
        let Some(key) = words.next() else { continue };
        let Some(value) = words.next() else {
            return Err(Error::new(line_number, format!("{key} has no value")));
        };
        match key.to_ascii_uppercase().as_str() {
            "ID" => {
                let value = number(value, line_number)?;
                fields.graphic =
                    Some(u16::try_from(value).map_err(|_| Error::new(line_number, "ID is outside u16"))?);
            }
            "X" => fields.x = Some(number(value, line_number)?),
            "Y" => fields.y = Some(number(value, line_number)?),
            "Z" => fields.z = Some(number(value, line_number)?),
            _ => {}
        }
    }

    if let Some(fields) = active {
        return Err(Error::new(fields.started, "WORLDITEM does not close with `}`"));
    }
    Ok(items)
}

/// Convert legacy absolute world items to one OpenShard house design.
///
/// Each source item becomes one drawn [`Component`].  `1` is the established
/// `multi.mul` flag for a component the client draws.  The custom-house wire
/// format carries each offset as an `i8`, so a larger relative coordinate is
/// rejected here rather than silently disappearing when the design is sent.
pub fn design_at(source: &str, origin: Origin) -> Result<Vec<Component>, Error> {
    world_items(source)?
        .into_iter()
        .map(|item| {
            component(
                item.graphic,
                item.at.x - origin.x,
                item.at.y - origin.y,
                item.at.z - origin.z,
                0,
            )
        })
        .collect()
}

/// Read a MultiScripter component export.
///
/// These files declare a component count on their fourth header line, followed
/// by `graphic dx dy dz flags` rows.  Their last flag is not `multi.mul`'s
/// drawn bit, so every imported component uses the one OpenShard requires.
pub fn multiscripter_design(source: &str) -> Result<Vec<Component>, Error> {
    let lines = source.lines().collect::<Vec<_>>();
    let Some((header, count_line)) = lines
        .iter()
        .enumerate()
        .find(|(_, line)| line.trim_end().ends_with("num components"))
    else {
        return Err(Error::new(0, "not a MultiScripter component export"));
    };
    let count = count_line
        .split_whitespace()
        .next()
        .ok_or_else(|| Error::new(header + 1, "component count is missing"))
        .and_then(|count| number(count, header + 1))?;
    let expected =
        usize::try_from(count).map_err(|_| Error::new(header + 1, "component count is negative"))?;
    let components = lines
        .into_iter()
        .enumerate()
        .skip(header + 1)
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(offset, line)| {
            let line_number = offset + 1;
            let values = line
                .split_whitespace()
                .take(4)
                .map(|value| number(value, line_number))
                .collect::<Result<Vec<_>, _>>()?;
            let [graphic, dx, dy, dz] = values.as_slice() else {
                return Err(Error::new(line_number, "component needs graphic dx dy dz"));
            };
            component(
                u16::try_from(*graphic).map_err(|_| Error::new(line_number, "graphic is outside u16"))?,
                *dx,
                *dy,
                *dz,
                line_number,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    if components.len() != expected {
        return Err(Error::new(
            header + 1,
            format!("declares {expected} components but contains {}", components.len()),
        ));
    }
    Ok(components)
}

/// Read numeric `COMPONENT=graphic,x,y[,z]` rows from a Sphere multi definition.
///
/// Older packs commonly omit the final height value; Sphere treats that form
/// as `z = 0`, so the importer does too.
pub fn sphere_component_design(source: &str) -> Result<Vec<Component>, Error> {
    let components = source
        .lines()
        .enumerate()
        .filter_map(|(offset, line)| {
            let line_number = offset + 1;
            let line = line.split_once("//").map_or(line, |(before, _)| before).trim();
            let (_, values) = line
                .split_once('=')
                .filter(|(key, _)| key.trim().eq_ignore_ascii_case("COMPONENT"))?;
            Some((line_number, values))
        })
        .map(|(line, values)| {
            let values = values.split(',').map(str::trim).collect::<Vec<_>>();
            let (graphic, dx, dy, dz) = match values.as_slice() {
                [graphic, dx, dy] => (*graphic, *dx, *dy, 0),
                [graphic, dx, dy, dz] => (*graphic, *dx, *dy, number(dz, line)?),
                _ => return Err(Error::new(line, "COMPONENT needs graphic,x,y[,z]")),
            };
            let graphic = u16::from_str_radix(graphic.trim_start_matches("0x").trim_start_matches("0X"), 16)
                .map_err(|_| Error::new(line, "COMPONENT graphic is not hexadecimal"))?;
            component(graphic, number(dx, line)?, number(dy, line)?, dz, line)
        })
        .collect::<Result<Vec<_>, _>>()?;
    if components.is_empty() {
        return Err(Error::new(0, "no numeric COMPONENT rows"));
    }
    Ok(components)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_items_ignore_unrelated_sections_and_keep_each_static() {
        let source = r#"
SECTION CHAR 0
{
NAME ignored
}

SECTION WORLDITEM 4
{
ID 0x05A2
X 5458
Y 1179
Z 20
TYPE 140
}
SECTION WORLDITEM 5
{
ID 1442
X 5457
Y 1179
Z 20
}
"#;
        assert_eq!(
            world_items(source).expect("two complete world items"),
            vec![
                WorldItem {
                    graphic: 0x05A2,
                    at: Origin {
                        x: 5458,
                        y: 1179,
                        z: 20
                    }
                },
                WorldItem {
                    graphic: 1442,
                    at: Origin {
                        x: 5457,
                        y: 1179,
                        z: 20
                    }
                },
            ]
        );
    }

    #[test]
    fn design_is_relative_to_the_explicit_origin() {
        let source = r#"
SECTION WORLDITEM 0
{
ID 1442
X 5458
Y 1179
Z 20
}
"#;
        assert_eq!(
            design_at(
                source,
                Origin {
                    x: 5455,
                    y: 1178,
                    z: 0
                }
            )
            .expect("one component"),
            vec![Component {
                graphic: Graphic(1442),
                dx: 3,
                dy: 1,
                dz: 20,
                flags: 1
            }]
        );
    }

    #[test]
    fn incomplete_world_item_is_refused() {
        let source = "SECTION WORLDITEM 0\n{\nID 1442\nX 5458\nY 1179\n}";
        assert_eq!(
            world_items(source)
                .expect_err("missing z must not be dropped")
                .to_string(),
            "line 1: WORLDITEM has no Z"
        );
    }

    #[test]
    fn a_component_the_design_packet_cannot_carry_is_refused() {
        let source = "SECTION WORLDITEM 0\n{\nID 1442\nX 128\nY 0\nZ 0\n}";
        assert!(
            design_at(source, Origin { x: 0, y: 0, z: 0 }).is_err(),
            "the runtime must not drop an offset the importer accepted"
        );
    }

    #[test]
    fn multiscripter_rows_become_drawn_components() {
        let source =
            "6 version\n0 template id\n-1 item version\n2 num components\n3025 11 -7 3 0\n2973 11 -7 3 0\n";
        assert_eq!(
            multiscripter_design(source).expect("the two declared rows"),
            vec![
                Component {
                    graphic: Graphic(3025),
                    dx: 11,
                    dy: -7,
                    dz: 3,
                    flags: 1
                },
                Component {
                    graphic: Graphic(2973),
                    dx: 11,
                    dy: -7,
                    dz: 3,
                    flags: 1
                },
            ]
        );
    }

    #[test]
    fn sphere_component_rows_are_hex_graphics_with_local_offsets() {
        let source = "[ITEMDEF i_multi_new]\nCOMPONENT=006A5,4,7,7 // sign\nCOMPONENT=00BD2,6,8,5\n";
        assert_eq!(
            sphere_component_design(source).expect("two component rows"),
            vec![
                Component {
                    graphic: Graphic(0x06A5),
                    dx: 4,
                    dy: 7,
                    dz: 7,
                    flags: 1
                },
                Component {
                    graphic: Graphic(0x0BD2),
                    dx: 6,
                    dy: 8,
                    dz: 5,
                    flags: 1
                },
            ]
        );
    }

    #[test]
    fn sphere_component_rows_default_an_omitted_height_to_zero() {
        let source = "COMPONENT=049c,-4,-4\n";
        assert_eq!(
            sphere_component_design(source).expect("one component at ground level"),
            vec![Component {
                graphic: Graphic(0x049c),
                dx: -4,
                dy: -4,
                dz: 0,
                flags: 1
            }]
        );
    }
}
