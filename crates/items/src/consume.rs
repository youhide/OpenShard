use super::*;

/// Remove an item from the world, wherever it lives — on the ground, inside a
/// container, or worn — sending the client update each case needs. The one-shot
/// primitive the pack's item triggers were missing: a drunk potion, a read-once
/// scroll, a scribed scroll consumed onto a book. `amount == 0` (or an `amount`
/// that covers the whole stack) removes the item entire; a smaller `amount`
/// decrements a stackable pile and leaves the rest.
///
/// Guarded like [`crate::add_loot`](super) callers: an unknown serial removes
/// nothing rather than erroring. Reach is *not* rechecked here — an
/// [`ItemUsed`](openshard_state) already cleared it server-side before a script
/// saw the item, and a script command is trusted input like `op_add_loot`.
/// Returns whether anything was consumed.
pub fn consume(state: &mut WorldState, serial: Serial, amount: u16) -> bool {
    let Some(entity) = state.registry.entity_of(serial) else {
        return false;
    };

    // A partial take only means something for a stackable pile; a smaller amount
    // asked of anything else is treated as a whole-item removal below.
    let have = amount_of(state, entity);
    let partial = amount != 0 && amount < have && state.registry.has::<Stackable>(entity);
    if partial {
        if let Some(&Contained { container, .. }) = state.registry.get::<Contained>(entity) {
            remove_from_stack(state, container, entity, amount);
            return true;
        }
        if state.registry.has::<Position>(entity) {
            set_stack_amount(state, entity, have - amount);
            redraw_ground_item(state, entity);
            return true;
        }
        // Worn or on a cursor — those never stack; fall through to whole removal.
    }

    // Whole-item removal, dispatched on where the item lives (the three location
    // components are mutually exclusive).
    if state.registry.has::<Position>(entity) {
        remove_ground_item(state, entity, serial);
    } else if let Some(&Contained { container, .. }) = state.registry.get::<Contained>(entity) {
        // A contained item is on no sector grid and no screen; the only client
        // that need hear are those with the container's gump open.
        tell_watchers_removed(state, container, serial);
        despawn_contents(state, serial);
        state.registry.despawn(entity);
    } else if let Some(&Equipped { mobile, .. }) = state.registry.get::<Equipped>(entity) {
        if let Some(wearer) = state.registry.entity_of(mobile) {
            broadcast_unequip(state, serial, wearer);
        }
        // A worn container someone had open is gone; forget it as `despawn_belongings` does.
        state.open_containers.remove(&serial);
        despawn_contents(state, serial);
        state.registry.despawn(entity);
    } else {
        // In limbo — held on a cursor, off every grid and screen; despawn is all.
        despawn_contents(state, serial);
        state.registry.despawn(entity);
    }
    true
}
