//! Report every classic-house stair tread that cannot be entered from a
//! neighbouring standing surface.
//!
//! ```sh
//! cargo run --release -p openshard-housing --example house_stair_audit -- \
//!   --client "/path/to/Ultima Online Classic"
//! ```
//!
//! To ask about the step a saved character is facing:
//!
//! ```sh
//! cargo run --release -p openshard-housing --example house_stair_audit -- \
//!   --client "/path/to/Ultima Online Classic" --base-set felucca.osbase \
//!   --saved-step openshard.db --character "Lord British"
//! ```
//!
//! The house picture lives in `multi.mul` (or `MultiCollection.uop`) and the
//! meaning of each component lives in `tiledata.mul`. A visual scan cannot
//! establish whether a stair is usable: `CLIMBABLE` is entered at its base and
//! stood on halfway up. This probe lays each standard house over flat ground
//! and asks the production step rule whether every tread has a legal entry from
//! at least one neighbouring surface.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

use openshard_map::grid::Tile;
use openshard_map::overlay::{Cover, Doors, Overlay};
use openshard_movement::{Bodies, Footing, MapTerrain, PLAYER_HEIGHT, Walker, can_stand, step_allowed};
use openshard_protocol::direction::{Direction, Facing};
use openshard_protocol::wire::Graphic;
use openshard_protocol::world::{Facet, Point, RawFastwalkKey, RawStepSequence, WalkRequest};
use openshard_tiles::TileData;
use openshard_uofiles::multi::{Component, Multi, Multis};

/// Every classic house this shard permits and equips with its declared doors.
/// Foundations are absent: their first design derives its own stair strip at
/// placement and has no fixed `multi.mul` stair to audit.
const CLASSIC_HOUSES: &[u16] = &[
    0x0064, 0x0066, 0x0068, 0x006A, 0x006C, 0x006E, 0x0074, 0x0076, 0x0078, 0x007A, 0x007C, 0x007E, 0x008C,
    0x0096, 0x0098, 0x009A, 0x009C, 0x009E, 0x00A0, 0x00A2,
];

/// One clear ground tile on every side lets a stair at the edge name its
/// neighbouring ground without giving a component a negative local coordinate.
const BORDER: i32 = 1;

#[derive(Clone, Copy)]
struct Stair {
    component: usize,
    graphic: u16,
    offset_x: i16,
    offset_y: i16,
    offset_z: i16,
    at: Point,
    stand_z: i32,
}

struct Inspect {
    multi: u16,
    origin: Point,
    target: Tile,
}

struct Cli {
    client: PathBuf,
    base_set: Option<PathBuf>,
    inspect: Option<Inspect>,
    saved: Option<Saved>,
}

struct Saved {
    database: PathBuf,
    character: String,
}

fn number(text: &str) -> Result<u16, String> {
    let text = text.trim();
    let hexadecimal = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X"));
    hexadecimal
        .map_or_else(|| text.parse(), |digits| u16::from_str_radix(digits, 16))
        .map_err(|error| format!("invalid number {text:?}: {error}"))
}

fn cli() -> Result<Cli, String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let client_at = args.iter().position(|arg| arg == "--client" || arg == "-c");
    let Some(client_at) = client_at else {
        return Err("missing --client /path/to/Ultima Online Classic".into());
    };
    let client = args
        .get(client_at + 1)
        .ok_or_else(|| "missing client path after --client".to_owned())?;
    let base_set = args
        .iter()
        .position(|arg| arg == "--base-set")
        .map(|at| {
            args.get(at + 1)
                .map(PathBuf::from)
                .ok_or_else(|| "missing base-set path after --base-set".to_owned())
        })
        .transpose()?;
    let inspect_at = args.iter().position(|arg| arg == "--inspect-house");
    let inspect = match inspect_at {
        None => None,
        Some(at) => {
            let numbers = args
                .get(at + 1..at + 6)
                .ok_or_else(|| "--inspect-house needs: MULTI ORIGIN_X ORIGIN_Y TARGET_X TARGET_Y".to_owned())?
                .iter()
                .map(|arg| number(arg))
                .collect::<Result<Vec<_>, _>>()?;
            Some(Inspect {
                multi: numbers[0],
                origin: Point::new(numbers[1], numbers[2], 0),
                target: Tile::new(numbers[3], numbers[4]),
            })
        }
    };
    let saved = args
        .iter()
        .position(|arg| arg == "--saved-step")
        .map(|at| {
            let database = args
                .get(at + 1)
                .ok_or_else(|| "missing database path after --saved-step".to_owned())?;
            let character_at = args
                .iter()
                .position(|arg| arg == "--character")
                .ok_or_else(|| "--saved-step also needs --character NAME".to_owned())?;
            let character = args
                .get(character_at + 1)
                .ok_or_else(|| "missing name after --character".to_owned())?;
            Ok::<_, String>(Saved {
                database: PathBuf::from(database),
                character: character.clone(),
            })
        })
        .transpose()?;
    if inspect.is_some() && saved.is_some() {
        return Err("choose either --inspect-house or --saved-step".into());
    }
    Ok(Cli {
        client: PathBuf::from(client),
        base_set,
        inspect,
        saved,
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = cli().map_err(std::io::Error::other)?;
    let tiles = openshard_uofiles::tiledata::load_tiles(cli.client.join("tiledata.mul"))?;
    let multis = Multis::load(&cli.client)?;
    if let Some(saved) = cli.saved {
        return inspect_saved_step(&cli.client, cli.base_set.as_deref(), &tiles, &multis, &saved);
    }
    if let Some(inspect) = cli.inspect {
        return inspect_house(&cli.client, cli.base_set.as_deref(), &tiles, &multis, inspect);
    }

    let mut checked = 0;
    let mut stairs = 0;
    let mut inaccessible = Vec::new();
    let mut inaccessible_tiles = Vec::new();
    for &id in CLASSIC_HOUSES {
        let Some(house) = multis.get(id) else {
            eprintln!("house {id:#06x}: not present in this client install");
            continue;
        };
        checked += 1;
        let report = audit(house, &tiles);
        stairs += report.len();
        let mut tiles_here = HashSet::new();
        let mut passable_here = HashSet::new();
        for (stair, walkable) in &report {
            let tile = Tile::new(stair.at.x, stair.at.y);
            tiles_here.insert(tile);
            if *walkable {
                passable_here.insert(tile);
            }
        }
        let inaccessible_here: HashSet<_> = tiles_here
            .into_iter()
            .filter(|tile| !passable_here.contains(tile))
            .collect();
        inaccessible_tiles.extend(inaccessible_here.iter().copied().map(|tile| (id, tile)));
        for (stair, walkable) in report {
            if !walkable && inaccessible_here.contains(&Tile::new(stair.at.x, stair.at.y)) {
                inaccessible.push((id, stair));
            }
        }
    }

    println!("classic houses checked: {checked}");
    println!("climbable stair components: {stairs}");
    inaccessible_tiles.sort_unstable_by_key(|(id, tile)| (*id, tile.x, tile.y));
    println!(
        "stair tiles with no legal incoming step: {}",
        inaccessible_tiles.len()
    );
    for (id, tile) in &inaccessible_tiles {
        println!("  house {id:#06x}, offset ({}, {})", tile.x, tile.y);
    }
    if inaccessible.is_empty() {
        println!("inaccessible components: 0");
    } else {
        println!("inaccessible components: {}", inaccessible.len());
        for (id, stair) in inaccessible {
            println!(
                "  house {id:#06x}, component #{}, graphic {:#06x}, multi offset ({}, {}, {}), standing z {}",
                stair.component, stair.graphic, stair.offset_x, stair.offset_y, stair.offset_z, stair.stand_z
            );
        }
    }
    Ok(())
}

/// Rebuild the saved character's immediate live footing without starting a
/// shard or a client. The database is opened read-only: this is an assertion
/// about a save, not another process capable of changing it.
fn inspect_saved_step(
    client: &std::path::Path,
    base_set: Option<&std::path::Path>,
    tiles: &TileData,
    multis: &Multis,
    saved: &Saved,
) -> Result<(), Box<dyn std::error::Error>> {
    use rusqlite::{Connection, OpenFlags, params};

    let database = Connection::open_with_flags(
        &saved.database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let (serial, name, facet, from, facing): (i64, String, u8, Point, u8) = database.query_row(
        "SELECT serial, name, facet, x, y, z, facing FROM characters WHERE name = ?1",
        [&saved.character],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                Point::new(row.get(3)?, row.get(4)?, row.get(5)?),
                row.get(6)?,
            ))
        },
    )?;
    let direction = Direction::from_bits(facing);
    let (dx, dy) = direction.step();
    let target = Tile::new(
        u16::try_from(i32::from(from.x) + dx)?,
        u16::try_from(i32::from(from.y) + dy)?,
    );

    let source = base_set.map_or(
        openshard_movement::bake::WorldSource::Install,
        openshard_movement::bake::WorldSource::BaseSet,
    );
    let world = openshard_movement::bake::FacetWorld::read(client, source, Facet(facet))?;
    let map = world.snapshot.map();
    let spans = openshard_movement::spans::SpanIndex::build(map, tiles);
    let terrain = MapTerrain::new(map, tiles, &spans);

    let mut covers: HashMap<Tile, Vec<Cover>> = HashMap::new();
    let mut relevant_components = Vec::new();
    let mut house_count = 0_usize;
    let mut house_rows = database.prepare("SELECT serial, multi, x, y, z FROM houses WHERE facet = ?1")?;
    let houses = house_rows.query_map([facet], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, u16>(1)?,
            Point::new(row.get(2)?, row.get(3)?, row.get(4)?),
        ))
    })?;
    for house in houses {
        let (house_serial, multi, origin) = house?;
        house_count += 1;
        let mut design_rows = database.prepare(
            "SELECT graphic, dx, dy, dz, flags FROM house_designs WHERE house = ?1 ORDER BY rowid",
        )?;
        let design = design_rows
            .query_map([house_serial], |row| {
                Ok(Component {
                    graphic: Graphic(row.get(0)?),
                    dx: row.get(1)?,
                    dy: row.get(2)?,
                    dz: row.get(3)?,
                    flags: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let components = match design.is_empty() {
            true => multis
                .get(multi)
                .ok_or_else(|| format!("saved house {house_serial} names missing multi {multi:#06x}"))?
                .components
                .as_slice(),
            false => design.as_slice(),
        };
        for (index, component) in components.iter().copied().enumerate().filter(|(_, c)| c.drawn()) {
            let at = component
                .placed_at(origin)
                .ok_or_else(|| format!("component #{index} of house {house_serial} falls off the map"))?;
            let laid = Cover::of_static(tiles.static_tile(component.graphic.0)).based_at(at.z);
            let tile = Tile::new(at.x, at.y);
            if tile == Tile::new(from.x, from.y) || tile == target {
                relevant_components.push((house_serial, multi, index, component, at, laid.clone()));
            }
            covers.entry(tile).or_default().extend(laid);
        }
    }

    // Ordinary ground items are registered from their tiledata at restore.
    let mut item_count = 0_usize;
    let mut item_rows =
        database.prepare("SELECT graphic, x, y, z FROM items WHERE loc_kind = 0 AND facet = ?1")?;
    for item in item_rows.query_map([facet], |row| {
        Ok((
            row.get::<_, u16>(0)?,
            Point::new(row.get(1)?, row.get(2)?, row.get(3)?),
        ))
    })? {
        let (graphic, at) = item?;
        item_count += 1;
        covers
            .entry(Tile::new(at.x, at.y))
            .or_default()
            .extend(Cover::of_static(tiles.static_tile(graphic)).based_at(at.z));
    }

    // Decorations include doors. An open saved door contributes no cover; a
    // shut one contributes the same door span restore_decorations registers.
    let mut decoration_count = 0_usize;
    let mut decoration_rows = database.prepare(
        "SELECT CAST(json_extract(data, '$.graphic') AS INTEGER), \
                CAST(json_extract(data, '$.x') AS INTEGER), \
                CAST(json_extract(data, '$.y') AS INTEGER), \
                CAST(json_extract(data, '$.z') AS INTEGER), \
                json_type(data, '$.door'), \
                COALESCE(CAST(json_extract(data, '$.door.is_open') AS INTEGER), 0) \
         FROM decorations \
         WHERE CAST(json_extract(data, '$.facet') AS INTEGER) = ?1",
    )?;
    for decoration in decoration_rows.query_map([facet], |row| {
        Ok((
            row.get::<_, u16>(0)?,
            Point::new(row.get(1)?, row.get(2)?, row.get(3)?),
            row.get::<_, Option<String>>(4)?.is_some(),
            row.get::<_, i64>(5)? != 0,
        ))
    })? {
        let (graphic, at, door, open) = decoration?;
        decoration_count += 1;
        let laid = if door {
            (!open)
                .then(|| vec![Cover::door(at.z, openshard_state::DOOR_HEIGHT)])
                .unwrap_or_default()
        } else {
            Cover::of_static(tiles.static_tile(graphic))
                .based_at(at.z)
                .into_iter()
                .collect()
        };
        covers.entry(Tile::new(at.x, at.y)).or_default().extend(laid);
    }

    let mut overlay = Overlay::default();
    for (tile, laid) in covers {
        overlay.set(tile, laid);
    }

    // The save has no separate movement index: on boot it is rebuilt from the
    // saved character and mobile positions. Only living bodies block.
    let mut crowd = Vec::new();
    let mut character_rows =
        database.prepare("SELECT x, y, z FROM characters WHERE facet = ?1 AND serial != ?2 AND dead = 0")?;
    crowd.extend(
        character_rows
            .query_map(params![facet, serial], |row| {
                Ok(Point::new(row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<Result<Vec<_>, _>>()?,
    );
    let mut mobile_rows = database.prepare(
        "SELECT CAST(json_extract(data, '$.x') AS INTEGER), \
                CAST(json_extract(data, '$.y') AS INTEGER), \
                CAST(json_extract(data, '$.z') AS INTEGER) \
         FROM mobiles \
         WHERE CAST(json_extract(data, '$.facet') AS INTEGER) = ?1 \
           AND CAST(json_extract(data, '$.hits_current') AS INTEGER) > 0",
    )?;
    crowd.extend(
        mobile_rows
            .query_map([facet], |row| {
                Ok(Point::new(row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<Result<Vec<_>, _>>()?,
    );
    crowd.sort_unstable_by_key(|body| (body.x, body.y));

    let footing = Footing::new(Some(terrain), &overlay, Doors::AsTheyStand);
    let live = footing.among(Bodies::standing(&crowd));
    let bare_answer = step_allowed(&footing, from, direction);
    let live_answer = step_allowed(&live, from, direction);
    let request_facing = if facing & openshard_protocol::direction::RUNNING_BIT != 0 {
        Facing::running(direction)
    } else {
        Facing::walking(direction)
    };
    let mut walker = Walker::new(from, request_facing);
    let request_answer = walker.request(
        WalkRequest {
            facing: request_facing,
            sequence: RawStepSequence(0),
            fastwalk_key: RawFastwalkKey(0),
        },
        &live,
        Instant::now(),
        false,
    );
    println!("save: {}", saved.database.display());
    println!(
        "character {name:?} serial {serial}, facet {facet}, from {from:?}, facing {direction:?}{}",
        if facing & openshard_protocol::direction::RUNNING_BIT != 0 {
            " (running)"
        } else {
            ""
        }
    );
    println!(
        "world revision {}, patches {}; restored {} house(s), {} ground item(s), {} decoration(s), {} other living body/bodies",
        world.snapshot.revision().get(),
        world.patches,
        house_count,
        item_count,
        decoration_count,
        crowd.len(),
    );
    println!("target: ({}, {})", target.x, target.y);
    println!("house components on source or target:");
    for (house, multi, index, component, at, laid) in relevant_components {
        println!(
            "  house {house}, multi {multi:#06x}, #{index} graphic {:#06x} at {at:?}: {laid:?}",
            component.graphic.0,
        );
    }
    println!("saved footing without bodies: {bare_answer:?}");
    println!("saved footing with bodies:    {live_answer:?}");
    println!("fresh full Walker::request:   {request_answer:?}");
    Ok(())
}

/// Reproduce one real house position over its actual Felucca terrain.
///
/// ```text
/// house_stair_audit --client CLIENT --base-set felucca.osbase \
///     --inspect-house 0x0076 1341 1893 1343 1898
/// ```
fn inspect_house(
    client: &std::path::Path,
    base_set: Option<&std::path::Path>,
    tiles: &TileData,
    multis: &Multis,
    inspect: Inspect,
) -> Result<(), Box<dyn std::error::Error>> {
    let house = multis
        .get(inspect.multi)
        .ok_or_else(|| format!("multi {:#06x} is absent from this client", inspect.multi))?;
    let source = base_set.map_or(
        openshard_movement::bake::WorldSource::Install,
        openshard_movement::bake::WorldSource::BaseSet,
    );
    let world =
        openshard_movement::bake::FacetWorld::read(client, source, openshard_protocol::world::Facet(0))?;
    let map = world.snapshot.map();
    let spans = openshard_movement::spans::SpanIndex::build(map, tiles);
    let terrain = MapTerrain::new(map, tiles, &spans);
    let mut overlay = Overlay::default();
    let mut components = Vec::new();
    for (index, component) in house
        .components
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, part)| part.drawn())
    {
        let at = component
            .placed_at(inspect.origin)
            .expect("a placed house component fits on the map");
        let cover = Cover::of_static(tiles.static_tile(component.graphic.0)).based_at(at.z);
        if Tile::new(at.x, at.y) == inspect.target {
            components.push((index, component, at, cover.clone()));
        }
        let tile = Tile::new(at.x, at.y);
        let mut covers = overlay.at(tile).to_vec();
        covers.extend(cover);
        overlay.set(tile, covers);
    }
    let footing = Footing::new(Some(terrain), &overlay, Doors::AsTheyStand);
    println!(
        "house {:#06x} at ({}, {}), target ({}, {}), map revision {}, patches {}",
        inspect.multi,
        inspect.origin.x,
        inspect.origin.y,
        inspect.target.x,
        inspect.target.y,
        world.snapshot.revision().get(),
        world.patches,
    );
    println!("components on target:");
    for (index, component, at, covers) in components {
        println!(
            "  #{index}: graphic {:#06x}, offset ({}, {}, {}), base z {}, covers {covers:?}",
            component.graphic.0, component.dx, component.dy, component.dz, at.z
        );
    }
    println!("incoming steps:");
    for direction in Direction::ALL {
        let (dx, dy) = direction.step();
        let x = i32::from(inspect.target.x) - dx;
        let y = i32::from(inspect.target.y) - dy;
        let Ok(x) = u16::try_from(x) else { continue };
        let Ok(y) = u16::try_from(y) else { continue };
        let source = Tile::new(x, y);
        let accepted = (i8::MIN..=i8::MAX)
            .filter_map(|z| {
                can_stand(&footing, source, i32::from(z), PLAYER_HEIGHT)
                    .then(|| (z, step_allowed(&footing, Point::new(x, y, z), direction)))
            })
            .collect::<Vec<_>>();
        println!("  {direction:?} from ({x}, {y}): {accepted:?}");
    }
    Ok(())
}

/// Return every climbable component and whether it has a legal incoming step.
fn audit(house: &Multi, tiles: &TileData) -> Vec<(Stair, bool)> {
    let drawn: Vec<(usize, Component)> = house
        .components
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, component)| component.drawn())
        .collect();
    let min_x = drawn
        .iter()
        .map(|(_, c)| i32::from(c.dx))
        .min()
        .unwrap_or_default();
    let max_x = drawn
        .iter()
        .map(|(_, c)| i32::from(c.dx))
        .max()
        .unwrap_or_default();
    let min_y = drawn
        .iter()
        .map(|(_, c)| i32::from(c.dy))
        .min()
        .unwrap_or_default();
    let max_y = drawn
        .iter()
        .map(|(_, c)| i32::from(c.dy))
        .max()
        .unwrap_or_default();
    let origin = Point::new((BORDER - min_x) as u16, (BORDER - min_y) as u16, 0);
    let width = max_x - min_x + 1 + BORDER * 2;
    let height = max_y - min_y + 1 + BORDER * 2;

    let mut covers: HashMap<Tile, Vec<Cover>> = HashMap::new();
    // The flat world beneath the placed house. A zero-height platform is a
    // standing surface at z=0 and no blocking body, exactly like a ground
    // floor laid over land.
    for y in 0..height as u16 {
        for x in 0..width as u16 {
            covers
                .entry(Tile::new(x, y))
                .or_default()
                .push(Cover::standing(0, 0));
        }
    }

    let mut stairs = Vec::new();
    for (component, part) in drawn {
        let at = part
            .placed_at(origin)
            .expect("the local audit origin fits every component");
        let laid = Cover::of_static(tiles.static_tile(part.graphic.0)).based_at(at.z);
        if let Some(stand) = laid.stands() {
            if tiles.static_tile(part.graphic.0).flags.is_climbable() {
                stairs.push(Stair {
                    component,
                    graphic: part.graphic.0,
                    offset_x: part.dx,
                    offset_y: part.dy,
                    offset_z: part.dz,
                    at,
                    stand_z: stand.surface(),
                });
            }
        }
        covers.entry(Tile::new(at.x, at.y)).or_default().extend(laid);
    }
    let mut overlay = Overlay::default();
    for (tile, covers) in covers {
        overlay.set(tile, covers);
    }

    let footing = Footing::new(None, &overlay, Doors::AllOpen);
    stairs
        .into_iter()
        .map(|stair| {
            let target = Point::new(stair.at.x, stair.at.y, stair.stand_z as i8);
            let walkable = Direction::ALL.into_iter().any(|direction| {
                let (dx, dy) = direction.step();
                let x = i32::from(stair.at.x) - dx;
                let y = i32::from(stair.at.y) - dy;
                if !(0..width).contains(&x) || !(0..height).contains(&y) {
                    return false;
                }
                let tile = Tile::new(x as u16, y as u16);
                overlay.surfaces_at(tile).any(|surface| {
                    let z = surface.surface();
                    let Ok(z) = i8::try_from(z) else {
                        return false;
                    };
                    can_stand(&footing, tile, i32::from(z), PLAYER_HEIGHT)
                        && step_allowed(&footing, Point::new(tile.x, tile.y, z), direction) == Some(target)
                })
            });
            (stair, walkable)
        })
        .collect()
}
