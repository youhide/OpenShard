use super::*;

/// Whether two items are one pile waiting to happen: both stackable, same
/// graphic and hue, and not the same entity.
pub fn can_stack(state: &WorldState, a: EntityId, b: EntityId) -> bool {
    a != b
        && state.registry.has::<Stackable>(a)
        && state.registry.has::<Stackable>(b)
        && state.registry.get::<Graphic>(a) == state.registry.get::<Graphic>(b)
}

/// Merge a held stack onto another stack, on the ground or inside a container.
/// See `can_stack`.
pub fn merge_onto(
    state: &mut WorldState,
    connection: ConnectionId,
    held: HeldItem,
    target: EntityId,
) {
    let Some(&player) = state.players.get(&connection) else {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    };

    // Where the target lives decides how it is reached and redrawn. On the
    // ground it is reach-checked against the player's tile and redrawn with a
    // `0x1A`; inside a container it is reach-checked through its container and
    // redrawn with a `0x25` to every open gump, as `give` does.
    if let Some(&Position(target_pos)) = state.registry.get::<Position>(target) {
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
        let total = merge_amounts(state, held.entity, target);
        state.held.remove(&connection);
        state.registry.despawn(held.entity);
        redraw_ground_item(state, target);
        debug!(total, "stacks merged");
    } else if let Some(&contained) = state.registry.get::<Contained>(target) {
        let container = contained.container;
        let reachable = state
            .registry
            .entity_of(container)
            .is_some_and(|c| container_in_reach(state, c, player));
        if !reachable {
            bounce(state, connection, held, DragCancelReason::OutOfRange);
            return;
        }
        let total = merge_amounts(state, held.entity, target);
        state.held.remove(&connection);
        // The dragged stack was on a cursor, on no screen and in no gump, so
        // despawning it needs no packet of its own.
        state.registry.despawn(held.entity);
        tell_watchers_updated(state, container, target);
        debug!(total, "stacks merged in a container");
    } else {
        // Worn, or nowhere placeable: nothing to merge onto.
        bounce(state, connection, held, DragCancelReason::Other);
    }
}

/// Fold the held stack's amount into the target's, clamped: a pile cannot count
/// past what its amount word can hold. Returns the target's new total.
fn merge_amounts(state: &mut WorldState, held: EntityId, target: EntityId) -> u16 {
    let total = amount_of(state, held).saturating_add(amount_of(state, target));
    set_stack_amount(state, target, total);
    total
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
