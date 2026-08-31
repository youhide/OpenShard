//! Permission-filtered, read-only search over indexed house storage.

use std::collections::HashSet;

use openshard_entities::EntityId;
use openshard_protocol::serial::Serial;
use openshard_state::components::{
    CorpseBody,
    House,
    ItemLocation,
    LockedDown,
    Position,
    SettledItemLocation,
    Standing,
    TradeWindow,
};
use openshard_state::{
    HouseInventoryCursor,
    HouseInventoryError,
    HouseInventoryPage,
    HouseItemIdentity,
    WorldState,
    house_item_identity,
};

/// Why a player cannot search the house inventory.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SearchRefusal {
    NotInAHouse,
    Banned,
    Index(HouseInventoryError),
}

/// Search the eligible storage of the house `actor` currently occupies.
///
/// Selectors are exact identities produced by the static item catalogue; text
/// and category matching therefore never makes the realtime tick enumerate the
/// house's items. Results are presentation only. Use [`resolve`] before opening
/// or highlighting a returned pile.
pub fn search(
    state: &WorldState,
    actor: EntityId,
    expected_epoch: Option<u64>,
    selectors: &[HouseItemIdentity],
    after: Option<HouseInventoryCursor>,
    limit: usize,
) -> Result<HouseInventoryPage, SearchRefusal> {
    let (house, serial, standing) = current_house(state, actor)?;
    let _ = house;
    state
        .house_inventory_page(serial, standing, expected_epoch, selectors, after, limit)
        .map_err(SearchRefusal::Index)
}

/// Revalidate one exact result before opening or highlighting it.
///
/// This grants no right to move or consume the item. It only returns the live
/// entity when the actor, house, root, containment path, identity and projection
/// epoch still agree with the page that named it.
#[must_use]
pub fn resolve(
    state: &WorldState,
    actor: EntityId,
    epoch: u64,
    identity: HouseItemIdentity,
    root: Serial,
    item: Serial,
) -> Option<EntityId> {
    let (house, house_serial, standing) = current_house(state, actor).ok()?;
    if !state.house_inventory_contains(house_serial, epoch, standing, identity, root, item) {
        return None;
    }

    let root_entity = state.registry.entity_of(root)?;
    let locked = state.registry.get::<LockedDown>(root_entity)?;
    if locked.house != house_serial || minimum_standing(*locked) > standing || excluded(state, root_entity) {
        return None;
    }
    let ItemLocation::Settled(SettledItemLocation::Ground { facet, position }) =
        *state.registry.get::<ItemLocation>(root_entity)?
    else {
        return None;
    };
    if crate::house_at(state, position, facet) != Some(house) {
        return None;
    }

    let item_entity = state.registry.entity_of(item)?;
    if excluded(state, item_entity)
        || house_item_identity(state, item_entity) != Some(identity)
        || !descends_from(state, item_entity, root_entity)
    {
        return None;
    }
    Some(item_entity)
}

fn current_house(state: &WorldState, actor: EntityId) -> Result<(EntityId, Serial, Standing), SearchRefusal> {
    let &Position(at) = state
        .registry
        .get::<Position>(actor)
        .ok_or(SearchRefusal::NotInAHouse)?;
    let house = crate::house_at(state, at, state.facet_of(actor)).ok_or(SearchRefusal::NotInAHouse)?;
    let entry = state
        .registry
        .get::<House>(house)
        .ok_or(SearchRefusal::NotInAHouse)?;
    let actor_serial = state
        .registry
        .serial_of(actor)
        .ok_or(SearchRefusal::NotInAHouse)?;
    let standing = entry.standing_of(actor_serial, state.is_staff(actor));
    if standing == Standing::Banned {
        return Err(SearchRefusal::Banned);
    }
    let serial = state
        .registry
        .serial_of(house)
        .ok_or(SearchRefusal::NotInAHouse)?;
    Ok((house, serial, standing))
}

fn minimum_standing(locked: LockedDown) -> Standing {
    match locked.secure {
        Some(Standing::Banned) | Some(Standing::Stranger) => Standing::Stranger,
        Some(standing) => standing,
        None => Standing::CoOwner,
    }
}

fn excluded(state: &WorldState, item: EntityId) -> bool {
    state.registry.has::<TradeWindow>(item) || state.registry.has::<CorpseBody>(item)
}

fn descends_from(state: &WorldState, mut item: EntityId, root: EntityId) -> bool {
    let mut visited = HashSet::new();
    while visited.insert(item) {
        if item == root {
            return true;
        }
        if excluded(state, item) || state.registry.has::<LockedDown>(item) {
            return false;
        }
        let Some(location) = state.registry.get::<ItemLocation>(item).copied() else {
            return false;
        };
        let SettledItemLocation::Contained(contained) = location.origin() else {
            return false;
        };
        let Some(parent) = state.registry.entity_of(contained.container) else {
            return false;
        };
        item = parent;
    }
    false
}
