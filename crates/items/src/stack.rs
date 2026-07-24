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
        let left = merge_amounts(state, held.entity, target);
        redraw_ground_item(state, target);
        if left > 0 {
            // The target filled up. What did not fit goes back where it came
            // from rather than onto the floor under the pile — the drop is
            // refused, in Sphere's sense of "the item did not all stack, do
            // something else with it", and the player keeps every coin.
            debug!(left, "stack filled; the remainder bounced");
            bounce(state, connection, held, DragCancelReason::Other);
            return;
        }
        state.held.remove(&connection);
        state.registry.despawn(held.entity);
        debug!("stacks merged");
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
        let left = merge_amounts(state, held.entity, target);
        tell_watchers_updated(state, container, target);
        if left > 0 {
            debug!(left, "stack filled in a container; the remainder bounced");
            bounce(state, connection, held, DragCancelReason::Other);
            return;
        }
        state.held.remove(&connection);
        // The dragged stack was on a cursor, on no screen and in no gump, so
        // despawning it needs no packet of its own.
        state.registry.despawn(held.entity);
        debug!("stacks merged in a container");
    } else {
        // Worn, or nowhere placeable: nothing to merge onto.
        bounce(state, connection, held, DragCancelReason::Other);
    }
}

/// The most one pile may hold — ServUO's `Item.WillStack` cap.
///
/// Below the `u16` an [`Amount`] is stored in and the wire carries, on purpose:
/// the ceiling has to be a number the arithmetic can pass without wrapping, and
/// 60,000 is the one both this engine and the reference can name.
pub const MAX_STACK: u16 = 60_000;

/// Fold as much of the held stack into the target as the target can hold.
/// Returns what is **left on the held item** — zero when it all went in.
///
/// The references disagree here and Sphere has the better answer. ServUO refuses
/// the merge outright when the sum would pass its cap (`WillStack`), so a full
/// pile simply will not take a drop. Sphere's `CItem::Stack` fills the
/// destination to its maximum, leaves the remainder on the source, and reports
/// that it did not all fit — which loses nothing and needs no explanation to the
/// player. What must not happen is what happened here before: clamping the sum
/// and despawning the source, which quietly destroyed the difference. Dropping
/// 50,000 gold onto 50,000 left one pile of 65,535 and 34,465 gone.
fn merge_amounts(state: &mut WorldState, held: EntityId, target: EntityId) -> u16 {
    let held_amount = amount_of(state, held);
    let room = MAX_STACK.saturating_sub(amount_of(state, target));
    let moved = held_amount.min(room);
    set_stack_amount(state, target, amount_of(state, target) + moved);
    let left = held_amount - moved;
    set_stack_amount(state, held, left);
    left
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
