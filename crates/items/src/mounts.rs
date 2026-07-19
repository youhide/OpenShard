use super::*;
use openshard_movement::{step_from, Terrain};

/// The layer a mount item rides on — the client draws whoever wears one as
/// mounted. `0x19`, the classic mount layer.
pub const MOUNT_LAYER: u8 = 0x19;

/// How close a rider must stand to swing up.
const MOUNT_REACH: u32 = 2;

/// Try to mount `target`: it must be a rideable body, riderless, no one's
/// client, and within arm's reach — and the player must be on foot. Returns
/// whether the double-click was a mounting, so the caller knows not to open a
/// paperdoll over it.
pub fn try_mount(
    state: &mut WorldState,
    player: EntityId,
    target: EntityId,
    target_serial: Serial,
) -> bool {
    let Some(&Body { id: body, hue }) = state.registry.get::<Body>(target) else {
        return false;
    };
    let Some(mount_graphic) = mount_item_for(body) else {
        return false;
    };
    if state.registry.has::<Client>(target)
        || state.registry.has::<Ridden>(target)
        || state.registry.has::<Riding>(player)
    {
        return false;
    }
    let (Some(&Position(at)), Some(&Position(player_at))) = (
        state.registry.get::<Position>(target),
        state.registry.get::<Position>(player),
    ) else {
        return false;
    };
    if state.facet_of(target) != state.facet_of(player) || !in_range(at, player_at, MOUNT_REACH) {
        return false;
    }
    let Some(rider_serial) = state.registry.serial_of(player) else {
        return false;
    };

    // The mount item first: if the serial pool is dry, nothing has happened yet.
    let Ok((item, _)) = state.registry.spawn_with_serial(SerialKind::Item) else {
        return false;
    };
    state.registry.insert(
        item,
        Graphic {
            id: mount_graphic,
            hue,
        },
    );
    state.registry.insert(
        item,
        Equipped {
            mobile: rider_serial,
            layer: MOUNT_LAYER,
        },
    );

    // The creature leaves the world: off every screen, off the sector grid,
    // without a position — the same limbo a lifted item sits in — until the
    // dismount puts it back.
    if let Some(serial) = state.registry.serial_of(target) {
        for watcher in state.watchers_of(target) {
            state.forget(watcher, target, serial);
        }
    }
    let facet = state.facet_of(target);
    state.facet_state_mut(facet).sectors.remove(target);
    state.registry.remove::<Position>(target);
    state.registry.insert(target, Ridden { rider: player });
    state.registry.insert(
        player,
        Riding {
            mount: target,
            item,
        },
    );

    // Everyone who sees the rider sees the saddle: the 0x2E draws the mount.
    broadcast_equip(state, item, player);
    let _ = target_serial;
    true
}

/// Put the rider back on foot: the mount item vanishes and the creature lands
/// beside its rider (or under, when every neighbouring tile is blocked).
pub fn dismount(state: &mut WorldState, player: EntityId) {
    let Some(&Riding { mount, item }) = state.registry.get::<Riding>(player) else {
        return;
    };
    state.registry.remove::<Riding>(player);
    state.registry.remove::<Ridden>(mount);

    // The saddle disappears from every screen that drew it — the watchers'
    // and the rider's own.
    if let Some(item_serial) = state.registry.serial_of(item) {
        for watcher in state.watchers_of(player) {
            state.forget(watcher, item, item_serial);
        }
        state.forget(player, item, item_serial);
    }
    state.registry.despawn(item);

    // The creature lands on the first open tile beside the rider.
    let Some(&Position(rider_at)) = state.registry.get::<Position>(player) else {
        return;
    };
    let facet = state.facet_of(player);
    let mut landing = rider_at;
    for dir in 0..8u8 {
        let dir = openshard_protocol::Direction::from_bits(dir);
        if let Some(tile) = step_from(rider_at, dir) {
            if state
                .facet_state(facet)
                .live_terrain()
                .can_step(rider_at, tile)
                .is_some()
            {
                landing = tile;
                break;
            }
        }
    }
    state.registry.insert(mount, Position(landing));
    state.registry.insert(mount, Facet(facet));
    state.facet_state_mut(facet).sectors.insert(mount, landing);
    state.reveal(mount);
    // And the rider redraws on foot for everyone watching.
    state.broadcast_move(player);
}
