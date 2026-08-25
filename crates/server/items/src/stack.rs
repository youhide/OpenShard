use super::*;

/// ServUO's three coin clinks, chosen by the pile that was set down.
const GOLD_DROP_SOUNDS: [SoundId; 3] = [SoundId(0x02E4), SoundId(0x02E5), SoundId(0x02E6)];

/// The sound an item makes when it lands, or the destination's ordinary sound
/// when the item has no sound of its own.
///
/// Gold is the one classic item whose sound depends on its amount: one coin,
/// a handful, and a pile each ring differently.  This is ServUO's
/// `Gold.GetDropSound`; carrying the rule beside stack merging prevents the
/// backpack, ground and container paths from choosing different coin sounds.
pub fn drop_sound(graphic: Graphic, amount: u16, fallback: SoundId) -> SoundId {
    if graphic != GOLD_GRAPHIC {
        return fallback;
    }
    match amount {
        0 | 1 => GOLD_DROP_SOUNDS[0],
        2..=5 => GOLD_DROP_SOUNDS[1],
        _ => GOLD_DROP_SOUNDS[2],
    }
}

/// Whether two items are one pile waiting to happen: both stackable, same
/// graphic and hue, and not the same entity.
pub fn can_stack(state: &WorldState, a: EntityId, b: EntityId) -> bool {
    let same_drawn = state.registry.get::<Drawn>(a) == state.registry.get::<Drawn>(b);
    a != b
        && same_drawn
        && (state.registry.has::<Stackable>(a) && state.registry.has::<Stackable>(b)
            // Older saves can contain one-coin gold items made before gold was
            // marked stackable at creation.  Keep those coins usable too.
            || state.registry.get::<Drawn>(a).is_some_and(|drawn| drawn.id == GOLD_GRAPHIC))
}

/// Merge a held stack onto another stack, on the ground or inside a container.
/// See `can_stack`.
pub fn merge_onto(state: &mut WorldState, connection: ConnectionId, held: HeldItem, target: EntityId) {
    let Some(&player) = state.players.get(&connection) else {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    };

    // Where the target lives decides how it is reached and redrawn. On the
    // ground it is reach-checked against the player's tile and redrawn with a
    // `0x1A`; inside a container it is reach-checked through its container and
    // redrawn with a `0x25` to every open gump, as `give` does.
    let target_location = item_location(state, target);
    if let Some(ItemLocation::Settled(SettledItemLocation::Ground {
        facet,
        position: target_pos,
    })) = target_location
    {
        let Some(&Position(player_pos)) = state.registry.get::<Position>(player) else {
            bounce(state, connection, held, DragCancelReason::Other);
            return;
        };
        if facet != state.facet_of(player) || !in_range(target_pos, player_pos, ITEM_REACH) {
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
        despawn_item(state, held.entity);
        let sound = drop_sound(
            state.registry.get::<Drawn>(target).expect("a stack has art").id,
            amount_of(state, target),
            SoundId(0x0042),
        );
        state.play_sound_to(player, sound);
        debug!("stacks merged");
    } else if let Some(ItemLocation::Settled(SettledItemLocation::Contained(contained))) = target_location {
        let container = contained.container;
        let reachable = state
            .registry
            .entity_of(container)
            .is_some_and(|c| in_reach(state, c, player));
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
        // The dragged stack was on a cursor, on no screen and in no gump, so
        // despawning it needs no packet of its own.
        despawn_item(state, held.entity);
        let sound = drop_sound(
            state.registry.get::<Drawn>(target).expect("a stack has art").id,
            amount_of(state, target),
            SoundId(0x0048),
        );
        state.play_sound_to(player, sound);
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
    // Normalise an old one-coin gold item as soon as it participates in a
    // merge, so its corrected state is retained by the next save.
    if state
        .registry
        .get::<Drawn>(target)
        .is_some_and(|drawn| drawn.id == GOLD_GRAPHIC)
    {
        state.registry.insert(target, Stackable);
    }
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
        let Some((connection, version)) = state.client_of(watcher) else {
            continue;
        };
        if let Some(packet) = state.draw_packet(watcher, item, version) {
            state.outbox.push(Outbound { connection, packet });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gold_uses_the_classic_amount_sensitive_clinks() {
        let fallback = SoundId(0x0048);
        assert_eq!(drop_sound(GOLD_GRAPHIC, 1, fallback), SoundId(0x02E4));
        assert_eq!(drop_sound(GOLD_GRAPHIC, 5, fallback), SoundId(0x02E5));
        assert_eq!(drop_sound(GOLD_GRAPHIC, 6, fallback), SoundId(0x02E6));
        assert_eq!(drop_sound(Graphic(0x0F5E), 1, fallback), fallback);
    }
}
