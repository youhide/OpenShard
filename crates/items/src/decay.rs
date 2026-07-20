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
        let facet = state.facet_of(item);
        for watcher in state.watchers_of(item) {
            state.forget(watcher, item, serial);
        }
        // A decaying container takes its contents with it — a corpse rots away
        // with whatever loot was never lifted, classic UO. Without this the loot
        // would outlive the corpse as orphaned items pointing at a gone serial.
        despawn_contents(state, serial);
        state.facet_state_mut(facet).sectors.remove(item);
        state.registry.despawn(item);
        debug!(%serial, "decayed");
    }
}

/// Despawn everything directly inside `container`, and recursively inside any
/// container among them. Used when a decaying container rots away.
fn despawn_contents(state: &mut WorldState, container: Serial) {
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
