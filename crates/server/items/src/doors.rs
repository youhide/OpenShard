use super::*;
use openshard_protocol::wire::Hue;
use openshard_state::components::{House, HouseDoor};
use openshard_state::{KeyValue, Lock, LockKind, Standing, WorldTick};

/// How long a door stays open before it swings shut on its own, in ticks —
/// roughly the classic client's self-closing delay.
pub(crate) const DOOR_OPEN_TICKS: u64 = 20 * TICKS_PER_SECOND;

/// The creak a wooden door makes swinging open, and the thud shutting — ServUO's
/// `DarkWoodHouseDoor` sounds, the sensible default for the town and shop doors
/// the pack lays down. A metal or barred door would sound different, but the
/// engine's `Door` is a generic toggle with no material, so wooden it is until a
/// door carries its own sound.
const DOOR_OPEN_SOUND: SoundId = SoundId(0x00EA);
const DOOR_CLOSE_SOUND: SoundId = SoundId(0x00F1);

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
    // A house door opens for the people the house trusts. Asked before the lock,
    // because a stranger at a friend's door is refused for *being* a stranger and
    // saying "that is locked" would send them looking for a key.
    //
    // The question is `House::standing_of`, which lives on the component in
    // `state` for exactly this: `items` has no business depending on the housing
    // crate, and a door is not a housing concept. See `Standing`'s own docs.
    if !is_open && !may_pass(state, player, door) {
        state.system_message(player, NOT_YOUR_DOOR);
        return;
    }
    // A locked door refuses to be opened, and says so. Only while *shut*: ServUO
    // checks `m_Locked && !m_Open`, so a door left standing open can always be pushed
    // closed again — otherwise a lock that snapped on while the door was open would
    // wedge it open for ever.
    if !is_open && is_locked(state, door) && !state.is_staff(player) {
        state.system_message(player, LOCKED_MESSAGE);
        return;
    }
    set_door_pair(state, door, serial, !is_open);
}

/// What a house door says to somebody it does not know. ServUO's own line for a
/// house they have no access to.
pub const NOT_YOUR_DOOR: &str = "You cannot open that door.";

/// ServUO cliloc 1019048, what anything a dead player double-clicks answers
/// with (`Item.OnDoubleClickDead`).
pub const DEAD_HANDS: &str = "I am dead and cannot do that.";

/// Whether `player` may work this door, which is only a question for a house's.
///
/// A door with no [`HouseDoor`] belongs to Britannia and opens for anyone — every
/// door in the world is one of those today.
fn may_pass(state: &WorldState, player: EntityId, door: EntityId) -> bool {
    let Some(&HouseDoor { house }) = state.registry.get::<HouseDoor>(door) else {
        return true;
    };
    let Some(house) = state
        .registry
        .entity_of(house)
        .and_then(|entity| state.registry.get::<House>(entity))
    else {
        // The house is gone and its door outlived it. Opens for anyone, which is
        // the harmless direction: the alternative is a door nobody can ever move
        // standing in the middle of a field.
        return true;
    };
    let Some(who) = state.registry.serial_of(player) else {
        return false;
    };
    house.standing_of(who, state.is_staff(player)) >= Standing::Friend
}

/// ServUO cliloc 502503, what a player is told by a door that will not open.
pub const LOCKED_MESSAGE: &str = "That is locked.";

/// A key was used: raise a target cursor for it — ServUO's `Key.OnDoubleClick`.
///
/// A cursor and not a guess. Several doors stand within arm's reach of each other in
/// most of Britannia's shops, and picking one for the player is how a key locks the
/// wrong door.
pub fn use_key(state: &mut WorldState, connection: ConnectionId, player: EntityId, key: EntityId) {
    if !state.registry.has::<KeyValue>(key) {
        return;
    }
    state.raise_target(player, openshard_state::TargetPurpose::TurnKey { key });
    state.send_packet(
        connection,
        &ServerPacket::TargetCursor(TargetCursor {
            cursor_id: CursorId(0),
            kind: TargetKind::Location,
        }),
    );
}

/// Whether a player is close enough to turn a key on something.
#[must_use]
pub fn in_key_reach(state: &WorldState, player: EntityId, target: EntityId) -> bool {
    let (Some(&Position(at)), Some(&Position(player_at))) = (
        state.registry.get::<Position>(target),
        state.registry.get::<Position>(player),
    ) else {
        // Not on the ground: a chest in a pack is reached through its container.
        return crate::in_reach(state, target, player);
    };
    state.facet_of(target) == state.facet_of(player) && in_range(at, player_at, ITEM_REACH)
}

/// Whether this door or container is locked.
#[must_use]
pub fn is_locked(state: &WorldState, entity: EntityId) -> bool {
    state.registry.has::<Lock>(entity)
}

/// Turn a key on a lock. Returns whether it fitted.
///
/// ServUO's `Key.OnTarget`: the *value* is what matches, not the item, so a copied key
/// works and a key to another door does not. A fitting key toggles the lock, which is
/// how ServUO's keys both lock and unlock.
pub fn turn_key(state: &mut WorldState, player: EntityId, key: EntityId, target: EntityId) -> bool {
    let Some(&value) = state.registry.get::<KeyValue>(key) else {
        return false;
    };
    // A door or a container is lockable; nothing else is.
    if !state.registry.has::<Door>(target) && !state.registry.has::<Container>(target) {
        return false;
    }
    match state.registry.get::<Lock>(target).copied() {
        // Locked, and this key fits: unlock it.
        Some(Lock {
            kind: LockKind::Key(key_value),
            ..
        }) if key_value == value => {
            state.registry.remove::<Lock>(target);
            state.system_message(player, "You unlock it.");
            true
        }
        // Locked by something else. ServUO says nothing useful here either.
        Some(_) => {
            state.system_message(player, "The key does not fit.");
            false
        }
        // Unlocked: the same key locks it again.
        None => {
            state.registry.insert(
                target,
                Lock {
                    kind: LockKind::Key(value),
                    required_skill: 0,
                    max_skill: 0,
                },
            );
            state.system_message(player, "You lock it.");
            true
        }
    }
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
        link,
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
        (closed, shift(at, -offset_x, -offset_y), WorldTick::ZERO)
    };

    for watcher in state.watchers_of(door) {
        state.forget(watcher, door, serial);
    }
    let facet = state.facet_of(door);
    state.registry.insert(
        door,
        Drawn {
            id: graphic,
            hue: Hue(0),
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
            link,
            is_open: open,
            close_at,
        },
    );
    state.place_item(facet, door, moved);
    // The creak or thud, heard by everyone who can see the door — the same
    // audience the redraw goes to. A door that swings in silence reads as broken.
    state.play_sound(door, if open { DOOR_OPEN_SOUND } else { DOOR_CLOSE_SOUND });
    // The tile the shut leaf fills is blocked; opening frees it. This is the
    // line that makes a door real to movement — see `state::obstruct`.
    if open {
        state.facet_state_mut(facet).unblock(at.x, at.y, door);
    } else {
        state.facet_state_mut(facet).block(
            moved.x,
            moved.y,
            door,
            openshard_map::overlay::Cover::door(moved.z, openshard_state::DOOR_HEIGHT),
        );
    }
    state.reveal(door);
}

/// Put one leaf and its linked twin into the same state. The link is a stable
/// serial because generated decoration survives a restart; a missing or stale
/// target degrades to an ordinary single door.
fn set_door_pair(state: &mut WorldState, door: EntityId, serial: Serial, open: bool) {
    let linked = linked_door(state, door);
    set_door(state, door, serial, open);
    if let Some((other, other_serial)) = linked {
        set_door(state, other, other_serial, open);
    }
}

/// Resolve the other leaf without following the reciprocal link. A corrupt
/// self-link is ignored, and a serial whose entity is gone is simply stale.
fn linked_door(state: &WorldState, door: EntityId) -> Option<(EntityId, Serial)> {
    let serial = state.registry.get::<Door>(door)?.link?;
    let other = state.registry.entity_of(serial)?;
    (other != door && state.registry.has::<Door>(other)).then_some((other, serial))
}

/// Whether an open leaf can return to its frame without closing over a body.
/// The same predicate is applied to both halves before either half of a pair is
/// moved, which makes auto-close all-or-nothing.
fn free_to_close(state: &WorldState, door: EntityId) -> bool {
    let Some(&Position(at)) = state.registry.get::<Position>(door) else {
        return false;
    };
    let Some(component) = state.registry.get::<Door>(door) else {
        return false;
    };
    let closed_at = if component.is_open {
        shift(at, -component.offset_x, -component.offset_y)
    } else {
        at
    };
    state.body_standing_at(state.facet_of(door), closed_at).is_none()
}

/// Open a shut door by decree — what an NPC that knows door handles does when
/// one stands in its way. No reach check: the caller is the AI, standing at the
/// threshold, not a client to be doubted. Opening arms the same auto-close as a
/// double-click, so the door swings shut behind the walker.
pub fn open_door(state: &mut WorldState, door: EntityId) {
    let Some(serial) = state.registry.serial_of(door) else {
        return;
    };
    // Hands are not a key. Without this a townsperson walking home strolls straight
    // through a locked shopfront, and the lock is decoration.
    if is_locked(state, door) {
        return;
    }
    set_door_pair(state, door, serial, true);
}

/// Swing shut every door whose auto-close tick has arrived. Driven by the tick
/// counter, like decay, so a door closes on the same tick in a replay. See
/// [`Door`].
pub fn close_doors(state: &mut WorldState) {
    let now = state.ticks;
    let due: Vec<(EntityId, Serial)> = state
        .registry
        .query::<Door>()
        .filter(|(_, door)| door.is_open && door.close_at != WorldTick::ZERO && door.close_at <= now)
        .filter_map(|(entity, _)| state.registry.serial_of(entity).map(|s| (entity, s)))
        .collect();
    for (door, serial) in due {
        // The pair may already have been closed when its other due row was
        // visited. Conversely, if either frame is occupied, neither leaf moves:
        // there is never half a doorway shut around the player in it.
        if !state.registry.get::<Door>(door).is_some_and(|door| door.is_open) {
            continue;
        }
        let linked = linked_door(state, door);
        if !free_to_close(state, door)
            || linked.is_some_and(|(other, _)| {
                state.registry.get::<Door>(other).is_some_and(|door| door.is_open)
                    && !free_to_close(state, other)
            })
        {
            continue;
        }
        set_door_pair(state, door, serial, false);
    }
}

/// A tile stepped by a small signed offset, clamped at the map edge.
pub(crate) fn shift(at: Point, dx: i16, dy: i16) -> Point {
    let x = (i32::from(at.x) + i32::from(dx)).clamp(0, i32::from(u16::MAX)) as u16;
    let y = (i32::from(at.y) + i32::from(dy)).clamp(0, i32::from(u16::MAX)) as u16;
    Point::new(x, y, at.z)
}
