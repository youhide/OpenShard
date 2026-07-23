use super::*;

/// The highest layer an item can be worn on: 1–25 are the body; higher numbers
/// are the backpack and bank, not "worn".
pub(crate) const MAX_WEARABLE_LAYER: u8 = 25;

/// Put a plain worn item on a mobile — a robe, hair, shoes. Like
/// [`equip_new_container`] but without the `Container`, so it is clothing, not a
/// bag. Drawn as part of the wearer's `0x78`; how an NPC stops being naked.
pub fn equip_worn_item(
    state: &mut WorldState,
    mobile: Serial,
    graphic: u16,
    hue: u16,
    layer: u8,
) -> Option<EntityId> {
    let (entity, serial) = match state.registry.spawn_with_serial(SerialKind::Item) {
        Ok(pair) => pair,
        Err(error) => {
            warn!(?error, "out of item serials; not equipping clothing");
            return None;
        }
    };
    state.registry.insert(entity, Graphic { id: graphic, hue });
    state.registry.insert(entity, Equipped { mobile, layer });
    debug!(%serial, graphic, layer, "clothing equipped");
    Some(entity)
}

/// Wear a client's held item on a mobile. See `Command::EquipItem`.
pub fn equip_item(
    state: &mut WorldState,
    connection: ConnectionId,
    item: u32,
    layer: u8,
    mobile: u32,
) {
    // Equipping is a *drop* of the dragged item, so there has to be one, and
    // it has to be the item named.
    let Some(held) = state.held.get(&connection).copied() else {
        return;
    };
    if state.registry.serial_of(held.entity) != Serial::new(item) {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    }
    if layer == 0 || layer > MAX_WEARABLE_LAYER {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    }
    let (Some(wearer_serial), Some(wearer)) = (
        Serial::new(mobile),
        Serial::new(mobile).and_then(|s| state.registry.entity_of(s)),
    ) else {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    };
    // Only a mobile wears things, and only within reach of the player.
    let Some(&player) = state.players.get(&connection) else {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    };
    let (Some(&Position(wearer_pos)), Some(&Position(player_pos))) = (
        state.registry.get::<Position>(wearer),
        state.registry.get::<Position>(player),
    ) else {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    };
    if !state.registry.has::<Body>(wearer) {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    }
    if state.facet_of(wearer) != state.facet_of(player)
        || !in_range(wearer_pos, player_pos, ITEM_REACH)
    {
        bounce(state, connection, held, DragCancelReason::OutOfRange);
        return;
    }
    // A layer holds one thing.
    if layer_taken(state, wearer_serial, layer) {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    }

    state.held.remove(&connection);
    state.registry.insert(
        held.entity,
        Equipped {
            mobile: wearer_serial,
            layer,
        },
    );
    broadcast_equip(state, held.entity, wearer);
    debug!(item, layer, "equipped");
}

/// Despawn everything a mobile carries — its worn items and whatever those hold.
///
/// Called when the mobile itself is leaving and its belongings are not persisted
/// yet, so they must not outlive it as orphans equipped on a serial that is about
/// to be released. One level deep — a backpack of loose items — which is all a
/// character has until nested containers and inventory persistence land.
pub fn despawn_belongings(state: &mut WorldState, mobile: Serial) {
    let worn: Vec<(EntityId, Serial)> = state
        .registry
        .query::<Equipped>()
        .filter(|(_, worn)| worn.mobile == mobile)
        .filter_map(|(item, _)| Some((item, state.registry.serial_of(item)?)))
        .collect();
    let worn_serials: Vec<Serial> = worn.iter().map(|(_, serial)| *serial).collect();

    let inside: Vec<EntityId> = state
        .registry
        .query::<Contained>()
        .filter(|(_, held)| worn_serials.contains(&held.container))
        .map(|(item, _)| item)
        .collect();
    for item in inside {
        state.registry.despawn(item);
    }
    for (item, serial) in worn {
        state.open_containers.remove(&serial);
        state.registry.despawn(item);
    }
}

/// Whether a mobile already wears something on a layer.
pub fn layer_taken(state: &WorldState, mobile: Serial, layer: u8) -> bool {
    state
        .registry
        .query::<Equipped>()
        .any(|(_, worn)| worn.mobile == mobile && worn.layer == layer)
}

/// Tell everyone who can see `mobile`, and the mobile itself if it is a
/// player, that it is now wearing `item` — a `0x2E` each.
pub fn broadcast_equip(state: &mut WorldState, item: EntityId, mobile: EntityId) {
    let Some(packet) = equip_packet(state, item) else {
        return;
    };
    for watcher in equip_audience(state, mobile) {
        if let Some(&Client { connection, .. }) = state.registry.get::<Client>(watcher) {
            state.outbox.push(Outbound {
                connection,
                packet: packet.clone(),
            });
        }
    }
}

/// Tell everyone who can see `mobile`, and the mobile itself, to forget a worn
/// item just taken off it — a `0x1D` each. The mirror of [`broadcast_equip`]:
/// there is no "remove from paperdoll" packet, so the client drops a worn item
/// the same way it drops any object, by its serial. Unlike the lift path in
/// `pick_up`, the wearer's own client is included here, because it is not the one
/// holding the item on a cursor.
pub(crate) fn broadcast_unequip(state: &mut WorldState, item: Serial, mobile: EntityId) {
    for watcher in equip_audience(state, mobile) {
        if let Some(&Client { connection, .. }) = state.registry.get::<Client>(watcher) {
            state.outbox.push(Outbound {
                connection,
                packet: encode_remove(item.raw()),
            });
        }
    }
}

/// Everyone who should hear about a change to `mobile`'s outfit: those who
/// can see it, and the mobile itself.
pub fn equip_audience(state: &WorldState, mobile: EntityId) -> Vec<EntityId> {
    let mut audience = state.watchers_of(mobile);
    audience.push(mobile);
    audience
}

/// Build the `0x2E` for a worn item.
pub fn equip_packet(state: &WorldState, item: EntityId) -> Option<Vec<u8>> {
    let serial = state.registry.serial_of(item)?;
    let Equipped { mobile, layer } = *state.registry.get::<Equipped>(item)?;
    let Graphic { id, hue } = *state.registry.get::<Graphic>(item)?;
    Some(encode_equip(serial.raw(), id, layer, mobile.raw(), hue))
}
