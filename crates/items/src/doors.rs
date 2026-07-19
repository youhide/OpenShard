use super::*;

/// How long a door stays open before it swings shut on its own, in ticks —
/// roughly the classic client's self-closing delay.
pub(crate) const DOOR_OPEN_TICKS: u64 = 20 * TICKS_PER_SECOND;

/// Open or close a door the player double-clicked, if it is in reach.
///
/// The toggle is the whole mechanic: swap the graphic between the door's shut and
/// open art, and hop it one tile by its hinge offset (and back when it shuts), so
/// the client draws the leaf swinging aside. Opening also arms the auto-close
/// tick; closing disarms it.
pub fn toggle_door(state: &mut WorldState, player: EntityId, door: EntityId, serial: Serial) {
    let Some(&Position(at)) = state.registry.get::<Position>(door) else {
        return;
    };
    let Some(&Position(player_pos)) = state.registry.get::<Position>(player) else {
        return;
    };
    if state.facet_of(door) != state.facet_of(player) || !in_range(at, player_pos, ITEM_REACH) {
        return;
    }
    let Some(is_open) = state.registry.get::<Door>(door).map(|d| d.is_open) else {
        return;
    };
    set_door(state, door, serial, !is_open);
}

/// Put a door into the open or closed state, redrawing it for everyone who can see
/// it. Shared by the double-click toggle and the tick's auto-close; neither
/// checks reach here — the caller does when a player is involved.
///
/// The move is pushed to every watcher as a fresh `0x1A` after a forget, because
/// the door both changed graphic and changed tile, and a client only redraws what
/// it was told to forget.
pub(crate) fn set_door(state: &mut WorldState, door: EntityId, serial: Serial, open: bool) {
    let Some(&Position(at)) = state.registry.get::<Position>(door) else {
        return;
    };
    let Some(&Door {
        closed,
        open: open_id,
        offset_x,
        offset_y,
        is_open,
        ..
    }) = state.registry.get::<Door>(door)
    else {
        return;
    };
    if is_open == open {
        return; // already there — nothing to redraw
    }

    // Opening hops the leaf aside by its offset; closing hops it back. The x/y are
    // world tiles and the offset is a small signed step, so saturate at the edges
    // rather than wrap.
    let (graphic, moved, close_at) = if open {
        (
            open_id,
            shift(at, offset_x, offset_y),
            state.ticks + DOOR_OPEN_TICKS,
        )
    } else {
        (closed, shift(at, -offset_x, -offset_y), 0)
    };

    for watcher in state.watchers_of(door) {
        state.forget(watcher, door, serial);
    }
    let facet = state.facet_of(door);
    state.registry.insert(
        door,
        Graphic {
            id: graphic,
            hue: 0,
        },
    );
    state.registry.insert(door, Position(moved));
    state.registry.insert(
        door,
        Door {
            closed,
            open: open_id,
            offset_x,
            offset_y,
            is_open: open,
            close_at,
        },
    );
    state.facet_state_mut(facet).sectors.insert(door, moved);
    // The tile the shut leaf fills is blocked; opening frees it. This is the
    // line that makes a door real to movement — see `state::obstruct`.
    if open {
        state
            .facet_state_mut(facet)
            .obstructions
            .unblock(at.x, at.y, door);
    } else {
        state.facet_state_mut(facet).obstructions.block(
            moved.x,
            moved.y,
            door,
            true,
            moved.z,
            openshard_state::DOOR_HEIGHT,
        );
    }
    state.reveal(door);
}

/// Open a shut door by decree — what an NPC that knows door handles does when
/// one stands in its way. No reach check: the caller is the AI, standing at the
/// threshold, not a client to be doubted. Opening arms the same auto-close as a
/// double-click, so the door swings shut behind the walker.
pub fn open_door(state: &mut WorldState, door: EntityId) {
    let Some(serial) = state.registry.serial_of(door) else {
        return;
    };
    set_door(state, door, serial, true);
}

/// Swing shut every door whose auto-close tick has arrived. Driven by the tick
/// counter, like decay, so a door closes on the same tick in a replay. See
/// [`Door`].
pub fn close_doors(state: &mut WorldState) {
    let now = state.ticks;
    let due: Vec<(EntityId, Serial)> = state
        .registry
        .query::<Door>()
        .filter(|(_, door)| door.is_open && door.close_at != 0 && door.close_at <= now)
        .filter_map(|(entity, _)| state.registry.serial_of(entity).map(|s| (entity, s)))
        .collect();
    for (door, serial) in due {
        set_door(state, door, serial, false);
    }
}

/// A tile stepped by a small signed offset, clamped at the map edge.
pub(crate) fn shift(at: Point, dx: i16, dy: i16) -> Point {
    let x = (i32::from(at.x) + i32::from(dx)).clamp(0, i32::from(u16::MAX)) as u16;
    let y = (i32::from(at.y) + i32::from(dy)).clamp(0, i32::from(u16::MAX)) as u16;
    Point::new(x, y, at.z)
}
