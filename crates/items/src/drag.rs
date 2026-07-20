use super::*;

/// How near, in tiles, a mobile must be to reach an item on the ground or set one
/// down. Sphere reaches two; a third forgives the diagonal the cursor is shown
/// on. Server-authoritative — the client's word is never taken.
pub(crate) const ITEM_REACH: u32 = 3;

/// Lift an item onto a client's cursor. See `Command::PickUpItem`.
pub fn pick_up(state: &mut WorldState, connection: ConnectionId, serial: u32, amount: u16) {
    let Some(&player) = state.players.get(&connection) else {
        return;
    };
    if state.held.contains_key(&connection) {
        reject_drag(state, connection, DragCancelReason::AlreadyHolding);
        return;
    }
    let Some(item_serial) = Serial::new(serial) else {
        reject_drag(state, connection, DragCancelReason::CannotLift);
        return;
    };
    let Some(item) = state.registry.entity_of(item_serial) else {
        reject_drag(state, connection, DragCancelReason::CannotLift);
        return;
    };
    // Only a thing with a graphic is an item. A mobile has none, so this
    // rejects trying to pick up a person.
    if !state.registry.has::<Graphic>(item) {
        reject_drag(state, connection, DragCancelReason::CannotLift);
        return;
    }
    // A town's fittings are not loot: script-placed decoration cannot be lifted.
    if state.registry.has::<Decoration>(item) {
        reject_drag(state, connection, DragCancelReason::CannotLift);
        return;
    }

    // Where it is now decides how it is lifted and where a cancelled drag
    // will put it back.
    if let Some(&Position(item_pos)) = state.registry.get::<Position>(item) {
        let Some(&Position(player_pos)) = state.registry.get::<Position>(player) else {
            return;
        };
        let facet = state.facet_of(item);
        if facet != state.facet_of(player) || !in_range(item_pos, player_pos, ITEM_REACH) {
            reject_drag(state, connection, DragCancelReason::OutOfRange);
            return;
        }
        // Taking part of a stack: leave the remainder behind as a new pile
        // and lift the original, now reduced to what was taken. The original
        // keeps its serial and goes to the cursor — the client's drag and its
        // eventual drop still name it — so only the leftover is a new object.
        let total = amount_of(state, item);
        if amount > 0 && amount < total && state.registry.has::<Stackable>(item) {
            spawn_leftover(state, item, total - amount, item_pos, facet);
            set_stack_amount(state, item, amount);
        }
        // Off the sector grid, off every screen but the picker's — whose own
        // client already put it on the cursor, so a 0x1D there would fight it.
        state.facet_state_mut(facet).sectors.remove(item);
        for watcher in state.watchers_of(item) {
            if watcher == player {
                if let Some(seen) = state.seen.get_mut(&player) {
                    seen.remove(&item);
                }
            } else {
                state.forget(watcher, item, item_serial);
            }
        }
        state.registry.remove::<Position>(item);
        // Off the ground, off the decay clock.
        state.registry.remove::<Decays>(item);
        state.held.insert(
            connection,
            HeldItem {
                entity: item,
                origin: Origin::Ground {
                    position: item_pos,
                    facet,
                },
            },
        );
    } else if let Some(&contained) = state.registry.get::<Contained>(item) {
        // Out of a container. The client with the gump open removes it from
        // the gump itself; the server just drops the containment.
        state.registry.remove::<Contained>(item);
        state.held.insert(
            connection,
            HeldItem {
                entity: item,
                origin: Origin::Container(contained),
            },
        );
    } else if let Some(&worn) = state.registry.get::<Equipped>(item) {
        // Off a mobile. The picker's own client drags it off the paperdoll;
        // everyone else watching the mobile is told to forget it, because
        // they knew it only as part of that mobile.
        state.registry.remove::<Equipped>(item);
        if let Some(mobile) = state.registry.entity_of(worn.mobile) {
            for watcher in equip_audience(state, mobile) {
                if watcher == player {
                    continue;
                }
                if let Some(&Client { connection: to, .. }) = state.registry.get::<Client>(watcher)
                {
                    state.outbox.push(Outbound {
                        connection: to,
                        packet: encode_remove(item_serial.raw()),
                    });
                }
            }
        }
        state.held.insert(
            connection,
            HeldItem {
                entity: item,
                origin: Origin::Worn(worn),
            },
        );
    } else {
        // Neither on the ground nor in a container: already on a cursor, or
        // nowhere. Nothing to lift.
        reject_drag(state, connection, DragCancelReason::CannotLift);
        return;
    }
    debug!(%item_serial, "lifted onto the cursor");
}

/// Put a client's held item down. See `Command::DropItem`.
pub fn drop_item(
    state: &mut WorldState,
    connection: ConnectionId,
    serial: u32,
    position: Point,
    container: u32,
) {
    let Some(held) = state.held.get(&connection).copied() else {
        // Nothing on the cursor — a stray 0x08, nothing to bounce.
        return;
    };
    // The serial has to be the thing actually held; a mismatch is a confused
    // client, and the safe answer is to give it back what it was holding.
    if state.registry.serial_of(held.entity) != Serial::new(serial) {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    }

    if container != DROP_TO_GROUND {
        drop_onto_item(state, connection, held, position, container);
        return;
    }

    // Onto the ground: within reach of the player, on the player's facet.
    let Some(&player) = state.players.get(&connection) else {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    };
    let Some(&Position(player_pos)) = state.registry.get::<Position>(player) else {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    };
    if !in_range(position, player_pos, ITEM_REACH) {
        bounce(state, connection, held, DragCancelReason::OutOfRange);
        return;
    }

    state.held.remove(&connection);
    place_on_ground(state, held.entity, position, state.facet_of(player));
    debug!(serial, "dropped on the ground");
}

/// Put a held item into a container. See `Command::DropItem`.
pub fn drop_into_container(
    state: &mut WorldState,
    connection: ConnectionId,
    held: HeldItem,
    position: Point,
    container: u32,
) {
    let Some(container_serial) = Serial::new(container) else {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    };
    let Some(container_entity) = state.registry.entity_of(container_serial) else {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    };
    if !state.registry.has::<Container>(container_entity) {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    }
    let Some(&player) = state.players.get(&connection) else {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    };
    // The container must be in reach — on the ground near the player, or worn on
    // them (their backpack) or on a mobile beside them. A worn pack has no
    // `Position` of its own; `container_in_reach` handles that. Dropping into a
    // container nested in another is a later refinement.
    if !container_in_reach(state, container_entity, player) {
        bounce(state, connection, held, DragCancelReason::OutOfRange);
        return;
    }

    // In it goes. The drop's `x`/`y` are gump coordinates, not world tiles.
    let grid = item_count(state, container_serial);
    state.held.remove(&connection);
    state.registry.insert(
        held.entity,
        Contained {
            container: container_serial,
            x: position.x,
            y: position.y,
            grid,
        },
    );
    // Tell the client, whose gump is open, that the item is now inside.
    if let (Some(&Client { version, .. }), Some(record)) = (
        state.registry.get::<Client>(player),
        contained_record(state, held.entity),
    ) {
        state.send(
            connection,
            encode_add_to_container(record, container, version),
        );
    }
    debug!(container, "dropped into a container");
}

/// A drop onto another item: into it if it is a container, merged with it if
/// it is an identical stack, refused otherwise.
pub fn drop_onto_item(
    state: &mut WorldState,
    connection: ConnectionId,
    held: HeldItem,
    position: Point,
    target_serial: u32,
) {
    let target = Serial::new(target_serial).and_then(|s| state.registry.entity_of(s));
    match target {
        Some(target) if state.registry.has::<Spellbook>(target) => {
            drop_scroll_on_book(state, connection, held, target);
        }
        Some(target) if state.registry.has::<Container>(target) => {
            drop_into_container(state, connection, held, position, target_serial);
        }
        Some(target) if can_stack(state, held.entity, target) => {
            merge_onto(state, connection, held, target);
        }
        _ => bounce(state, connection, held, DragCancelReason::Other),
    }
}

/// A Magery scroll dropped on a spellbook is learned into it and spent. A
/// non-scroll, a book out of reach, or a spell the book already holds bounces
/// back — no scroll is wasted on a spell you have.
fn drop_scroll_on_book(
    state: &mut WorldState,
    connection: ConnectionId,
    held: HeldItem,
    book: EntityId,
) {
    let spell = state
        .registry
        .get::<Graphic>(held.entity)
        .and_then(|g| scroll_spell(g.id));
    let (Some(spell), Some(&player), Some(book_serial)) = (
        spell,
        state.players.get(&connection),
        state.registry.serial_of(book),
    ) else {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    };
    if !crate::container_in_reach(state, book, player) {
        bounce(state, connection, held, DragCancelReason::OutOfRange);
        return;
    }
    let mut mask = state
        .registry
        .get::<Spellbook>(book)
        .copied()
        .unwrap_or_default();
    if mask.has(spell) {
        // Already in the book — keep the scroll rather than burn it for nothing.
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    }
    mask.learn(spell);
    state.registry.insert(book, mask);
    state.held.remove(&connection);
    state.registry.despawn(held.entity);
    // Refresh the open book so the new spell appears at once.
    state.send(
        connection,
        encode_spellbook_content(book_serial.raw(), SPELLBOOK_GRAPHIC, 1, mask.0),
    );
    debug!(spell, "learned a spell from a scroll");
}

/// Put a held item back where it was lifted and tell the client the drag is
/// off, so it stops showing the item on the cursor.
pub fn bounce(
    state: &mut WorldState,
    connection: ConnectionId,
    held: HeldItem,
    reason: DragCancelReason,
) {
    state.held.remove(&connection);
    restore(state, held);
    reject_drag(state, connection, reason);
}

/// Put a held item back exactly where it came from — the ground it lay on or
/// the container it was in.
pub fn restore(state: &mut WorldState, held: HeldItem) {
    match held.origin {
        Origin::Ground { position, facet } => {
            place_on_ground(state, held.entity, position, facet);
        }
        Origin::Container(contained) => {
            state.registry.insert(held.entity, contained);
        }
        Origin::Worn(worn) => {
            state.registry.insert(held.entity, worn);
            // Back on the mobile, and back on every screen that shows it.
            if let Some(mobile) = state.registry.entity_of(worn.mobile) {
                broadcast_equip(state, held.entity, mobile);
            }
        }
    }
}

/// Send a `0x27`, cancelling whatever drag the client thinks it has.
pub fn reject_drag(state: &mut WorldState, connection: ConnectionId, reason: DragCancelReason) {
    state.send(connection, encode_drag_cancel(reason));
}
