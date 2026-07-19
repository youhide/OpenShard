use super::*;
use openshard_movement::{step_from, Terrain, Walker};
use openshard_protocol::Notoriety;
use openshard_state::components::{Aggression, Brain, Heading, Hitpoints, Movement};

/// The layer a mount item rides on — the client draws whoever wears one as
/// mounted. `0x19`, the classic mount layer.
pub const MOUNT_LAYER: u8 = 0x19;

/// How close a rider must stand to swing up.
const MOUNT_REACH: u32 = 2;

/// The hit points a save-rebuilt mount comes back with — the save keeps only the
/// saddle, not the creature, so a restored horse is simply healthy.
const DEFAULT_MOUNT_HITS: u16 = 50;

/// How far a dismounted, save-rebuilt mount notices the world — enough to flee a
/// blow (passive animals run when struck), not enough to go looking for one.
const MOUNT_SIGHT: u8 = 8;

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

    // The saddle disappears from every screen that drew it — the watchers' and
    // the rider's own. It rode out as a `0x2E` (equipment is drawn as part of the
    // wearer's `0x78`, and never enters anyone's `seen`), so `forget` would find
    // nothing to remove and send no `0x1D`, leaving the rider looking mounted.
    // The remove goes straight to the equip audience, the way `drag.rs` unequips
    // a worn item; the client then drops the mount layer and redraws on foot.
    if let Some(item_serial) = state.registry.serial_of(item) {
        for watcher in equip_audience(state, player) {
            if let Some(&Client { connection, .. }) = state.registry.get::<Client>(watcher) {
                state.outbox.push(Outbound {
                    connection,
                    packet: encode_remove(item_serial.raw()),
                });
            }
        }
    }
    state.registry.despawn(item);

    // The creature lands on the first open tile beside the rider, at the floor z
    // the terrain computes for it — not the rider's own z carried verbatim.
    let Some(&Position(rider_at)) = state.registry.get::<Position>(player) else {
        return;
    };
    let facet = state.facet_of(player);
    let mut landing = rider_at;
    for dir in 0..8u8 {
        let dir = openshard_protocol::Direction::from_bits(dir);
        if let Some(tile) = step_from(rider_at, dir) {
            if let Some(landed) = state
                .facet_state(facet)
                .live_terrain()
                .can_step(rider_at, tile)
            {
                landing = landed;
                break;
            }
        }
    }
    state.registry.insert(mount, Position(landing));
    state.registry.insert(mount, Facet(facet));

    // Reconstitute what the ride (or a save) stripped. The horse faces the way
    // its rider faces — and without a `Heading` the `0x78` encoder refuses to
    // draw it at all, which was the invisible-horse bug after a mounted relogin.
    let heading = state
        .registry
        .get::<Heading>(player)
        .copied()
        .unwrap_or(Heading(openshard_protocol::Facing::walking(
            openshard_protocol::Direction::South,
        )));
    state.registry.insert(mount, heading);
    // The walker restarts at the landing, always: the ride never moved it, so a
    // horse ridden across the map would otherwise take its next step from where
    // it was *mounted* — teleporting away and vanishing off the rider's screen.
    state
        .registry
        .insert(mount, Movement(Walker::new(landing, heading.0)));
    // A save-rebuilt mount carries only its body; give it the pack-horse
    // temperament so it is a live animal, not an inert prop. A fresh mount keeps
    // its real spawned values — these fill in only where nothing is.
    if state.registry.get::<Notoriety>(mount).is_none() {
        state.registry.insert(mount, Notoriety::Innocent);
    }
    if state.registry.get::<Hitpoints>(mount).is_none() {
        state.registry.insert(
            mount,
            Hitpoints {
                current: DEFAULT_MOUNT_HITS,
                max: DEFAULT_MOUNT_HITS,
            },
        );
    }
    if state.registry.get::<Brain>(mount).is_none() {
        state.registry.insert(
            mount,
            Brain {
                sight: MOUNT_SIGHT,
                wander: true,
                aggression: Aggression::Passive,
                ..Brain::default()
            },
        );
    }

    state.facet_state_mut(facet).sectors.insert(mount, landing);
    state.reveal(mount);
    // And the rider redraws on foot for everyone watching.
    state.broadcast_move(player);
}
