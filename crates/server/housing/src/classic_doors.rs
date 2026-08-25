//! The fixtures a classic house's multi does not carry.
//!
//! `multi.mul` is the picture of a house, not its complete contents. ServUO's
//! classic house classes place their doors separately, with one table entry per
//! house type. Keeping that table here makes a house placed from an otherwise
//! complete client multi usable immediately, without requiring a decoration
//! pass to have guessed that somebody would later build on this plot.

use openshard_entities::EntityId;
use openshard_protocol::serial::SerialKind;
use openshard_protocol::wire::{Graphic, Hue, MultiId};
use openshard_protocol::world::{Facet, Point};
use openshard_state::components::{Decoration, Door, Drawn, HouseDoor, Position};
use openshard_state::{ItemLocation, WorldState, WorldTick, establish_item_location};

#[derive(Clone, Copy)]
enum Material {
    Wood,
    Metal,
}

/// ServUO's `DoorFacing` index. The number selects both the two art ids and the
/// hinge displacement, so keeping it as one field prevents a plausible-looking
/// graphic from swinging in a different direction.
#[derive(Clone, Copy)]
struct Facing(u16);

impl Facing {
    const WEST_CW: Self = Self(0);
    const EAST_CCW: Self = Self(1);
    const WEST_CCW: Self = Self(2);
    const EAST_CW: Self = Self(3);
    const SOUTH_CW: Self = Self(4);
}

#[derive(Clone, Copy)]
struct DoorSpec {
    dx: i16,
    dy: i16,
    dz: i16,
    material: Material,
    facing: Facing,
    /// The other row of a double door. Both stable serials are assigned before
    /// either component receives this relationship.
    link: Option<usize>,
}

const fn door(dx: i16, dy: i16, dz: i16, material: Material, facing: Facing) -> DoorSpec {
    DoorSpec {
        dx,
        dy,
        dz,
        material,
        facing,
        link: None,
    }
}

const fn paired(mut spec: DoorSpec, link: usize) -> DoorSpec {
    spec.link = Some(link);
    spec
}

const SMALL_OLD: [DoorSpec; 1] = [door(0, 3, 7, Material::Wood, Facing::WEST_CW)];
const GUILD: [DoorSpec; 4] = [
    paired(door(-1, 6, 7, Material::Wood, Facing::WEST_CW), 1),
    paired(door(0, 6, 7, Material::Wood, Facing::EAST_CCW), 0),
    door(-3, -1, 7, Material::Wood, Facing::WEST_CW),
    door(3, -1, 7, Material::Wood, Facing::WEST_CW),
];
const TWO_STOREY_WOOD: [DoorSpec; 4] = [
    paired(door(-3, 6, 7, Material::Wood, Facing::WEST_CW), 1),
    paired(door(-2, 6, 7, Material::Wood, Facing::EAST_CCW), 0),
    door(-3, 0, 7, Material::Wood, Facing::WEST_CW),
    door(-2, 0, 27, Material::Wood, Facing::WEST_CW),
];
const TWO_STOREY_STONE: [DoorSpec; 4] = [
    paired(door(-3, 6, 7, Material::Wood, Facing::WEST_CW), 1),
    paired(door(-2, 6, 7, Material::Wood, Facing::EAST_CCW), 0),
    door(-3, 0, 7, Material::Wood, Facing::WEST_CW),
    door(-3, 0, 27, Material::Wood, Facing::WEST_CW),
];
const TOWER: [DoorSpec; 5] = [
    paired(door(0, 6, 6, Material::Metal, Facing::WEST_CW), 1),
    paired(door(1, 6, 6, Material::Metal, Facing::EAST_CCW), 0),
    door(3, -2, 6, Material::Metal, Facing::WEST_CW),
    door(1, 4, 26, Material::Metal, Facing::SOUTH_CW),
    door(1, 4, 46, Material::Metal, Facing::SOUTH_CW),
];
const KEEP: [DoorSpec; 2] = [
    paired(door(0, 10, 6, Material::Metal, Facing::WEST_CW), 1),
    paired(door(1, 10, 6, Material::Metal, Facing::EAST_CCW), 0),
];
const CASTLE: [DoorSpec; 8] = [
    paired(door(0, 15, 6, Material::Metal, Facing::WEST_CW), 1),
    paired(door(1, 15, 6, Material::Metal, Facing::EAST_CCW), 0),
    paired(door(0, 11, 6, Material::Metal, Facing::WEST_CCW), 3),
    paired(door(1, 11, 6, Material::Metal, Facing::EAST_CW), 2),
    paired(door(0, 5, 6, Material::Metal, Facing::WEST_CW), 5),
    paired(door(1, 5, 6, Material::Metal, Facing::EAST_CCW), 4),
    paired(door(-1, -11, 6, Material::Metal, Facing::WEST_CW), 7),
    paired(door(0, -11, 6, Material::Metal, Facing::EAST_CCW), 6),
];
const LARGE_PATIO: [DoorSpec; 5] = [
    paired(door(-4, 6, 7, Material::Wood, Facing::WEST_CW), 1),
    paired(door(-3, 6, 7, Material::Wood, Facing::EAST_CCW), 0),
    door(1, 4, 7, Material::Wood, Facing::SOUTH_CW),
    door(1, -4, 7, Material::Wood, Facing::SOUTH_CW),
    door(4, -1, 7, Material::Wood, Facing::WEST_CW),
];
const LARGE_MARBLE: [DoorSpec; 2] = [
    paired(door(-4, 3, 4, Material::Metal, Facing::WEST_CW), 1),
    paired(door(-3, 3, 4, Material::Metal, Facing::EAST_CCW), 0),
];
const SMALL_TOWER: [DoorSpec; 1] = [door(3, 3, 6, Material::Metal, Facing::WEST_CW)];
const LOG_CABIN: [DoorSpec; 2] = [
    door(1, 4, 8, Material::Wood, Facing::WEST_CW),
    door(1, 0, 29, Material::Wood, Facing::WEST_CW),
];
const SANDSTONE: [DoorSpec; 1] = [door(-1, 3, 6, Material::Wood, Facing::WEST_CW)];
const VILLA: [DoorSpec; 4] = [
    paired(door(3, 1, 5, Material::Wood, Facing::WEST_CW), 1),
    paired(door(4, 1, 5, Material::Wood, Facing::EAST_CCW), 0),
    door(1, 0, 25, Material::Wood, Facing::SOUTH_CW),
    door(-3, -1, 25, Material::Wood, Facing::WEST_CW),
];
const SHOP_STONE: [DoorSpec; 1] = [door(-2, 0, 27, Material::Metal, Facing::EAST_CW)];
const SHOP_MARBLE: [DoorSpec; 1] = [door(-2, 0, 24, Material::Metal, Facing::EAST_CW)];

fn specs(multi: MultiId) -> &'static [DoorSpec] {
    match multi.0 {
        0x0064 | 0x0066 | 0x0068 | 0x006A | 0x006C | 0x006E => &SMALL_OLD,
        0x0074 => &GUILD,
        0x0076 => &TWO_STOREY_WOOD,
        0x0078 => &TWO_STOREY_STONE,
        0x007A => &TOWER,
        0x007C => &KEEP,
        0x007E => &CASTLE,
        0x008C => &LARGE_PATIO,
        0x0096 => &LARGE_MARBLE,
        0x0098 => &SMALL_TOWER,
        0x009A => &LOG_CABIN,
        0x009C => &SANDSTONE,
        0x009E => &VILLA,
        0x00A0 => &SHOP_STONE,
        0x00A2 => &SHOP_MARBLE,
        _ => &[],
    }
}

impl DoorSpec {
    fn at(self, origin: Point) -> Option<Point> {
        let x = u16::try_from(i32::from(origin.x) + i32::from(self.dx)).ok()?;
        let y = u16::try_from(i32::from(origin.y) + i32::from(self.dy)).ok()?;
        let z = i8::try_from(i32::from(origin.z) + i32::from(self.dz)).ok()?;
        Some(Point::new(x, y, z))
    }

    fn component(self) -> Door {
        let base = match self.material {
            Material::Wood => 0x06A5,
            Material::Metal => 0x0675,
        };
        let closed = base + 2 * self.facing.0;
        let (offset_x, offset_y) = OFFSETS[usize::from(self.facing.0)];
        Door {
            closed: Graphic(closed),
            open: Graphic(closed + 1),
            offset_x,
            offset_y,
            link: None,
            is_open: false,
            close_at: WorldTick::ZERO,
        }
    }
}

/// ServUO's hinge offsets, indexed by `DoorFacing`.
const OFFSETS: [(i16, i16); 8] = [
    (-1, 1),
    (1, 1),
    (-1, 0),
    (1, -1),
    (1, 1),
    (1, -1),
    (0, 0),
    (0, -1),
];

/// Put every separately-defined fixture into a classic house, reusing a door
/// already restored or laid by the content pack at the same exact frame.
pub(crate) fn install(state: &mut WorldState, house: EntityId, facet: Facet, origin: Point, multi: MultiId) {
    let Some(house_serial) = state.registry.serial_of(house) else {
        return;
    };
    let mut installed = Vec::with_capacity(specs(multi).len());
    for spec in specs(multi) {
        let Some(at) = spec.at(origin) else {
            installed.push(None);
            continue;
        };
        let existing = state
            .registry
            .query::<Door>()
            .map(|(entity, _)| entity)
            .find(|&entity| {
                state.facet_of(entity) == facet
                    && state
                        .registry
                        .get::<Position>(entity)
                        .is_some_and(|position| position.0 == at)
            });
        let entity = match existing {
            Some(entity) => entity,
            None => match spawn(state, house_serial, facet, at, spec.component()) {
                Some(entity) => entity,
                None => {
                    installed.push(None);
                    continue;
                }
            },
        };
        state.registry.insert(entity, HouseDoor { house: house_serial });
        installed.push(Some(entity));
    }

    for (row, spec) in specs(multi).iter().enumerate() {
        let (Some(entity), Some(other)) = (installed[row], spec.link.and_then(|link| installed[link])) else {
            continue;
        };
        let Some(serial) = state.registry.serial_of(other) else {
            continue;
        };
        let Some(mut component) = state.registry.get::<Door>(entity).copied() else {
            continue;
        };
        component.link = Some(serial);
        state.registry.insert(entity, component);
    }
}

fn spawn(
    state: &mut WorldState,
    house: openshard_protocol::serial::Serial,
    facet: Facet,
    at: Point,
    component: Door,
) -> Option<EntityId> {
    let (entity, _) = state.registry.spawn_with_serial(SerialKind::Item).ok()?;
    state.registry.insert(
        entity,
        Drawn {
            id: component.closed,
            hue: Hue::NONE,
        },
    );
    establish_item_location(state, entity, ItemLocation::ground(facet, at))
        .expect("a fresh house door has one valid ground location");
    // Decoration is the existing persistence boundary for a functional door.
    // It says this fixture is fixed world content rather than loose loot.
    state.registry.insert(entity, Decoration);
    state.registry.insert(entity, component);
    state.registry.insert(entity, HouseDoor { house });
    state.place_item(facet, entity, at);
    state.facet_state_mut(facet).block(
        at.x,
        at.y,
        entity,
        openshard_map::overlay::Cover::door(at.z, openshard_state::DOOR_HEIGHT),
    );
    state.reveal(entity);
    Some(entity)
}

/// Whether a house-owned door is one of the fixtures installed with this
/// classic house. An open leaf is tested at its frame, not where it swung to.
pub(crate) fn is_fixture(state: &WorldState, entity: EntityId, origin: Point, multi: MultiId) -> bool {
    let (Some(&Position(mut at)), Some(component)) = (
        state.registry.get::<Position>(entity),
        state.registry.get::<Door>(entity),
    ) else {
        return false;
    };
    if component.is_open {
        at.x = (i32::from(at.x) - i32::from(component.offset_x)).clamp(0, i32::from(u16::MAX)) as u16;
        at.y = (i32::from(at.y) - i32::from(component.offset_y)).clamp(0, i32::from(u16::MAX)) as u16;
    }
    specs(multi).iter().any(|spec| spec.at(origin) == Some(at))
}
