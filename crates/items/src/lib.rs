//! Items: spawning, the drag protocol, stacking, decay, containers, and gear.
//!
//! A gameplay system in its own crate, operating on the shared [`WorldState`].
//! An item is an entity in exactly one of three places — on the ground
//! ([`Position`]), inside a container ([`Contained`]), or worn ([`Equipped`]) —
//! and these functions move it between them: spawn it, lift it onto a cursor,
//! drop it, stack or split it, decay it, put it in a container, wear it. Reach
//! and layer checks are server-authoritative; the client's word is never taken.
//!
//! The drawing goes through [`WorldState`]'s interest machinery (`reveal`,
//! `show`, `forget`); this crate owns the *rules* of where a thing is.

use openshard_entities::{EntityId, Serial, SerialKind};
use openshard_gateway::ConnectionId;
use openshard_protocol::{
    encode_add_to_container, encode_container_contents, encode_drag_cancel, encode_equip,
    encode_open_container, encode_remove, ContainedItem, DragCancelReason, Point, DROP_TO_GROUND,
};
use openshard_state::components::{
    Amount, Body, Client, Contained, Container, Decays, Equipped, Facet, Graphic, Position,
    Stackable,
};
use openshard_state::sectors::in_range;
use openshard_state::{HeldItem, Origin, Outbound, WorldState};
use tracing::{debug, warn};

/// How near, in tiles, a mobile must be to reach an item on the ground or set one
/// down. Sphere reaches two; a third forgives the diagonal the cursor is shown
/// on. Server-authoritative — the client's word is never taken.
const ITEM_REACH: u32 = 3;
/// The highest layer an item can be worn on: 1–25 are the body; higher numbers
/// are the backpack and bank, not "worn".
const MAX_WEARABLE_LAYER: u8 = 25;

/// An item appeared in the world.
///
/// Emitted when the server puts a thing on the ground — the item counterpart of
/// `PlayerEntered`. What a script or persistence does with it is their affair;
/// the world's part is only to say it happened.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ItemSpawned {
    /// The entity.
    pub entity: EntityId,
    /// Its wire identity.
    pub serial: Serial,
    /// Where it lies.
    pub position: Point,
}

/// Put an item on the ground. See `Command::SpawnItem`.
///
/// Returns the entity so `spawn_container` can make
/// the same thing and then say it holds others.
pub fn spawn_item(
    state: &mut WorldState,
    graphic: u16,
    hue: u16,
    amount: u16,
    stackable: bool,
    position: Point,
    facet: u8,
) -> Option<EntityId> {
    let facet = if state.facets.contains_key(&facet) {
        facet
    } else {
        warn!(facet, "unloaded facet; spawning the item on the default");
        state.default_facet
    };
    let (entity, serial) = match state.registry.spawn_with_serial(SerialKind::Item) {
        Ok(pair) => pair,
        Err(error) => {
            warn!(?error, "out of item serials; not spawning");
            return None;
        }
    };
    state.registry.insert(entity, Graphic { id: graphic, hue });
    state.registry.insert(entity, Position(position));
    state.registry.insert(entity, Facet(facet));
    // Only a real stack carries an amount; a single item stays a bare graphic.
    if amount > 1 {
        state.registry.insert(entity, Amount(amount));
    }
    if stackable {
        state.registry.insert(entity, Stackable);
    }
    mark_decay(state, entity);
    state
        .facet_state_mut(facet)
        .sectors
        .insert(entity, position);
    state.bus.send(ItemSpawned {
        entity,
        serial,
        position,
    });
    state.reveal(entity);
    debug!(%serial, graphic, position = %position, "item on the ground");
    Some(entity)
}

/// Put a container on the ground. See `Command::SpawnContainer`.
///
/// A container is an ordinary ground item that also carries a [`Container`],
/// which is the only thing that makes it openable. So it is spawned exactly
/// like one and then marked.
pub fn spawn_container(
    state: &mut WorldState,
    graphic: u16,
    gump: u16,
    hue: u16,
    position: Point,
    facet: u8,
) {
    if let Some(entity) = spawn_item(state, graphic, hue, 1, false, position, facet) {
        state.registry.insert(entity, Container { gump });
        // A container does not rot with its contents inside it; only loose
        // ground clutter decays.
        state.registry.remove::<Decays>(entity);
    }
}
/// Set an item's decay clock: it rots `gameplay.decay_ticks` from now. Every
/// loose item on the ground has one; every item off it has none, and so does a
/// container — it and its contents stay put until someone moves them, which is
/// also why a container picked up and set back down does not start rotting.
pub fn mark_decay(state: &mut WorldState, item: EntityId) {
    if state.registry.has::<Container>(item) {
        return;
    }
    state.registry.insert(
        item,
        Decays {
            at_tick: state.ticks + state.gameplay.decay_ticks,
        },
    );
}

/// Open a container onto a client's screen. See `Command::DoubleClick`.
///
/// Only containers do anything yet — a double-click on anything else is
/// ignored rather than answered, because "use" for a door or a food is a
/// later rule and a wrong guess is worse than silence.
pub fn double_click(state: &mut WorldState, connection: ConnectionId, serial: u32) {
    let Some(&player) = state.players.get(&connection) else {
        return;
    };
    let Some(item_serial) = Serial::new(serial) else {
        return;
    };
    let Some(item) = state.registry.entity_of(item_serial) else {
        return;
    };
    let Some(&Container { gump }) = state.registry.get::<Container>(item) else {
        return;
    };
    // The container has to be in reach on the ground. Nesting — opening one
    // out of another already open — is a later refinement.
    let Some(&Position(item_pos)) = state.registry.get::<Position>(item) else {
        return;
    };
    let Some(&Position(player_pos)) = state.registry.get::<Position>(player) else {
        return;
    };
    if state.facet_of(item) != state.facet_of(player) || !in_range(item_pos, player_pos, ITEM_REACH)
    {
        return;
    }
    let Some(&Client { version, .. }) = state.registry.get::<Client>(player) else {
        return;
    };

    let contents = contents_of(state, item_serial);
    state.send(connection, encode_open_container(serial, gump, version));
    state.send(
        connection,
        encode_container_contents(serial, &contents, version),
    );
    // Remember it is open, so a later change to its contents can be pushed here.
    state
        .open_containers
        .entry(item_serial)
        .or_default()
        .insert(connection);
    debug!(%item_serial, items = contents.len(), "container opened");
}

/// Everything inside a container, as the wire records `0x3C`/`0x25` need.
pub fn contents_of(state: &WorldState, container: Serial) -> Vec<ContainedItem> {
    state
        .registry
        .query::<Contained>()
        .filter(|(_, held)| held.container == container)
        .filter_map(|(entity, _)| contained_record(state, entity))
        .collect()
}

/// How many items a container already holds — the next free grid slot.
pub fn item_count(state: &WorldState, container: Serial) -> u8 {
    state
        .registry
        .query::<Contained>()
        .filter(|(_, held)| held.container == container)
        .count()
        .min(u8::MAX as usize) as u8
}

/// How many of `graphic` a container holds, counting stack amounts.
#[must_use]
pub fn count_in_container(state: &WorldState, container: Serial, graphic: u16) -> u32 {
    state
        .registry
        .query::<Contained>()
        .filter(|(_, held)| held.container == container)
        .filter(|(entity, _)| {
            state
                .registry
                .get::<Graphic>(*entity)
                .is_some_and(|g| g.id == graphic)
        })
        .map(|(entity, _)| u32::from(state.registry.get::<Amount>(entity).map_or(1, |a| a.0)))
        .sum()
}

/// Take `count` of `graphic` out of a container, all or nothing.
///
/// The container/inventory search reagents are built on: a spell needs its
/// reagents *and* consumes them, so this both checks and takes in one pass —
/// returns `false` and touches nothing if the container is short, else removes
/// exactly `count` (whole items, then a partial stack) and returns `true`. A
/// stack it empties is despawned; a stack it dips into loses that much
/// [`Amount`]. (A container open on a client is not live-redrawn yet — reagents
/// come from a closed pack; the gump refreshes when reopened.)
pub fn take_from_container(
    state: &mut WorldState,
    container: Serial,
    graphic: u16,
    count: u16,
) -> bool {
    if count == 0 {
        return true;
    }
    let matches: Vec<(EntityId, u16)> = state
        .registry
        .query::<Contained>()
        .filter(|(_, held)| held.container == container)
        .filter(|(entity, _)| {
            state
                .registry
                .get::<Graphic>(*entity)
                .is_some_and(|g| g.id == graphic)
        })
        .map(|(entity, _)| {
            (
                entity,
                state.registry.get::<Amount>(entity).map_or(1, |a| a.0),
            )
        })
        .collect();
    let total: u32 = matches.iter().map(|(_, amount)| u32::from(*amount)).sum();
    if total < u32::from(count) {
        return false;
    }

    let mut remaining = count;
    for (entity, amount) in matches {
        if remaining == 0 {
            break;
        }
        if amount <= remaining {
            // The whole item goes: a contained item is on no sector grid and no
            // screen, so despawning it is all it takes.
            remaining -= amount;
            let serial = state.registry.serial_of(entity);
            state.registry.despawn(entity);
            if let Some(serial) = serial {
                tell_watchers_removed(state, container, serial);
            }
        } else {
            set_stack_amount(state, entity, amount - remaining);
            remaining = 0;
            tell_watchers_updated(state, container, entity);
        }
    }
    true
}

/// Tell every client with `container` open that `item` has left it — a `0x1D`,
/// the same "forget that" the interest system draws with, so a reagent consumed
/// out of an open pack disappears from the gump live.
fn tell_watchers_removed(state: &mut WorldState, container: Serial, item: Serial) {
    let watchers: Vec<ConnectionId> = state
        .open_containers
        .get(&container)
        .map(|w| w.iter().copied().collect())
        .unwrap_or_default();
    for connection in watchers {
        state.send(connection, encode_remove(item.raw()));
    }
}

/// Tell every client with `container` open that an item in it changed — a dipped
/// stack's new amount — by re-sending its `0x25` record.
fn tell_watchers_updated(state: &mut WorldState, container: Serial, entity: EntityId) {
    let Some(record) = contained_record(state, entity) else {
        return;
    };
    let watchers: Vec<ConnectionId> = state
        .open_containers
        .get(&container)
        .map(|w| w.iter().copied().collect())
        .unwrap_or_default();
    for connection in watchers {
        let version = state
            .players
            .get(&connection)
            .and_then(|&player| state.registry.get::<Client>(player))
            .map(|client| client.version);
        if let Some(version) = version {
            state.send(
                connection,
                encode_add_to_container(record, container.raw(), version),
            );
        }
    }
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
    // The container has to be a reachable one on the ground. Dropping into a
    // container that is itself inside another is a later refinement.
    let Some(&Position(container_pos)) = state.registry.get::<Position>(container_entity) else {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    };
    let Some(&Position(player_pos)) = state.registry.get::<Position>(player) else {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    };
    if state.facet_of(container_entity) != state.facet_of(player)
        || !in_range(container_pos, player_pos, ITEM_REACH)
    {
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
        Some(target) if state.registry.has::<Container>(target) => {
            drop_into_container(state, connection, held, position, target_serial);
        }
        Some(target) if can_stack(state, held.entity, target) => {
            merge_onto(state, connection, held, target);
        }
        _ => bounce(state, connection, held, DragCancelReason::Other),
    }
}

/// Whether two items are one pile waiting to happen: both stackable, same
/// graphic and hue, and not the same entity.
pub fn can_stack(state: &WorldState, a: EntityId, b: EntityId) -> bool {
    a != b
        && state.registry.has::<Stackable>(a)
        && state.registry.has::<Stackable>(b)
        && state.registry.get::<Graphic>(a) == state.registry.get::<Graphic>(b)
}

/// Merge a held stack onto a stack on the ground. See `can_stack`.
pub fn merge_onto(
    state: &mut WorldState,
    connection: ConnectionId,
    held: HeldItem,
    target: EntityId,
) {
    // Only ground stacks merge for now; merging onto a stack inside a
    // container is a later refinement, and until then it bounces.
    let Some(&Position(target_pos)) = state.registry.get::<Position>(target) else {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    };
    let Some(&player) = state.players.get(&connection) else {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    };
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

    // Sum, clamped: a pile cannot count past what its amount word can hold.
    let total = amount_of(state, held.entity).saturating_add(amount_of(state, target));
    set_stack_amount(state, target, total);
    state.held.remove(&connection);
    // The dragged stack is gone into the other; it was on a cursor, on
    // nobody's ground, so despawning it needs no packet.
    state.registry.despawn(held.entity);
    redraw_ground_item(state, target);
    debug!(total, "stacks merged");
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

/// Leave the remainder of a split stack behind, at the same spot, as a fresh
/// pile. A dupe with a new serial — the original goes onto the cursor keeping
/// its own serial, so the client's drag and its eventual drop still name it,
/// and the copy is what the ground is left with. Straight from Sphere's
/// `CItem::UnStackSplit`.
pub fn spawn_leftover(
    state: &mut WorldState,
    original: EntityId,
    amount: u16,
    position: Point,
    facet: u8,
) {
    let Some(&Graphic { id, hue }) = state.registry.get::<Graphic>(original) else {
        return;
    };
    let leftover = match state.registry.spawn_with_serial(SerialKind::Item) {
        Ok((entity, _)) => entity,
        Err(error) => {
            warn!(?error, "out of item serials; a split remainder is lost");
            return;
        }
    };
    state.registry.insert(leftover, Graphic { id, hue });
    state.registry.insert(leftover, Stackable);
    set_stack_amount(state, leftover, amount);
    state.registry.insert(leftover, Position(position));
    state.registry.insert(leftover, Facet(facet));
    mark_decay(state, leftover);
    state
        .facet_state_mut(facet)
        .sectors
        .insert(leftover, position);
    state.reveal(leftover);
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

/// Remove every ground item whose decay tick has arrived. Runs each tick,
/// against `ticks`, so it reads no clock.
pub fn decay(state: &mut WorldState) {
    let now = state.ticks;
    let expired: Vec<EntityId> = state
        .registry
        .query::<Decays>()
        .filter(|(_, decays)| decays.at_tick <= now)
        .map(|(entity, _)| entity)
        .collect();
    for item in expired {
        let Some(serial) = state.registry.serial_of(item) else {
            continue;
        };
        let facet = state.facet_of(item);
        for watcher in state.watchers_of(item) {
            state.forget(watcher, item, serial);
        }
        state.facet_state_mut(facet).sectors.remove(item);
        state.registry.despawn(item);
        debug!(%serial, "decayed");
    }
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

/// Build the `0x25`/`0x3C` record for one contained item.
pub fn contained_record(state: &WorldState, entity: EntityId) -> Option<ContainedItem> {
    let serial = state.registry.serial_of(entity)?;
    let Contained { x, y, grid, .. } = *state.registry.get::<Contained>(entity)?;
    let Graphic { id, hue } = *state.registry.get::<Graphic>(entity)?;
    let amount = state.registry.get::<Amount>(entity).map_or(1, |a| a.0);
    Some(ContainedItem {
        serial: serial.raw(),
        graphic: id,
        amount,
        x,
        y,
        grid,
        hue,
    })
}

/// Send a `0x27`, cancelling whatever drag the client thinks it has.
pub fn reject_drag(state: &mut WorldState, connection: ConnectionId, reason: DragCancelReason) {
    state.send(connection, encode_drag_cancel(reason));
}

/// Land an item on the ground at `position` and draw it for everyone in range.
pub fn place_on_ground(state: &mut WorldState, item: EntityId, position: Point, facet: u8) {
    state.registry.insert(item, Position(position));
    state.registry.insert(item, Facet(facet));
    // Back on the ground, back on the decay clock.
    mark_decay(state, item);
    state.facet_state_mut(facet).sectors.insert(item, position);
    state.reveal(item);
}
