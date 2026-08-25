//! The authoritative ownership edge of every live item.
//!
//! `ItemLocation` is the source of truth.  The historical `Position`,
//! `Contained`, and `Equipped` columns remain during the migration as read
//! projections for spatial, container, and outfit code; this module is the one
//! door through which the canonical edge is established or changed.

use crate::components::{Body, Container, Drawn, Equipped, ItemLocation, Position, SettledItemLocation};
use crate::{HeldItem, WorldState, runtime::Origin};
use openshard_entities::EntityId;
use openshard_protocol::serial::Serial;
use std::collections::HashSet;

/// Why an ownership edge could not be installed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LocationError {
    /// Only an item — an entity carrying `Drawn` — may enter the item graph.
    NotAnItem,
    /// A newly-established item already had a canonical parent.
    AlreadyLocated,
    /// A relocation was asked of an item the graph did not know.
    Unlocated,
    /// The destination serial resolves to no live entity.
    MissingParent,
    /// A contained destination is not a container.
    NotAContainer,
    /// An equipped destination is not a mobile.
    NotAMobile,
    /// The paperdoll layer is already occupied.
    LayerOccupied,
    /// Putting the item there would make it contain itself, directly or through
    /// another container.
    ContainerCycle,
    /// The connection does not exist or already holds another item.
    CursorUnavailable,
    /// A lift did not preserve the item's actual settled origin.
    OriginMismatch,
}

/// A contradiction found by [`audit_item_graph`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ItemGraphViolation {
    /// A drawn item has no canonical parent at all.
    UnlocatedItem(EntityId),
    /// A location component was installed on something that is not an item.
    LocationOnNonItem(EntityId),
    /// The canonical destination no longer exists or violates a graph rule.
    InvalidDestination { item: EntityId, error: LocationError },
    /// `Position`/`Facet` do not project the canonical ground edge exactly.
    GroundProjection(EntityId),
    /// `Contained` does not project the canonical container edge exactly.
    ContainedProjection(EntityId),
    /// `Equipped` does not project the canonical paperdoll edge exactly.
    EquippedProjection(EntityId),
    /// A held edge and the connection's cursor row disagree.
    CursorProjection(EntityId),
    /// A connection cursor names an item whose canonical edge says otherwise.
    CursorWithoutItemLocation(EntityId),
}

/// Read an item's canonical ownership edge.
#[must_use]
pub fn item_location(state: &WorldState, item: EntityId) -> Option<ItemLocation> {
    state.registry.get::<ItemLocation>(item).copied()
}

/// Items whose canonical parent is `container`.
pub fn contained_items(
    state: &WorldState,
    container: Serial,
) -> impl Iterator<Item = (EntityId, crate::components::Contained)> + '_ {
    state
        .registry
        .query::<ItemLocation>()
        .filter_map(move |(item, location)| match *location {
            ItemLocation::Settled(SettledItemLocation::Contained(contained))
                if contained.container == container =>
            {
                Some((item, contained))
            }
            _ => None,
        })
}

/// Items canonically worn by `mobile`.
pub fn equipped_items(state: &WorldState, mobile: Serial) -> impl Iterator<Item = (EntityId, Equipped)> + '_ {
    state
        .registry
        .query::<ItemLocation>()
        .filter_map(move |(item, location)| match *location {
            ItemLocation::Settled(SettledItemLocation::Equipped(equipped)) if equipped.mobile == mobile => {
                Some((item, equipped))
            }
            _ => None,
        })
}

/// Put a newly-created item into the ownership graph.
///
/// The canonical edge and its `Position`, `Contained`, or `Equipped` projection
/// are installed together. Spatial indexing and packets remain higher-level
/// concerns because they describe visibility, not ownership.
pub fn establish_item_location(
    state: &mut WorldState,
    item: EntityId,
    location: ItemLocation,
) -> Result<(), LocationError> {
    if !state.registry.has::<Drawn>(item) {
        return Err(LocationError::NotAnItem);
    }
    if state.registry.has::<ItemLocation>(item) {
        return Err(LocationError::AlreadyLocated);
    }
    validate_destination(state, item, location)?;
    state.registry.insert(item, location);
    apply_projection(state, item, None, location);
    Ok(())
}

/// Atomically replace the canonical parent of an existing item.
///
/// Validation happens before the write, so a rejected transition leaves the old
/// edge and all projections intact. Spatial-index and packet changes stay with
/// the gameplay operation that knows which clients are affected.
pub fn relocate_item(
    state: &mut WorldState,
    item: EntityId,
    location: ItemLocation,
) -> Result<ItemLocation, LocationError> {
    let Some(previous) = item_location(state, item) else {
        return Err(LocationError::Unlocated);
    };
    validate_cursor_projection(state, item, previous)?;
    if let ItemLocation::Held { origin, .. } = location {
        let expected = match previous {
            ItemLocation::Settled(previous) => previous,
            ItemLocation::Held { origin, .. } => origin,
        };
        if origin != expected {
            return Err(LocationError::OriginMismatch);
        }
    }
    validate_destination(state, item, location)?;
    state.registry.insert(item, location);
    apply_projection(state, item, Some(previous), location);
    Ok(previous)
}

/// Remove an item while also releasing any cursor that owns it.
///
/// Container subtree policy, spatial indexing, and removal packets belong to
/// the caller; this function owns the canonical edge and its cursor projection.
pub fn despawn_item(state: &mut WorldState, item: EntityId) -> bool {
    if let Some(ItemLocation::Held { connection, .. }) = item_location(state, item) {
        if let Some(row) = state.connections.get_mut(&connection) {
            if row.held.is_some_and(|held| held.entity == item) {
                row.held = None;
            }
        }
    }
    state.registry.despawn(item)
}

/// Check the entire live ownership forest and all temporary read projections.
///
/// This is intentionally an explicit audit rather than work paid on every game
/// tick.  Tests call it after compound operations, and a debug/admin diagnostic
/// can call it at a boundary such as boot restore.  An empty result means every
/// drawn item has one parent, every parent is legal, and the legacy views agree
/// with that parent.
#[must_use]
pub fn audit_item_graph(state: &WorldState) -> Vec<ItemGraphViolation> {
    let mut violations = Vec::new();

    for (item, _) in state.registry.query::<Drawn>() {
        if !state.registry.has::<ItemLocation>(item) {
            violations.push(ItemGraphViolation::UnlocatedItem(item));
        }
    }

    for (item, &location) in state.registry.query::<ItemLocation>() {
        if !state.registry.has::<Drawn>(item) {
            violations.push(ItemGraphViolation::LocationOnNonItem(item));
            continue;
        }
        if let Err(error) = validate_destination(state, item, location) {
            violations.push(ItemGraphViolation::InvalidDestination { item, error });
        }

        let has_position = state.registry.has::<Position>(item);
        let has_contained = state.registry.has::<crate::components::Contained>(item);
        let has_equipped = state.registry.has::<Equipped>(item);
        match location {
            ItemLocation::Settled(SettledItemLocation::Ground { facet, position }) => {
                if state.registry.get::<Position>(item) != Some(&Position(position))
                    || state.registry.get::<openshard_protocol::world::Facet>(item) != Some(&facet)
                    || has_contained
                    || has_equipped
                {
                    violations.push(ItemGraphViolation::GroundProjection(item));
                }
            }
            ItemLocation::Settled(SettledItemLocation::Contained(contained)) => {
                if state.registry.get::<crate::components::Contained>(item) != Some(&contained)
                    || has_position
                    || has_equipped
                {
                    violations.push(ItemGraphViolation::ContainedProjection(item));
                }
            }
            ItemLocation::Settled(SettledItemLocation::Equipped(equipped)) => {
                if state.registry.get::<Equipped>(item) != Some(&equipped) || has_position || has_contained {
                    violations.push(ItemGraphViolation::EquippedProjection(item));
                }
            }
            ItemLocation::Held { connection, origin } => {
                let cursor_matches = state
                    .connections
                    .get(&connection)
                    .and_then(|row| row.held)
                    .is_some_and(|held| held.entity == item && settled_from_origin(held.origin) == origin);
                if has_position || has_contained || has_equipped || !cursor_matches {
                    violations.push(ItemGraphViolation::CursorProjection(item));
                }
            }
        }
    }

    for (&connection, row) in &state.connections {
        let Some(held) = row.held else {
            continue;
        };
        if !matches!(
            item_location(state, held.entity),
            Some(ItemLocation::Held { connection: held_by, origin })
                if held_by == connection && origin == settled_from_origin(held.origin)
        ) {
            violations.push(ItemGraphViolation::CursorWithoutItemLocation(held.entity));
        }
    }

    violations
}

/// Make the historical columns a mechanically-derived view of the canonical
/// edge.  Spatial indexing and packets remain higher-level concerns, but an
/// item can no longer be both in a pack and on a paperdoll because those two
/// columns are replaced together here.
fn apply_projection(
    state: &mut WorldState,
    item: EntityId,
    previous: Option<ItemLocation>,
    location: ItemLocation,
) {
    if let Some(ItemLocation::Held { connection, .. }) = previous {
        if let Some(row) = state.connections.get_mut(&connection) {
            if row.held.is_some_and(|held| held.entity == item) {
                row.held = None;
            }
        }
    }
    state.registry.remove::<Position>(item);
    state.registry.remove::<crate::components::Contained>(item);
    state.registry.remove::<Equipped>(item);

    match location {
        ItemLocation::Settled(SettledItemLocation::Ground { facet, position }) => {
            state.registry.insert(item, Position(position));
            state.registry.insert(item, facet);
        }
        ItemLocation::Settled(SettledItemLocation::Contained(contained)) => {
            state.registry.insert(item, contained);
        }
        ItemLocation::Settled(SettledItemLocation::Equipped(equipped)) => {
            state.registry.insert(item, equipped);
        }
        ItemLocation::Held { connection, origin } => {
            state
                .connections
                .get_mut(&connection)
                .expect("validated connection")
                .held = Some(HeldItem {
                entity: item,
                origin: origin_from_settled(origin),
            });
        }
    }
}

/// Translate the drag protocol's remembered origin into the canonical settled
/// edge stored on the item while it is held.
#[must_use]
pub const fn settled_from_origin(origin: Origin) -> SettledItemLocation {
    match origin {
        Origin::Ground { position, facet } => SettledItemLocation::Ground { facet, position },
        Origin::Container(contained) => SettledItemLocation::Contained(contained),
        Origin::Worn(equipped) => SettledItemLocation::Equipped(equipped),
    }
}

fn validate_destination(
    state: &WorldState,
    item: EntityId,
    location: ItemLocation,
) -> Result<(), LocationError> {
    match location {
        ItemLocation::Settled(SettledItemLocation::Ground { facet, .. }) => {
            if !state.facets.contains_key(&facet) {
                return Err(LocationError::MissingParent);
            }
        }
        ItemLocation::Settled(SettledItemLocation::Contained(contained)) => {
            validate_container(state, item, contained.container)?;
        }
        ItemLocation::Settled(SettledItemLocation::Equipped(equipped)) => {
            let Some(mobile) = state.registry.entity_of(equipped.mobile) else {
                return Err(LocationError::MissingParent);
            };
            if !state.registry.has::<Body>(mobile) {
                return Err(LocationError::NotAMobile);
            }
            let occupied = state.registry.query::<ItemLocation>().any(|(other, location)| {
                other != item
                    && matches!(
                        location,
                        ItemLocation::Settled(SettledItemLocation::Equipped(worn))
                            if worn.mobile == equipped.mobile && worn.layer == equipped.layer
                    )
            });
            if occupied {
                return Err(LocationError::LayerOccupied);
            }
        }
        ItemLocation::Held { connection, origin } => {
            validate_destination(state, item, ItemLocation::Settled(origin))?;
            let Some(row) = state.connections.get(&connection) else {
                return Err(LocationError::CursorUnavailable);
            };
            if row.held.is_some_and(|held| held.entity != item) {
                return Err(LocationError::CursorUnavailable);
            }
            let occupied = state.registry.query::<ItemLocation>().any(|(other, location)| {
                other != item
                    && matches!(location, ItemLocation::Held { connection: held_by, .. } if *held_by == connection)
            });
            if occupied {
                return Err(LocationError::CursorUnavailable);
            }
        }
    }
    Ok(())
}

fn validate_cursor_projection(
    state: &WorldState,
    item: EntityId,
    location: ItemLocation,
) -> Result<(), LocationError> {
    let ItemLocation::Held { connection, origin } = location else {
        return Ok(());
    };
    let expected = HeldItem {
        entity: item,
        origin: origin_from_settled(origin),
    };
    if state.connections.get(&connection).and_then(|row| row.held) != Some(expected) {
        return Err(LocationError::CursorUnavailable);
    }
    Ok(())
}

const fn origin_from_settled(location: SettledItemLocation) -> Origin {
    match location {
        SettledItemLocation::Ground { facet, position } => Origin::Ground { position, facet },
        SettledItemLocation::Contained(contained) => Origin::Container(contained),
        SettledItemLocation::Equipped(equipped) => Origin::Worn(equipped),
    }
}

fn validate_container(state: &WorldState, item: EntityId, container: Serial) -> Result<(), LocationError> {
    let Some(mut parent) = state.registry.entity_of(container) else {
        return Err(LocationError::MissingParent);
    };
    if !state.registry.has::<Container>(parent) {
        return Err(LocationError::NotAContainer);
    }

    // Follow canonical parent edges only. A container on the ground, equipped,
    // or held is a root for this purpose; a contained container walks upward.
    // Reaching `item` means the proposed edge closes a cycle.
    let mut visited = HashSet::new();
    loop {
        if parent == item || !visited.insert(parent) {
            return Err(LocationError::ContainerCycle);
        }
        let Some(ItemLocation::Settled(SettledItemLocation::Contained(above))) =
            state.registry.get::<ItemLocation>(parent).copied()
        else {
            break;
        };
        let Some(next) = state.registry.entity_of(above.container) else {
            return Err(LocationError::MissingParent);
        };
        parent = next;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FacetState;
    use crate::components::Contained;
    use crate::components::{Drawn, Equipped};
    use openshard_protocol::access::AccessLevel;
    use openshard_protocol::containers::GridSlot;
    use openshard_protocol::gump::GumpPoint;
    use openshard_protocol::identity::AccountName;
    use openshard_protocol::serial::SerialKind;
    use openshard_protocol::version::ClientVersion;
    use openshard_protocol::wire::{Graphic, Hue, Layer};
    use openshard_protocol::world::{Facet, Point};
    use std::collections::BTreeMap;

    fn world() -> WorldState {
        let tiles = openshard_tiles::TileData::empty();
        let mut facets = BTreeMap::new();
        facets.insert(
            Facet(0),
            FacetState::new(
                None,
                None,
                16,
                16,
                crate::facet_rules::FacetRules::classic(Facet(0)),
                None,
                &tiles,
            ),
        );
        WorldState::new(
            facets,
            Facet(0),
            tiles,
            openshard_uofiles::multi::Multis::default(),
            (0, 0),
            1,
        )
    }

    fn item(state: &mut WorldState, graphic: u16) -> (EntityId, Serial) {
        let (entity, serial) = state
            .registry
            .spawn_with_serial(SerialKind::Item)
            .expect("an item serial");
        state.registry.insert(
            entity,
            Drawn {
                id: Graphic(graphic),
                hue: Hue(0),
            },
        );
        (entity, serial)
    }

    #[test]
    fn one_item_cannot_be_established_in_two_places() {
        let mut state = world();
        let (item, _) = item(&mut state, 1);
        let at = Point::new(10, 10, 0);

        assert_eq!(
            establish_item_location(&mut state, item, ItemLocation::ground(Facet(0), at)),
            Ok(())
        );
        assert_eq!(
            establish_item_location(&mut state, item, ItemLocation::ground(Facet(0), at)),
            Err(LocationError::AlreadyLocated)
        );
    }

    #[test]
    fn a_container_cannot_be_put_inside_its_descendant() {
        let mut state = world();
        let (outer, outer_serial) = item(&mut state, 1);
        let (inner, inner_serial) = item(&mut state, 2);
        state.registry.insert(outer, Container { gump: Graphic(1) });
        state.registry.insert(inner, Container { gump: Graphic(1) });
        let at = Point::new(10, 10, 0);
        establish_item_location(&mut state, outer, ItemLocation::ground(Facet(0), at)).unwrap();
        establish_item_location(
            &mut state,
            inner,
            ItemLocation::contained(Contained {
                container: outer_serial,
                position: GumpPoint::new(0, 0),
                grid: GridSlot(0),
            }),
        )
        .unwrap();

        assert_eq!(
            relocate_item(
                &mut state,
                outer,
                ItemLocation::contained(Contained {
                    container: inner_serial,
                    position: GumpPoint::new(0, 0),
                    grid: GridSlot(0),
                }),
            ),
            Err(LocationError::ContainerCycle)
        );
        assert_eq!(
            item_location(&state, outer),
            Some(ItemLocation::ground(Facet(0), at))
        );
    }

    #[test]
    fn one_paperdoll_layer_has_one_item() {
        let mut state = world();
        let (mobile, mobile_serial) = state
            .registry
            .spawn_with_serial(SerialKind::Mobile)
            .expect("a mobile serial");
        state.registry.insert(
            mobile,
            Body {
                id: Graphic(0x0190),
                hue: Hue(0),
            },
        );
        let (first, _) = item(&mut state, 1);
        let (second, _) = item(&mut state, 2);
        let worn = Equipped {
            mobile: mobile_serial,
            layer: Layer(1),
        };

        establish_item_location(&mut state, first, ItemLocation::equipped(worn)).unwrap();
        assert_eq!(
            establish_item_location(&mut state, second, ItemLocation::equipped(worn)),
            Err(LocationError::LayerOccupied)
        );
    }

    #[test]
    fn projections_are_replaced_as_one_location_changes() {
        let mut state = world();
        let (container, container_serial) = item(&mut state, 1);
        state.registry.insert(container, Container { gump: Graphic(1) });
        let at = Point::new(10, 10, 0);
        establish_item_location(&mut state, container, ItemLocation::ground(Facet(0), at)).unwrap();

        let (item, _) = item(&mut state, 2);
        establish_item_location(&mut state, item, ItemLocation::ground(Facet(0), at)).unwrap();
        let contained = Contained {
            container: container_serial,
            position: GumpPoint::new(20, 30),
            grid: GridSlot(1),
        };
        relocate_item(&mut state, item, ItemLocation::contained(contained)).unwrap();

        assert_eq!(state.registry.get::<Contained>(item), Some(&contained));
        assert!(!state.registry.has::<Position>(item));
        assert!(!state.registry.has::<Equipped>(item));
        assert!(audit_item_graph(&state).is_empty());
    }

    #[test]
    fn cursor_projection_moves_and_despawns_with_the_item() {
        let mut state = world();
        let (item, _) = item(&mut state, 1);
        let connection = openshard_gateway::ConnectionId::from_raw(7);
        state.connections.insert(
            connection,
            crate::connection::Connection::new(
                ClientVersion::TOL,
                AccountName("test".to_owned()),
                AccessLevel::Player,
            ),
        );
        let at = Point::new(10, 10, 0);
        establish_item_location(&mut state, item, ItemLocation::ground(Facet(0), at)).unwrap();

        relocate_item(
            &mut state,
            item,
            ItemLocation::Held {
                connection,
                origin: SettledItemLocation::Ground {
                    facet: Facet(0),
                    position: at,
                },
            },
        )
        .unwrap();
        assert_eq!(state.held_of(connection).map(|held| held.entity), Some(item));
        assert!(audit_item_graph(&state).is_empty());

        assert!(despawn_item(&mut state, item));
        assert!(state.held_of(connection).is_none());
        assert!(audit_item_graph(&state).is_empty());
    }

    #[test]
    fn audit_finds_a_projection_modified_behind_the_subsystems_back() {
        let mut state = world();
        let (item, _) = item(&mut state, 1);
        let at = Point::new(10, 10, 0);
        establish_item_location(&mut state, item, ItemLocation::ground(Facet(0), at)).unwrap();

        state.registry.remove::<Position>(item);

        assert_eq!(
            audit_item_graph(&state),
            vec![ItemGraphViolation::GroundProjection(item)]
        );
    }
}
