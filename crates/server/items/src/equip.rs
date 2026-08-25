use super::*;
use openshard_protocol::wire::{Graphic, Hue};
use openshard_state::weapon::{LAYER_ONE_HANDED, LAYER_TWO_HANDED, weapon_data, weapon_layer};

/// The highest layer an item can be worn on: 1–25 are the body; higher numbers
/// are the backpack and bank, not "worn".
pub(crate) const MAX_WEARABLE_LAYER: Layer = Layer(25);

/// Put a plain worn item on a mobile — a robe, hair, shoes. Like
/// [`equip_new_container`] but without the `Container`, so it is clothing, not a
/// bag. Drawn as part of the wearer's `0x78`; how an NPC stops being naked.
pub fn equip_worn_item(
    state: &mut WorldState,
    mobile: Serial,
    graphic: Graphic,
    hue: Hue,
    layer: Layer,
) -> Option<EntityId> {
    let (entity, serial) = match state.registry.spawn_with_serial(SerialKind::Item) {
        Ok(pair) => pair,
        Err(error) => {
            warn!(?error, "out of item serials; not equipping clothing");
            return None;
        }
    };
    state.registry.insert(entity, Drawn { id: graphic, hue });
    let equipped = Equipped { mobile, layer };
    establish_item_location(state, entity, ItemLocation::equipped(equipped))
        .expect("new clothing has one valid paperdoll location");
    debug!(%serial, graphic = graphic.0, layer = layer.0, "clothing equipped");
    Some(entity)
}

/// Wear a client's held item on a mobile. See `Command::EquipItem`.
pub fn equip_item(
    state: &mut WorldState,
    connection: ConnectionId,
    item: RawSerial,
    layer: RawLayer,
    mobile: RawSerial,
) {
    // Equipping is a *drop* of the dragged item, so there has to be one, and
    // it has to be the item named.
    let Some(held) = state.held_of(connection) else {
        return;
    };
    if state.registry.serial_of(held.entity) != item.validate() {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    }
    // Which slots may be *worn into* is this crate's rule, not the wire's: a
    // layer byte is a name, and `RawLayer::interpret` gives it back whole.
    let layer = layer.interpret();
    if layer == Layer(0) || layer > MAX_WEARABLE_LAYER {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    }
    let (Some(wearer_serial), Some(wearer)) = (
        mobile.validate(),
        mobile.validate().and_then(|s| state.registry.entity_of(s)),
    ) else {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    };
    // Only the player on this connection may wear the item. Besides being the
    // authority boundary, this prevents another cursor from filling an origin
    // layer while its owner is in the middle of a drag.
    let Some(&player) = state.players.get(&connection) else {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    };
    if wearer != player {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    }
    // Only a mobile wears things, and it still has to be in reach.
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
    if state.facet_of(wearer) != state.facet_of(player) || !in_range(wearer_pos, player_pos, ITEM_REACH) {
        bounce(state, connection, held, DragCancelReason::OutOfRange);
        return;
    }
    // A layer holds one thing.
    if layer_taken(state, wearer_serial, layer) {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    }
    let Some(drawn) = state.registry.get::<Drawn>(held.entity).copied() else {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    };
    if hands_conflict(state, wearer_serial, drawn.id, layer) {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    }

    let equipped = Equipped {
        mobile: wearer_serial,
        layer,
    };
    relocate_item(state, held.entity, ItemLocation::equipped(equipped))
        .expect("an accepted equip has one valid paperdoll location");
    broadcast_equip(state, held.entity, wearer);
    debug!(item = item.0, layer = layer.0, "equipped");
}

/// The two protocol hand layers are not two independent weapon slots. A
/// one-handed weapon may share `TwoHanded` with a shield, but a weapon in that
/// layer is two-handed and therefore excludes anything in `OneHanded`.
fn hands_conflict(state: &WorldState, mobile: Serial, graphic: Graphic, layer: Layer) -> bool {
    let Some(weapon) = weapon_data(graphic) else {
        return false;
    };
    // The client proposes a layer, but tiledata (and the handful of weapon
    // class overrides) decides where the weapon actually belongs.
    // An empty table — a shard with no client files — gives every graphic layer
    // zero, so every weapon is one-handed: the same bargain a terrainless shard
    // makes by allowing every step.
    let tile_layer = Layer(state.tiles().static_tile(graphic.0).layer);
    let expected = weapon_layer(weapon, tile_layer);
    if expected != Layer(0) && expected != layer {
        return true;
    }
    let one_hand = layer == LAYER_ONE_HANDED;
    let two_hands = layer == LAYER_TWO_HANDED;
    if !one_hand && !two_hands {
        return false;
    }
    if two_hands && layer_taken(state, mobile, LAYER_ONE_HANDED) {
        return true;
    }
    if !one_hand {
        return false;
    }
    equipped_items(state, mobile).any(|(item, worn)| {
        worn.layer == LAYER_TWO_HANDED
            && state
                .registry
                .get::<Drawn>(item)
                .is_some_and(|existing| weapon_data(existing.id).is_some())
    })
}

/// Despawn everything a mobile carries — its worn items and whatever those hold.
///
/// Called when the mobile itself is leaving and its belongings are not persisted
/// yet, so they must not outlive it as orphans equipped on a serial that is about
/// to be released. Walk the whole ownership subtree: inventory persistence saves
/// nested containers at every depth, so logout must remove that same whole tree.
pub fn despawn_belongings(state: &mut WorldState, mobile: Serial) {
    let worn: Vec<(EntityId, Option<Serial>)> = equipped_items(state, mobile)
        .map(|(item, _)| (item, state.registry.serial_of(item)))
        .collect();

    let mut belongings = worn;
    let mut containers: Vec<Serial> = belongings.iter().filter_map(|(_, serial)| *serial).collect();
    while let Some(container) = containers.pop() {
        let children: Vec<(EntityId, Option<Serial>)> = contained_items(state, container)
            .map(|(item, _)| (item, state.registry.serial_of(item)))
            .collect();
        containers.extend(children.iter().filter_map(|(_, serial)| *serial));
        belongings.extend(children);
    }

    // Children first keeps no live canonical edge pointing at an entity already
    // removed from the registry, even within this short teardown operation.
    for (item, serial) in belongings.into_iter().rev() {
        if let Some(serial) = serial {
            state.open_containers.remove(&serial);
        }
        despawn_item(state, item);
    }
}

/// Whether a mobile already wears something on a layer.
pub fn layer_taken(state: &WorldState, mobile: Serial, layer: Layer) -> bool {
    equipped_items(state, mobile).any(|(_, worn)| worn.layer == layer)
}

/// Tell everyone who can see `mobile`, and the mobile itself if it is a
/// player, that it is now wearing `item` — a `0x2E` each.
pub fn broadcast_equip(state: &mut WorldState, item: EntityId, mobile: EntityId) {
    let Some(update) = equip_packet(state, item) else {
        return;
    };
    for watcher in equip_audience(state, mobile) {
        if let Some(&Client { connection, .. }) = state.registry.get::<Client>(watcher) {
            state.send_packet(connection, &ServerPacket::EquipUpdate(update));
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
            state.send_packet(connection, &ServerPacket::Remove(Remove { serial: item }));
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
pub fn equip_packet(state: &WorldState, item: EntityId) -> Option<EquipUpdate> {
    let serial = state.registry.serial_of(item)?;
    let ItemLocation::Settled(SettledItemLocation::Equipped(Equipped { mobile, layer })) =
        item_location(state, item)?
    else {
        return None;
    };
    let Drawn { id, hue } = *state.registry.get::<Drawn>(item)?;
    Some(EquipUpdate {
        item: serial,
        graphic: id,
        layer,
        mobile,
        hue,
    })
}
