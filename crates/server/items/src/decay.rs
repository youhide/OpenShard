use super::*;

/// Set an item's decay clock: it rots `gameplay.decay_ticks` from now. Every
/// loose item on the ground has one; every item off it has none, and so does a
/// container — it and its contents stay put until someone moves them, which is
/// also why a container picked up and set back down does not start rotting.
pub fn mark_decay(state: &mut WorldState, item: EntityId) {
    // Zero is the operator's explicit "keep everything" setting.  Do this
    // before attaching a clock: an item created while cleanup is off must stay
    // clock-free if cleanup is enabled again later.
    if state.gameplay.decay_ticks == 0 {
        return;
    }
    if state.registry.has::<Container>(item) {
        return;
    }
    // And nothing a house is holding. A lockdown is the player saying "leave
    // this where it is", and rotting it twenty minutes later is the opposite of
    // that.
    if state
        .registry
        .has::<openshard_state::components::LockedDown>(item)
    {
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
    // Also leave clocks that pre-date a configuration change alone.  This
    // makes disabling decay immediate for a running world and covers corpses,
    // whose clock is installed directly by the death system.
    if state.gameplay.decay_ticks == 0 {
        return;
    }
    let now = state.ticks;
    let expired: Vec<EntityId> = state
        .registry
        .query::<Decays>()
        .filter(|(_, decays)| decays.at_tick <= now)
        .map(|(entity, _)| entity)
        // The belt to `mark_decay`'s braces, and the case that needs it: an item
        // is dropped loose (which marks it) and locked down *after*, so the clock
        // is already running when the pin arrives.
        .filter(|&entity| {
            !state
                .registry
                .has::<openshard_state::components::LockedDown>(entity)
        })
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
    state.unplace(facet, item);
    despawn_item(state, item);
}

/// Despawn everything directly inside `container`, and recursively inside any
/// container among them. Used when a decaying or consumed container rots away.
pub(crate) fn despawn_contents(state: &mut WorldState, container: Serial) {
    let contained: Vec<EntityId> = contained_items(state, container)
        .map(|(entity, _)| entity)
        .collect();
    for entity in contained {
        if let Some(serial) = state.registry.serial_of(entity) {
            despawn_contents(state, serial);
            despawn_item(state, entity);
        }
    }
}
