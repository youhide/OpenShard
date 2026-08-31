//! The authoritative ownership edge of every live item.
//!
//! `ItemLocation` is the source of truth.  The historical `Position`,
//! `Contained`, and `Equipped` columns remain during the migration as read
//! projections for spatial, container, and outfit code; this module is the one
//! door through which the canonical edge is established or changed.

use std::collections::HashSet;

use openshard_entities::EntityId;
use openshard_protocol::serial::Serial;

use crate::components::{
    Body,
    Container,
    Drawn,
    Equipped,
    ItemLocation,
    Position,
    SettledItemLocation,
};
use crate::runtime::Origin;
use crate::{
    HeldItem,
    WorldState,
};

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

/// One ownership-edge replacement whose destination has already been checked.
///
/// Compound item operations prepare this before allocating or changing any
/// quantity. The fields are private so only this module can manufacture a
/// value that [`commit_item_relocation`] may apply without an ordinary failure
/// branch.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PreparedItemRelocation {
    item:        EntityId,
    previous:    ItemLocation,
    destination: ItemLocation,
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
    /// A worn item is missing from its mobile's [`Worn`] index — the one way
    /// that index can be *wrong* rather than merely out of date.
    UnindexedOutfit(EntityId),
    /// A contained item is missing from its parent's [`ContainedItems`] index.
    UnindexedContainedItem(EntityId),
    /// A live contained item occurs more than once in its parent's candidate
    /// list.
    DuplicateContainedCandidate(EntityId),
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

/// What a mobile is wearing, kept on the *mobile* so that asking costs the size
/// of one outfit rather than the size of the world.
///
/// # It is a hint, and the item is still the authority
///
/// This is a second statement of a fact the `Equipped` column already holds, and
/// the usual objection to that is exactly right: two representations of one
/// thing drift apart. What makes it safe here is that the drift can only ever go
/// **one way**, and that way is harmless.
///
/// * An entry can become *stale* — `Registry::despawn` drops an item's
///   components without passing through this module, and there are twenty such
///   call sites. So every read re-asks the item itself
///   ([`equipped_items`]) and ignores anything whose `Equipped` no longer names
///   this mobile. A stale entry costs one lookup and is dropped the next time
///   the outfit is touched.
/// * An entry cannot become *missing*, which is the failure that would be a
///   real bug: `Equipped` is written in exactly one place — [`apply_projection`]
///   — and that place maintains this beside it. [`audit_item_graph`] asserts the
///   pair anyway, because "exactly one place" is a claim, and a claim gets
///   checked.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Worn {
    /// Candidate items. Not authoritative — see the type's own documentation.
    pub items: Vec<EntityId>,
}

/// Candidate direct children kept on one container.
///
/// The item's canonical [`ItemLocation`] remains authoritative. A raw despawn
/// may leave a stale candidate, so every read rechecks the edge; the canonical
/// location door ensures a live child is never missing or duplicated.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ContainedItems {
    /// Candidate children. Stale rows are harmless and are pruned on mutation.
    pub candidates: Vec<EntityId>,
}

/// Items whose canonical parent is `container`.
pub fn contained_items(
    state: &WorldState,
    container: Serial,
) -> impl Iterator<Item = (EntityId, crate::components::Contained)> + '_ {
    let candidates: &[EntityId] = state
        .registry
        .entity_of(container)
        .and_then(|entity| state.registry.get::<ContainedItems>(entity))
        .map_or(&[], |contents| contents.candidates.as_slice());
    candidates.iter().copied().filter_map(move |item| {
        match item_location(state, item) {
            Some(ItemLocation::Settled(SettledItemLocation::Contained(contained)))
                if contained.container == container =>
            {
                Some((item, contained))
            }
            _ => None,
        }
    })
}

/// Every descendant below `root`, in stable serial order.
///
/// Recursion pays only indexed direct-child reads. Canonical edges are checked
/// by [`contained_items`] at every level and the visited set makes this total
/// even over a corrupt restored cycle; the ownership audit still reports that
/// cycle separately.
#[must_use]
pub fn recursive_contained_items(
    state: &WorldState,
    root: Serial,
) -> Vec<(EntityId, crate::components::Contained)> {
    fn visit(
        state: &WorldState,
        container: Serial,
        visited: &mut HashSet<EntityId>,
        descendants: &mut Vec<(EntityId, crate::components::Contained)>,
    ) {
        let mut direct: Vec<_> = contained_items(state, container).collect();
        direct.sort_by_key(|(item, _)| (state.registry.serial_of(*item), *item));
        for (item, contained) in direct {
            if !visited.insert(item) {
                continue;
            }
            descendants.push((item, contained));
            if state.registry.has::<Container>(item) {
                if let Some(serial) = state.registry.serial_of(item) {
                    visit(state, serial, visited, descendants);
                }
            }
        }
    }

    let mut descendants = Vec::new();
    let mut visited = HashSet::new();
    if let Some(root_entity) = state.registry.entity_of(root) {
        visited.insert(root_entity);
    }
    visit(state, root, &mut visited, &mut descendants);
    descendants
}

/// Items canonically worn by `mobile`.
///
/// # Why this reads the projection and not the canonical column
///
/// The answer is the same either way — [`audit_item_graph`] refuses any world
/// where `Equipped` and the canonical edge disagree, and
/// [`apply_projection`] is the only place either is written — but the *cost* is
/// not, and this is a hot path in a way its shape does not admit.
///
/// `query::<ItemLocation>()` walks **every located item in the world**: a
/// restored Felucca is 15,194 items and 26,477 decorations, and every one of
/// them was being examined to find the six things one mobile has on. That is
/// paid per call, and `combat::equipped_weapon_item` makes two of them per
/// mobile per tick — so the cost was the world's item count times its mobile
/// count, every tick, and on a shard with 10,959 mobiles it was **80% of the
/// whole tick** (measured with `perf` on `openshard-e2e-shard`'s `tick_pace`).
/// The tick ran at 6.9 of its declared 40 per second, which made every duration
/// the shard announces — a bow's 1600ms among them — about five times shorter
/// than the one it delivered.
///
/// `Equipped` is the column that answers this question: only worn items are in
/// it. That is what the projection is *for*, and reading the canonical column
/// instead was the migration note in this module's header being followed past
/// the point where it says anything useful.
///
/// Reading the `Equipped` column instead of the canonical one was the first half
/// of the fix and was not enough: that column holds every dressed body in the
/// world, so the scan merely shrank from 41,671 rows to the ~55,000 garments of
/// 10,959 mobiles. The answer is [`Worn`] — the outfit, kept on the mobile — and
/// what is walked here is now one body's clothes.
pub fn equipped_items(state: &WorldState, mobile: Serial) -> impl Iterator<Item = (EntityId, Equipped)> + '_ {
    // The item's own `Equipped` is re-read for every candidate, and that is the
    // whole reason the index may be stale without being wrong. See [`Worn`].
    let worn: &[EntityId] = state
        .registry
        .entity_of(mobile)
        .and_then(|entity| state.registry.get::<Worn>(entity))
        .map_or(&[], |worn| worn.items.as_slice());
    worn.iter().copied().filter_map(move |item| {
        state
            .registry
            .get::<Equipped>(item)
            .filter(|equipped| equipped.mobile == mobile)
            .map(|equipped| (item, *equipped))
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
    let prepared = prepare_item_relocation(state, item, location)?;
    Ok(commit_item_relocation(state, prepared))
}

/// Validate an ownership-edge replacement without changing the world.
///
/// This is the prepare half used by compound operations such as stack splitting:
/// a cursor refusal must be known before the operation allocates a remainder or
/// changes the original pile's amount.
pub fn prepare_item_relocation(
    state: &WorldState,
    item: EntityId,
    destination: ItemLocation,
) -> Result<PreparedItemRelocation, LocationError> {
    let Some(previous) = item_location(state, item) else {
        return Err(LocationError::Unlocated);
    };
    validate_cursor_projection(state, item, previous)?;
    if let ItemLocation::Held { origin, .. } = destination {
        let expected = match previous {
            ItemLocation::Settled(previous) => previous,
            ItemLocation::Held { origin, .. } => origin,
        };
        if origin != expected {
            return Err(LocationError::OriginMismatch);
        }
    }
    validate_destination(state, item, destination)?;
    Ok(PreparedItemRelocation {
        item,
        previous,
        destination,
    })
}

/// Apply a relocation returned by [`prepare_item_relocation`].
///
/// # Panics
///
/// Panics if the item moved between prepare and commit. The world tick is the
/// sole owner and compound operations commit immediately, so that would be a
/// caller violating the prepared-operation contract rather than a gameplay
/// refusal.
pub fn commit_item_relocation(state: &mut WorldState, prepared: PreparedItemRelocation) -> ItemLocation {
    assert_eq!(
        item_location(state, prepared.item),
        Some(prepared.previous),
        "a prepared item relocation commits against the edge it validated"
    );
    state.registry.insert(prepared.item, prepared.destination);
    apply_projection(
        state,
        prepared.item,
        Some(prepared.previous),
        prepared.destination,
    );
    prepared.previous
}

/// Remove an item while also releasing any cursor that owns it.
///
/// Container subtree policy, spatial indexing, and removal packets belong to
/// the caller; this function owns the canonical edge and its cursor projection.
pub fn despawn_item(state: &mut WorldState, item: EntityId) -> bool {
    match item_location(state, item) {
        Some(ItemLocation::Settled(SettledItemLocation::Contained(contained))) => {
            uncontain(state, contained.container, item);
        }
        Some(ItemLocation::Settled(SettledItemLocation::Equipped(equipped))) => {
            unwear(state, equipped.mobile, item);
        }
        Some(ItemLocation::Held { connection, .. }) => {
            if let Some(row) = state.connections.get_mut(&connection) {
                if row.held.is_some_and(|held| held.entity == item) {
                    row.held = None;
                }
            }
        }
        Some(ItemLocation::Settled(SettledItemLocation::Ground { .. })) | None => {}
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
                let occurrences = state
                    .registry
                    .entity_of(contained.container)
                    .and_then(|parent| state.registry.get::<ContainedItems>(parent))
                    .map_or(0, |contents| {
                        contents
                            .candidates
                            .iter()
                            .filter(|&&candidate| candidate == item)
                            .count()
                    });
                if occurrences == 0 {
                    violations.push(ItemGraphViolation::UnindexedContainedItem(item));
                } else if occurrences > 1 {
                    violations.push(ItemGraphViolation::DuplicateContainedCandidate(item));
                }
            }
            ItemLocation::Settled(SettledItemLocation::Equipped(equipped)) => {
                if state.registry.get::<Equipped>(item) != Some(&equipped) || has_position || has_contained {
                    violations.push(ItemGraphViolation::EquippedProjection(item));
                }
                // The outfit index may hold *more* than is worn — a despawn drops
                // an item's components without passing through this module — but
                // never less. A worn item its own mobile does not list is the one
                // failure that would make `equipped_items` answer wrongly instead
                // of merely slowly, so it is the half that is checked.
                let listed = state
                    .registry
                    .entity_of(equipped.mobile)
                    .and_then(|mobile| state.registry.get::<Worn>(mobile))
                    .is_some_and(|worn| worn.items.contains(&item));
                if !listed {
                    violations.push(ItemGraphViolation::UnindexedOutfit(item));
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
    // The outfit index follows the projection it indexes, in the same breath and
    // in the same function, which is what makes "an entry cannot go missing" a
    // property of the code rather than a hope. See [`Worn`].
    if let Some(ItemLocation::Settled(SettledItemLocation::Equipped(equipped))) = previous {
        unwear(state, equipped.mobile, item);
    }
    if let Some(ItemLocation::Settled(SettledItemLocation::Contained(contained))) = previous {
        uncontain(state, contained.container, item);
    }

    match location {
        ItemLocation::Settled(SettledItemLocation::Ground { facet, position }) => {
            state.registry.insert(item, Position(position));
            state.registry.insert(item, facet);
        }
        ItemLocation::Settled(SettledItemLocation::Contained(contained)) => {
            state.registry.insert(item, contained);
            contain(state, contained.container, item);
        }
        ItemLocation::Settled(SettledItemLocation::Equipped(equipped)) => {
            state.registry.insert(item, equipped);
            wear(state, equipped.mobile, item);
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

/// Put `item` into its parent's candidate list, pruning stale rows and any old
/// duplicate of the same live child.
fn contain(state: &mut WorldState, container: Serial, item: EntityId) {
    let Some(parent) = state.registry.entity_of(container) else {
        return;
    };
    let mut candidates = state
        .registry
        .get::<ContainedItems>(parent)
        .map_or_else(Vec::new, |contents| contents.candidates.clone());
    candidates.retain(|&candidate| {
        candidate != item
            && matches!(
                item_location(state, candidate),
                Some(ItemLocation::Settled(SettledItemLocation::Contained(contained)))
                    if contained.container == container
            )
    });
    candidates.push(item);
    state.registry.insert(parent, ContainedItems { candidates });
}

/// Remove `item` from its former parent's candidate list when that parent is
/// still live. A raw despawn may leave a stale row; readers revalidate it.
fn uncontain(state: &mut WorldState, container: Serial, item: EntityId) {
    let Some(parent) = state.registry.entity_of(container) else {
        return;
    };
    let Some(contents) = state.registry.get::<ContainedItems>(parent) else {
        return;
    };
    let candidates: Vec<_> = contents
        .candidates
        .iter()
        .copied()
        .filter(|&candidate| candidate != item)
        .collect();
    state.registry.insert(parent, ContainedItems { candidates });
}

/// Put `item` into `mobile`'s outfit index, pruning whatever has fallen out of
/// it since the last time anybody looked.
///
/// The prune rides here rather than on the read for two reasons: a read takes
/// `&WorldState` and cannot write, and a list is only ever added to at the
/// moment somebody puts something on — so the one place a mobile's outfit grows
/// is also the one place it is worth paying to tidy. Without it a body that is
/// dressed and stripped by a spawner all day would accumulate dead ids forever.
fn wear(state: &mut WorldState, mobile: Serial, item: EntityId) {
    let Some(entity) = state.registry.entity_of(mobile) else {
        // A mobile that does not resolve wears nothing anybody can ask about:
        // `equipped_items` looks this same serial up and finds the same nothing.
        return;
    };
    let mut items: Vec<EntityId> = match state.registry.get::<Worn>(entity) {
        Some(worn) => worn.items.clone(),
        None => Vec::new(),
    };
    items.retain(|&candidate| {
        candidate != item
            && state
                .registry
                .get::<Equipped>(candidate)
                .is_some_and(|equipped| equipped.mobile == mobile)
    });
    items.push(item);
    state.registry.insert(entity, Worn { items });
}

/// Take `item` out of `mobile`'s outfit index.
fn unwear(state: &mut WorldState, mobile: Serial, item: EntityId) {
    let Some(entity) = state.registry.entity_of(mobile) else {
        return;
    };
    let Some(worn) = state.registry.get::<Worn>(entity) else {
        return;
    };
    let items: Vec<EntityId> = worn.items.iter().copied().filter(|&worn| worn != item).collect();
    state.registry.insert(entity, Worn { items });
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
    use std::collections::BTreeMap;

    use openshard_protocol::access::AccessLevel;
    use openshard_protocol::containers::GridSlot;
    use openshard_protocol::gump::GumpPoint;
    use openshard_protocol::identity::AccountName;
    use openshard_protocol::serial::SerialKind;
    use openshard_protocol::version::ClientVersion;
    use openshard_protocol::wire::{
        Graphic,
        Hue,
        Layer,
    };
    use openshard_protocol::world::{
        Facet,
        Point,
    };
    use proptest::prelude::*;

    use super::*;
    use crate::FacetState;
    use crate::components::{
        Contained,
        Drawn,
        Equipped,
    };

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
            openshard_map::grid::Tile::new(0, 0),
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
                id:  Graphic(graphic),
                hue: Hue(0),
            },
        );
        (entity, serial)
    }

    fn slow_contained(state: &WorldState, container: Serial) -> Vec<EntityId> {
        let mut children: Vec<_> = state
            .registry
            .query::<ItemLocation>()
            .filter_map(|(item, location)| {
                matches!(
                    location,
                    ItemLocation::Settled(SettledItemLocation::Contained(contained))
                        if contained.container == container
                )
                .then_some(item)
            })
            .collect();
        children.sort();
        children
    }

    fn indexed_contained(state: &WorldState, container: Serial) -> Vec<EntityId> {
        let mut children: Vec<_> = contained_items(state, container).map(|(item, _)| item).collect();
        children.sort();
        children
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
                position:  GumpPoint::new(0, 0),
                grid:      GridSlot(0),
            }),
        )
        .unwrap();

        assert_eq!(
            relocate_item(
                &mut state,
                outer,
                ItemLocation::contained(Contained {
                    container: inner_serial,
                    position:  GumpPoint::new(0, 0),
                    grid:      GridSlot(0),
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
                id:  Graphic(0x0190),
                hue: Hue(0),
            },
        );
        let (first, _) = item(&mut state, 1);
        let (second, _) = item(&mut state, 2);
        let worn = Equipped {
            mobile: mobile_serial,
            layer:  Layer(1),
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
            position:  GumpPoint::new(20, 30),
            grid:      GridSlot(1),
        };
        relocate_item(&mut state, item, ItemLocation::contained(contained)).unwrap();

        assert_eq!(state.registry.get::<Contained>(item), Some(&contained));
        assert!(!state.registry.has::<Position>(item));
        assert!(!state.registry.has::<Equipped>(item));
        assert!(audit_item_graph(&state).is_empty());
    }

    #[test]
    fn container_membership_follows_the_canonical_edge_between_parents() {
        let mut state = world();
        let at = Point::new(10, 10, 0);
        let (first, first_serial) = item(&mut state, 1);
        let (second, second_serial) = item(&mut state, 2);
        for container in [first, second] {
            state.registry.insert(container, Container { gump: Graphic(1) });
            establish_item_location(&mut state, container, ItemLocation::ground(Facet(0), at)).unwrap();
        }
        let (child, _) = item(&mut state, 3);
        let in_first = Contained {
            container: first_serial,
            position:  GumpPoint::new(10, 20),
            grid:      GridSlot(1),
        };
        establish_item_location(&mut state, child, ItemLocation::contained(in_first)).unwrap();

        assert_eq!(
            contained_items(&state, first_serial).collect::<Vec<_>>(),
            vec![(child, in_first)]
        );
        assert!(contained_items(&state, second_serial).next().is_none());

        let in_second = Contained {
            container: second_serial,
            position:  GumpPoint::new(30, 40),
            grid:      GridSlot(2),
        };
        relocate_item(&mut state, child, ItemLocation::contained(in_second)).unwrap();

        assert!(contained_items(&state, first_serial).next().is_none());
        assert_eq!(
            contained_items(&state, second_serial).collect::<Vec<_>>(),
            vec![(child, in_second)]
        );
        assert!(audit_item_graph(&state).is_empty());
    }

    #[test]
    fn a_stale_container_candidate_is_never_returned_as_a_live_child() {
        let mut state = world();
        let at = Point::new(10, 10, 0);
        let (container, container_serial) = item(&mut state, 1);
        state.registry.insert(container, Container { gump: Graphic(1) });
        establish_item_location(&mut state, container, ItemLocation::ground(Facet(0), at)).unwrap();
        let (child, _) = item(&mut state, 2);
        establish_item_location(
            &mut state,
            child,
            ItemLocation::contained(Contained {
                container: container_serial,
                position:  GumpPoint::new(10, 20),
                grid:      GridSlot(1),
            }),
        )
        .unwrap();

        assert!(
            state.registry.despawn(child),
            "the fixture bypasses the cleanup door"
        );

        assert!(contained_items(&state, container_serial).next().is_none());
        assert!(audit_item_graph(&state).is_empty());
    }

    #[test]
    fn the_item_graph_audit_names_missing_and_duplicate_container_membership() {
        let mut state = world();
        let at = Point::new(10, 10, 0);
        let (container, container_serial) = item(&mut state, 1);
        state.registry.insert(container, Container { gump: Graphic(1) });
        establish_item_location(&mut state, container, ItemLocation::ground(Facet(0), at)).unwrap();
        let (child, _) = item(&mut state, 2);
        establish_item_location(
            &mut state,
            child,
            ItemLocation::contained(Contained {
                container: container_serial,
                position:  GumpPoint::new(10, 20),
                grid:      GridSlot(1),
            }),
        )
        .unwrap();

        state.registry.insert(container, ContainedItems::default());
        assert_eq!(
            audit_item_graph(&state),
            vec![ItemGraphViolation::UnindexedContainedItem(child)]
        );

        state.registry.insert(
            container,
            ContainedItems {
                candidates: vec![child, child],
            },
        );
        assert_eq!(
            audit_item_graph(&state),
            vec![ItemGraphViolation::DuplicateContainedCandidate(child)]
        );
    }

    #[test]
    fn recursive_container_walk_is_depth_first_and_serial_ordered() {
        let mut state = world();
        let at = Point::new(10, 10, 0);
        let (root, root_serial) = item(&mut state, 1);
        state.registry.insert(root, Container { gump: Graphic(1) });
        establish_item_location(&mut state, root, ItemLocation::ground(Facet(0), at)).unwrap();

        let (lower_serial_child, _) = item(&mut state, 2);
        let (nested, nested_serial) = item(&mut state, 3);
        state.registry.insert(nested, Container { gump: Graphic(1) });
        let (nested_child, _) = item(&mut state, 4);

        // Insert in the opposite order to prove the traversal is not the
        // candidate vector's incidental insertion order.
        establish_item_location(
            &mut state,
            nested,
            ItemLocation::contained(Contained {
                container: root_serial,
                position:  GumpPoint::new(30, 40),
                grid:      GridSlot(2),
            }),
        )
        .unwrap();
        establish_item_location(
            &mut state,
            lower_serial_child,
            ItemLocation::contained(Contained {
                container: root_serial,
                position:  GumpPoint::new(10, 20),
                grid:      GridSlot(1),
            }),
        )
        .unwrap();
        establish_item_location(
            &mut state,
            nested_child,
            ItemLocation::contained(Contained {
                container: nested_serial,
                position:  GumpPoint::new(50, 60),
                grid:      GridSlot(3),
            }),
        )
        .unwrap();

        let walked: Vec<_> = recursive_contained_items(&state, root_serial)
            .into_iter()
            .map(|(item, _)| item)
            .collect();
        assert_eq!(walked, vec![lower_serial_child, nested, nested_child]);
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
                    facet:    Facet(0),
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
    fn despawning_a_contained_item_removes_its_parent_candidate() {
        let mut state = world();
        let at = Point::new(10, 10, 0);
        let (container, container_serial) = item(&mut state, 1);
        state.registry.insert(container, Container { gump: Graphic(1) });
        establish_item_location(&mut state, container, ItemLocation::ground(Facet(0), at)).unwrap();
        let (child, _) = item(&mut state, 2);
        establish_item_location(
            &mut state,
            child,
            ItemLocation::contained(Contained {
                container: container_serial,
                position:  GumpPoint::new(10, 20),
                grid:      GridSlot(1),
            }),
        )
        .unwrap();

        assert!(despawn_item(&mut state, child));

        assert!(contained_items(&state, container_serial).next().is_none());
        assert_eq!(
            state.registry.get::<ContainedItems>(container),
            Some(&ContainedItems::default())
        );
        assert!(audit_item_graph(&state).is_empty());
    }

    proptest! {
        #[test]
        fn indexed_membership_matches_a_slow_scan_after_mutation_sequences(
            actions in prop::collection::vec((0_u8..8, 0_u8..4), 1..=128),
        ) {
            let mut state = world();
            let at = Point::new(10, 10, 0);
            let (first, first_serial) = item(&mut state, 1);
            let (second, second_serial) = item(&mut state, 2);
            for container in [first, second] {
                state.registry.insert(container, Container { gump: Graphic(1) });
                establish_item_location(&mut state, container, ItemLocation::ground(Facet(0), at))
                    .unwrap();
            }
            let children: Vec<_> = (0..8)
                .map(|graphic| {
                    let (child, _) = item(&mut state, graphic + 10);
                    establish_item_location(
                        &mut state,
                        child,
                        ItemLocation::ground(Facet(0), at),
                    )
                    .unwrap();
                    child
                })
                .collect();

            for (slot, destination) in actions {
                let child = children[usize::from(slot)];
                if !state.registry.contains(child) {
                    continue;
                }
                match destination {
                    0 => {
                        relocate_item(&mut state, child, ItemLocation::ground(Facet(0), at)).unwrap();
                    }
                    1 | 2 => {
                        let container = if destination == 1 { first_serial } else { second_serial };
                        relocate_item(
                            &mut state,
                            child,
                            ItemLocation::contained(Contained {
                                container,
                                position: GumpPoint::new(i32::from(slot), i32::from(destination)),
                                grid: GridSlot(slot),
                            }),
                        )
                        .unwrap();
                    }
                    3 => {
                        despawn_item(&mut state, child);
                    }
                    _ => unreachable!("the strategy emits four destinations"),
                }

                prop_assert_eq!(indexed_contained(&state, first_serial), slow_contained(&state, first_serial));
                prop_assert_eq!(indexed_contained(&state, second_serial), slow_contained(&state, second_serial));
                prop_assert!(audit_item_graph(&state).is_empty());
            }
        }
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
