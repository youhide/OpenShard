use super::*;

/// Set an item's decay clock: it rots `gameplay.decay_ticks` from now. Every
/// loose item on the ground has one; every item off it has none, and so does a
/// container — it and its contents stay put until someone moves them, which is
/// also why a container picked up and set back down does not start rotting.
pub fn mark_decay(state: &mut WorldState, item: EntityId) {
    if state.registry.has::<Container>(item) {
        return;
    }
    state.registry.insert(
        item,
        Decays {
            at_tick: state.ticks + state.gameplay.decay_ticks,
        },
    );
}

/// Remove every ground item whose decay tick has arrived. Runs each tick,
/// against `ticks`, so it reads no clock.
pub fn decay(state: &mut WorldState) {
    let now = state.ticks;
    let expired: Vec<EntityId> = state
        .registry
        .query::<Decays>()
        .filter(|(_, decays)| decays.at_tick <= now)
        .map(|(entity, _)| entity)
        .collect();
    for item in expired {
        let Some(serial) = state.registry.serial_of(item) else {
            continue;
        };
        remove_ground_item(state, item, serial);
        debug!(%serial, "decayed");
    }
}

/// Take a ground item off every screen that has it (`0x1D`), off the sector grid,
/// and out of the registry — cascading into its contents if it is a container.
/// The shared tail of [`decay`] and [`consume`](crate::consume): a decaying
/// container takes its loot with it (classic UO), and so does a consumed one,
/// rather than leaving orphans pointing at a gone serial.
pub(crate) fn remove_ground_item(state: &mut WorldState, item: EntityId, serial: Serial) {
    let facet = state.facet_of(item);
    for watcher in state.watchers_of(item) {
        state.forget(watcher, item, serial);
    }
    despawn_contents(state, serial);
    state.facet_state_mut(facet).sectors.remove(item);
    state.registry.despawn(item);
}

/// Despawn everything directly inside `container`, and recursively inside any
/// container among them. Used when a decaying or consumed container rots away.
pub(crate) fn despawn_contents(state: &mut WorldState, container: Serial) {
    let contained: Vec<EntityId> = state
        .registry
        .query::<Contained>()
        .filter(|(_, c)| c.container == container)
        .map(|(entity, _)| entity)
        .collect();
    for entity in contained {
        if let Some(serial) = state.registry.serial_of(entity) {
            despawn_contents(state, serial);
            state.registry.despawn(entity);
        }
    }
}
