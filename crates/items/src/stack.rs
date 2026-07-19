use super::*;

/// Whether two items are one pile waiting to happen: both stackable, same
/// graphic and hue, and not the same entity.
pub fn can_stack(state: &WorldState, a: EntityId, b: EntityId) -> bool {
    a != b
        && state.registry.has::<Stackable>(a)
        && state.registry.has::<Stackable>(b)
        && state.registry.get::<Graphic>(a) == state.registry.get::<Graphic>(b)
}

/// Merge a held stack onto a stack on the ground. See `can_stack`.
pub fn merge_onto(
    state: &mut WorldState,
    connection: ConnectionId,
    held: HeldItem,
    target: EntityId,
) {
    // Only ground stacks merge for now; merging onto a stack inside a
    // container is a later refinement, and until then it bounces.
    let Some(&Position(target_pos)) = state.registry.get::<Position>(target) else {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    };
    let Some(&player) = state.players.get(&connection) else {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    };
    let Some(&Position(player_pos)) = state.registry.get::<Position>(player) else {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    };
    if state.facet_of(target) != state.facet_of(player)
        || !in_range(target_pos, player_pos, ITEM_REACH)
    {
        bounce(state, connection, held, DragCancelReason::OutOfRange);
        return;
    }

    // Sum, clamped: a pile cannot count past what its amount word can hold.
    let total = amount_of(state, held.entity).saturating_add(amount_of(state, target));
    set_stack_amount(state, target, total);
    state.held.remove(&connection);
    // The dragged stack is gone into the other; it was on a cursor, on
    // nobody's ground, so despawning it needs no packet.
    state.registry.despawn(held.entity);
    redraw_ground_item(state, target);
    debug!(total, "stacks merged");
}

/// How many an item is: its [`Amount`], or one if it has none.
pub fn amount_of(state: &WorldState, item: EntityId) -> u16 {
    state.registry.get::<Amount>(item).map_or(1, |a| a.0)
}

/// Set a stack's size, keeping the "a single carries no `Amount`" rule that
/// `spawn_item` and the `0x1A` encoder both rely on.
pub fn set_stack_amount(state: &mut WorldState, item: EntityId, amount: u16) {
    if amount > 1 {
        state.registry.insert(item, Amount(amount));
    } else {
        state.registry.remove::<Amount>(item);
    }
}

/// Re-send a ground item to everyone already watching it — for when its
/// amount changed and the `seen` set would otherwise suppress the redraw.
pub fn redraw_ground_item(state: &mut WorldState, item: EntityId) {
    for watcher in state.watchers_of(item) {
        let Some(&Client {
            connection,
            version,
        }) = state.registry.get::<Client>(watcher)
        else {
            continue;
        };
        if let Some(packet) = state.draw_packet(item, version) {
            state.outbox.push(Outbound { connection, packet });
        }
    }
}
