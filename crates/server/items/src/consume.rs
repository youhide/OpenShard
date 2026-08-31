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
/// saw the item, and a queued command is trusted input like `Command::AddLoot`.
/// Returns whether anything was consumed.
pub fn consume(state: &mut WorldState, serial: Serial, amount: u16) -> bool {
    let Some(entity) = state.registry.entity_of(serial) else {
        return false;
    };

    // A partial take only means something for a stackable pile; a smaller amount
    // asked of anything else is treated as a whole-item removal below.
    let have = amount_of(state, entity);
    let partial = amount != 0 && amount < have && state.registry.has::<Stackable>(entity);
    let location = item_location(state, entity);
    if partial {
        if let Some(ItemLocation::Settled(SettledItemLocation::Contained(Contained { container, .. }))) =
            location
        {
            remove_from_stack(state, container, entity, amount);
            return true;
        }
        if matches!(
            location,
            Some(ItemLocation::Settled(SettledItemLocation::Ground { .. }))
        ) {
            set_stack_amount(state, entity, have - amount);
            return true;
        }
        // Worn or on a cursor — those never stack; fall through to whole removal.
    }

    // A script may name an item while its ordinary drag is in flight. The item
    // is allowed to be consumed, but the cursor is part of the same state
    // transition: leaving `Connection::held` pointing at the despawned entity
    // makes every later lift fail with AlreadyHolding forever.
    let holders: Vec<ConnectionId> = state
        .connections
        .iter()
        .filter_map(|(&connection, row)| {
            row.held
                .is_some_and(|held| held.entity == entity)
                .then_some(connection)
        })
        .collect();
    for connection in holders {
        reject_drag(state, connection, DragCancelReason::Other);
        // The lifter's authoritative view still projects the item at its
        // origin: a successful lift intentionally echoed no Remove to that
        // same client. DragCancel normally means "restore that projection",
        // so consumption also has to say that the serial itself is gone.
        state.send_packet(connection, &ServerPacket::Remove(Remove { serial }));
    }

    // Whole-item removal, dispatched on where the item lives (the three location
    // components are mutually exclusive).
    match location {
        Some(ItemLocation::Settled(SettledItemLocation::Ground { .. })) => {
            remove_ground_item(state, entity, serial);
        }
        Some(ItemLocation::Settled(SettledItemLocation::Contained(Contained { container, .. }))) => {
            // A contained item is on no sector grid and no screen; the only client
            // that need hear are those with the container's gump open.
            tell_watchers_removed(state, container, serial);
            despawn_contents(state, serial);
            despawn_item(state, entity);
        }
        Some(ItemLocation::Settled(SettledItemLocation::Equipped(Equipped { mobile, .. }))) => {
            if let Some(wearer) = state.registry.entity_of(mobile) {
                broadcast_unequip(state, serial, wearer);
            }
            // A worn container someone had open is gone; forget it as `despawn_belongings` does.
            state.open_containers.remove(&serial);
            despawn_contents(state, serial);
            despawn_item(state, entity);
        }
        Some(ItemLocation::Held { .. }) | None => {
            // In limbo — held on a cursor, off every grid and screen; despawn is all.
            despawn_contents(state, serial);
            despawn_item(state, entity);
        }
    }
    true
}
