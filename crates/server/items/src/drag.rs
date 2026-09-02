use std::fmt;

use openshard_entities::SpawnError;
use openshard_state::{
    LocationError,
    PreparedItemRelocation,
    commit_item_relocation,
    prepare_item_relocation,
};

use super::*;

/// How near, in tiles, a mobile must be to reach an item on the ground or set one
/// down. Sphere reaches two; a third forgives the diagonal the cursor is shown
/// on. Server-authoritative — the client's word is never taken.
pub(crate) const ITEM_REACH: u32 = 3;

/// The layers nothing may ever be lifted off — hair (`0x0B`) and a beard (`0x10`).
///
/// UO has no "hair" field on a mobile: hair and a beard are ordinary items worn on
/// their own layers and drawn in the same `0x78` equipment list as a shirt. Which
/// means the lift path below would happily take them, and a player standing next to
/// a shopkeeper could pull the hair off its head onto their cursor. ServUO marks the
/// same items `Movable = false`; this is that, at the one door a lift comes through.
pub const FIXED_LAYERS: &[Layer] = &[Layer(0x0B), Layer(0x10)];

/// What a lift off a house's lockdown says. ServUO's cliloc 501727 in plain
/// words, on `LOCKED_MESSAGE`'s licence: the refusals in this crate are English
/// because the reference sends half of them as plain lines too.
pub const LOCKED_DOWN_MESSAGE: &str = "That is locked down and you cannot lift it.";

/// What a drop into a full house says. ServUO's cliloc 1080013 in plain words,
/// on [`LOCKED_DOWN_MESSAGE`]'s licence.
pub const SECURE_FULL_MESSAGE: &str = "This house cannot hold any more.";

/// An item whose identity and current parent have passed a lift's basic gates.
#[derive(Clone, Copy)]
struct LiftableItem {
    entity:   EntityId,
    serial:   Serial,
    location: ItemLocation,
}

/// A lift whose cursor transition and optional stack remainder are ready to
/// commit without allocation or another gameplay refusal.
struct PreparedLift {
    relocation: PreparedItemRelocation,
    split:      Option<PreparedSplit>,
}

/// The quantity half of a partial lift.
///
/// `leftover` already owns its serial and identity, but deliberately has no
/// location and has emitted no event or packet until [`commit_split`].
struct PreparedSplit {
    original:  EntityId,
    leftover:  EntityId,
    taken:     u16,
    remainder: u16,
    origin:    SettledItemLocation,
}

#[derive(Debug)]
enum PrepareLiftError {
    Location(LocationError),
    Allocation(SpawnError),
}

impl fmt::Display for PrepareLiftError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Location(error) => write!(formatter, "invalid cursor transition: {error:?}"),
            Self::Allocation(error) => write!(formatter, "cannot allocate split remainder: {error}"),
        }
    }
}

/// The art of an item that already passed the lift gate.
///
/// [`liftable_item`] refuses every entity without [`Drawn`] before it can become
/// held, so absence here is a broken cursor invariant rather than graphic zero.
fn held_graphic(state: &WorldState, item: EntityId) -> Graphic {
    state
        .registry
        .get::<Drawn>(item)
        .expect("an item on a cursor must have art")
        .id
}

/// Validate a lift and reserve its remainder before changing the visible pile.
fn prepare_lift(
    state: &mut WorldState,
    connection: ConnectionId,
    item: LiftableItem,
    amount: u16,
    origin: SettledItemLocation,
) -> Result<PreparedLift, PrepareLiftError> {
    let relocation = prepare_item_relocation(state, item.entity, ItemLocation::Held { connection, origin })
        .map_err(PrepareLiftError::Location)?;

    let total = amount_of(state, item.entity);
    let stackable = state.registry.has::<Stackable>(item.entity)
        || state
            .registry
            .get::<Drawn>(item.entity)
            .is_some_and(|drawn| intrinsically_stackable(drawn.id));
    let split = if amount > 0 && amount < total && stackable {
        // Serial first. `spawn_with_serial` leaves no entity behind on failure,
        // so this is the last ordinary failure before the operation commits.
        let (leftover, _) = state
            .registry
            .spawn_with_serial(SerialKind::Item)
            .map_err(PrepareLiftError::Allocation)?;
        let drawn = *state
            .registry
            .get::<Drawn>(item.entity)
            .expect("a liftable item has art");
        state.registry.insert(leftover, drawn);
        crate::spawn::copy_identity(state, item.entity, leftover);
        state.registry.insert(leftover, Stackable);
        initialize_stack_amount(state, leftover, total - amount);
        Some(PreparedSplit {
            original: item.entity,
            leftover,
            taken: amount,
            remainder: total - amount,
            origin,
        })
    } else {
        None
    };

    Ok(PreparedLift { relocation, split })
}

/// Publish a prepared remainder at the origin the original pile is vacating.
fn commit_split(state: &mut WorldState, split: PreparedSplit) {
    // Normalise legacy gold/arrows/bolts while they participate, so persistence
    // retains the fact that both resulting singleton piles remain stackable.
    state.registry.insert(split.original, Stackable);
    commit_prepared_split_amount(state, split.original, split.taken);
    debug_assert_eq!(amount_of(state, split.leftover), split.remainder);
    establish_item_location(state, split.leftover, ItemLocation::Settled(split.origin))
        .expect("a prepared split remainder keeps the original's valid parent");
    match split.origin {
        SettledItemLocation::Ground { facet, position } => {
            mark_decay(state, split.leftover);
            state.place_item(facet, split.leftover, position);
            state.reveal(split.leftover);
        }
        SettledItemLocation::Contained(contained) => {
            tell_watchers_updated(state, contained.container, split.leftover);
        }
        SettledItemLocation::Equipped(_) => {
            unreachable!("paperdoll lifts are never split")
        }
    }
}

/// Lift an item onto a client's cursor. See `Command::PickUpItem`.
pub fn pick_up(state: &mut WorldState, connection: ConnectionId, serial: RawSerial, amount: u16) {
    let Some(&player) = state.players.get(&connection) else {
        return;
    };
    if let Some(held) = state.held_of(connection) {
        // Converge even a confused/older client. Merely rejecting the second
        // lift leaves the first item in server limbo while the one DragCancel
        // clears whichever transaction the client currently remembers. Bounce
        // the authoritative cursor item instead, so both sides become empty.
        bounce(state, connection, held, DragCancelReason::AlreadyHolding);
        return;
    }
    let Some(item) = liftable_item(state, connection, player, serial) else {
        return;
    };

    let lifted = match item.location {
        ItemLocation::Settled(SettledItemLocation::Ground { facet, position }) => {
            lift_ground_item(state, connection, player, item, amount, facet, position)
        }
        ItemLocation::Settled(SettledItemLocation::Contained(contained)) => {
            lift_contained_item(state, connection, player, item, amount, contained)
        }
        ItemLocation::Settled(SettledItemLocation::Equipped(worn)) => {
            lift_equipped_item(state, connection, player, item, worn);
            true
        }
        // Neither on the ground nor in a container: already on a cursor.
        ItemLocation::Held { .. } => {
            reject_drag(state, connection, DragCancelReason::CannotLift);
            false
        }
    };
    if !lifted {
        return;
    }
    // Reaching for something gives you away and breaks concentration — ServUO
    // calls `DisruptiveAction` from `Mobile.Lift` and from both `Use` overloads,
    // and a thief who could loot from hiding would never need Stealing at all.
    state.break_cover(player);
    debug!(serial = %item.serial, "lifted onto the cursor");
}

/// Check that `serial` names an ordinary, movable item and read its parent.
fn liftable_item(
    state: &mut WorldState,
    connection: ConnectionId,
    player: EntityId,
    serial: RawSerial,
) -> Option<LiftableItem> {
    // The seam, and a refusal rather than silence: a lift the server will not
    // do is answered with a `0x27`, or the client keeps the item on its cursor
    // for ever.
    let Some(item_serial) = serial.validate() else {
        reject_drag(state, connection, DragCancelReason::CannotLift);
        return None;
    };
    let Some(item) = state.registry.entity_of(item_serial) else {
        reject_drag(state, connection, DragCancelReason::CannotLift);
        return None;
    };
    // Only a thing with a graphic is an item. A mobile has none, so this
    // rejects trying to pick up a person.
    if !state.registry.has::<Drawn>(item) {
        reject_drag(state, connection, DragCancelReason::CannotLift);
        return None;
    }
    // A town's fittings are not loot: script-placed decoration cannot be lifted.
    if state.registry.has::<Decoration>(item) {
        reject_drag(state, connection, DragCancelReason::CannotLift);
        return None;
    }
    // Nor is a field's crop, standing or picked. ServUO builds every
    // `FarmableCrop` with `Movable = false` and checks it again in the pick, for
    // the reason the whole slice exists: a cotton plant that lifted into a pack
    // would be a field harvested by dragging rather than by picking, and the
    // plant is not the cotton.
    if state.registry.has::<openshard_state::components::Crop>(item) {
        reject_drag(state, connection, DragCancelReason::CannotLift);
        return None;
    }
    // A house and its sign are both world items for protocol purposes, but are
    // fixed housing infrastructure.  In particular the sign is derived from
    // its house and rebuilt on restore, so letting it into a pack detaches the
    // management UI from the building it represents.
    if state.registry.has::<House>(item) || state.registry.has::<HouseSign>(item) {
        reject_drag(state, connection, DragCancelReason::CannotLift);
        return None;
    }
    // Nor is the trade window itself, which is a worn container and would
    // otherwise lift like any other — ServUO's `CheckLift` refusing outright.
    // What is *inside* it lifts normally; that is how an offer is taken back.
    if state.registry.has::<TradeWindow>(item) {
        reject_drag(state, connection, DragCancelReason::CannotLift);
        return None;
    }
    // Nor is anything locked down inside a house. ServUO's `CheckLift` asks the
    // house the same question, and the answer does not depend on who is asking:
    // a co-owner cannot lift their own lockdown either, they release it first.
    // Staff walk through it, as they do a locked door.
    if state
        .registry
        .has::<openshard_state::components::LockedDown>(item)
        && !state.is_staff(player)
    {
        state.system_message(player, LOCKED_DOWN_MESSAGE);
        reject_drag(state, connection, DragCancelReason::CannotLift);
        return None;
    }
    // Nor is somebody's hair. See `FIXED_LAYERS`.
    let Some(location) = item_location(state, item) else {
        reject_drag(state, connection, DragCancelReason::CannotLift);
        return None;
    };
    if matches!(
        location,
        ItemLocation::Settled(SettledItemLocation::Equipped(worn))
            if FIXED_LAYERS.contains(&worn.layer)
    ) {
        reject_drag(state, connection, DragCancelReason::CannotLift);
        return None;
    }
    Some(LiftableItem {
        entity: item,
        serial: item_serial,
        location,
    })
}

/// Lift a ground item, remove it from every applicable screen, and put it on the cursor.
fn lift_ground_item(
    state: &mut WorldState,
    connection: ConnectionId,
    player: EntityId,
    item: LiftableItem,
    amount: u16,
    facet: Facet,
    position: Point,
) -> bool {
    let Some(&Position(player_position)) = state.registry.get::<Position>(player) else {
        return false;
    };
    if facet != state.facet_of(player) || !in_range(position, player_position, ITEM_REACH) {
        reject_drag(state, connection, DragCancelReason::OutOfRange);
        return false;
    }
    let origin = SettledItemLocation::Ground { facet, position };
    let prepared = match prepare_lift(state, connection, item, amount, origin) {
        Ok(prepared) => prepared,
        Err(error) => {
            warn!(%error, "partial lift could not be prepared");
            reject_drag(state, connection, DragCancelReason::CannotLift);
            return false;
        }
    };
    if let Some(split) = prepared.split {
        commit_split(state, split);
    }
    // Off the sector grid, off every screen but the picker's — whose own
    // client already put it on the cursor, so a 0x1D there would fight it.
    state.unplace(facet, item.entity);
    for watcher in state.watchers_of(item.entity) {
        if watcher == player {
            if let Some(seen) = state.seen.get_mut(&player) {
                seen.remove(&item.entity);
            }
        } else {
            state.forget(watcher, item.entity, item.serial);
        }
    }
    // Off the ground, off the decay clock.
    state.registry.remove::<Decays>(item.entity);
    commit_item_relocation(state, prepared.relocation);
    true
}

/// Lift an item from a container after enforcing its trade-owner rule.
fn lift_contained_item(
    state: &mut WorldState,
    connection: ConnectionId,
    player: EntityId,
    item: LiftableItem,
    amount: u16,
    contained: Contained,
) -> bool {
    // Both halves of a secure trade are visible to both players, but only the
    // owner of a half may remove its contents. Visibility is not authority:
    // otherwise a partner could lift an offered item straight into their own
    // pack before either checkbox was ticked.
    if trade_container_owner(state, contained.container).is_some_and(|owner| owner != player) {
        reject_drag(state, connection, DragCancelReason::CannotLift);
        return false;
    }
    let origin = SettledItemLocation::Contained(contained);
    let prepared = match prepare_lift(state, connection, item, amount, origin) {
        Ok(prepared) => prepared,
        Err(error) => {
            warn!(%error, "partial lift could not be prepared");
            reject_drag(state, connection, DragCancelReason::CannotLift);
            return false;
        }
    };
    if let Some(split) = prepared.split {
        commit_split(state, split);
    }
    // Out of a container. The lifter's own client takes it out of the gump
    // itself, but anybody *else* looking in has to be told — a second viewer
    // of a chest, and both parties to a trade, where watching the other side
    // take something back is the whole point.
    note_looter(state, contained.container, player);
    tell_watchers_removed_except(state, contained.container, item.serial, Some(connection));
    commit_item_relocation(state, prepared.relocation);
    true
}

/// Lift an item off a paperdoll and remove it from other watchers' screens.
fn lift_equipped_item(
    state: &mut WorldState,
    connection: ConnectionId,
    player: EntityId,
    item: LiftableItem,
    worn: Equipped,
) {
    // Off a mobile. The picker's own client drags it off the paperdoll;
    // everyone else watching the mobile is told to forget it, because
    // they knew it only as part of that mobile.
    relocate_item(
        state,
        item.entity,
        ItemLocation::Held {
            connection,
            origin: SettledItemLocation::Equipped(worn),
        },
    )
    .expect("a lifted worn item has one cursor parent");
    if let Some(mobile) = state.registry.entity_of(worn.mobile) {
        for watcher in equip_audience(state, mobile) {
            if watcher == player {
                continue;
            }
            if let Some(&Client { connection: to, .. }) = state.registry.get::<Client>(watcher) {
                state.send_packet(to, &ServerPacket::Remove(Remove { serial: item.serial }));
            }
        }
    }
}

/// Put a client's held item down. See `Command::DropItem`.
///
/// The destination arrives already interpreted — which of the three the client
/// asked for, and its position read in the space that destination uses. Nothing
/// in this crate re-derives it from a serial, which is what used to make a
/// gump offset and a map tile the same `Point`.
pub fn drop_item(
    state: &mut WorldState,
    connection: ConnectionId,
    serial: RawSerial,
    destination: DropDestination,
) {
    let Some(held) = state.held_of(connection) else {
        // Nothing on the cursor — a stray 0x08, nothing to bounce.
        return;
    };
    // The serial has to be the thing actually held; a mismatch is a confused
    // client, and the safe answer is to give it back what it was holding.
    if state.registry.serial_of(held.entity) != serial.validate() {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    }

    let position = match destination {
        DropDestination::Ground(position) => position,
        DropDestination::Item { item, at } => {
            drop_onto_serial(state, connection, held, at, item);
            return;
        }
        // A mobile is dropped *on*, not into, and the packet's position means
        // nothing — `drop_onto_serial` reaches the equip and trade arms without
        // one, so the gump point it takes for the container case is a default
        // no arm below reads.
        DropDestination::Mobile(mobile) => {
            drop_onto_serial(state, connection, held, GumpPoint::new(0, 0), mobile);
            return;
        }
        // The client named nothing. It is still holding the item, so it is owed
        // the bounce it would get for a target it cannot reach.
        DropDestination::Nowhere => {
            bounce(state, connection, held, DragCancelReason::Other);
            return;
        }
    };

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

    let graphic = held_graphic(state, held.entity);
    place_on_ground(state, held.entity, position, state.facet_of(player));
    state.play_sound_to(
        player,
        drop_sound(graphic, amount_of(state, held.entity), SoundId(0x0042)),
    );
    debug!(serial = serial.0, "dropped on the ground");
}

/// Put a held item into a container. See `Command::DropItem`.
///
/// `at` is where in the container's gump the client let go — gump space, not
/// world tiles. The `0x08` carries both meanings in one field and
/// [`DropDestination`] is where they part company, so by the time a serial
/// reaches this function the question has already been answered and there is
/// nothing here to convert.
pub fn drop_into_container(
    state: &mut WorldState,
    connection: ConnectionId,
    held: HeldItem,
    at: GumpPoint,
    container_serial: Serial,
) {
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
    // A trade escrow is only writable by its owner.  The partner watches this
    // container, which is deliberately different from sharing permission to
    // alter its offer.
    if trade_container_owner(state, container_serial).is_some_and(|owner| owner != player) {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    }
    // The container must be in reach — on the ground near the player, or worn on
    // them (their backpack) or on a mobile beside them. A worn pack has no
    // `Position` of its own; `in_reach` handles that. Dropping into a
    // container nested in another is a later refinement.
    if !in_reach(state, container_entity, player) {
        bounce(state, connection, held, DragCancelReason::OutOfRange);
        return;
    }
    // And it must have room. ServUO's `CheckHold`, asked here rather than in
    // `drop_onto_serial` because both arms of that one land in this function —
    // the gate belongs where the item actually goes in. The item bounces back to
    // where it came from, which is what makes the refusal readable: the player
    // sees it return to their hand and reads why.
    let (plus_items, plus_weight) = crate::cost_of(state, held.entity);
    if let Some(full) = crate::check_hold(state, player, container_serial, plus_items, plus_weight) {
        state.system_message(player, full.message());
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    }
    // And, if it is a house's secure, the *house* must have room. A second
    // ceiling over the same drop, because they count different things: the
    // container's is about that container, and this is about everything the
    // house is storing between all of its secures.
    if !state.secure_has_room(container_entity, 1) {
        state.system_message(player, SECURE_FULL_MESSAGE);
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    }

    let grid = item_count(state, container_serial);
    let contained = Contained {
        container: container_serial,
        position:  at,
        grid:      GridSlot(grid),
    };
    let graphic = held_graphic(state, held.entity);
    relocate_item(state, held.entity, ItemLocation::contained(contained))
        .expect("an accepted container drop has one valid parent");
    state.play_sound_to(
        player,
        drop_sound(graphic, amount_of(state, held.entity), SoundId(0x0048)),
    );
    // Tell the client, whose gump is open, that the item is now inside.
    if let (Some(version), Some(record)) =
        (state.version_of(connection), contained_record(state, held.entity))
    {
        state.send(
            connection,
            encode_add_to_container(record, container_serial, version),
        );
    }
    // And everyone else looking into the same container, which is what makes an
    // offer visible across a trade window.
    tell_watchers_updated_except(state, container_serial, held.entity, Some(connection));
    debug!(%container_serial, "dropped into a container");
}

/// A drop onto something the client named by serial — an item *or another
/// player*: into it if it is a container, merged with it if it is an identical
/// stack, offered as a trade if it is somebody, refused otherwise.
///
/// `at` is a gump point and only the container arm reads it. A drop onto a
/// mobile has no meaningful position at all, which is why
/// [`DropDestination::Mobile`] does not carry one.
pub fn drop_onto_serial(
    state: &mut WorldState,
    connection: ConnectionId,
    held: HeldItem,
    at: GumpPoint,
    target_serial: Serial,
) {
    let target = state.registry.entity_of(target_serial);
    match target {
        Some(target) if state.registry.has::<Spellbook>(target) => {
            drop_scroll_on_book(state, connection, held, target);
        }
        Some(target) if state.registry.has::<Runebook>(target) => {
            drop_onto_runebook(state, connection, held, target);
        }
        Some(target) if state.registry.has::<Container>(target) => {
            drop_into_container(state, connection, held, at, target_serial);
        }
        Some(target) if can_stack(state, held.entity, target) => {
            merge_onto(state, connection, held, target);
        }
        // Dropping something on a *person* opens the secure trade window. Only
        // a player: a creature or a shopkeeper is a body too, and dropping on
        // one still bounces exactly as it always did.
        Some(target) if is_player(state, target) => {
            offer(state, connection, held, target);
        }
        _ => bounce(state, connection, held, DragCancelReason::Other),
    }
}

/// What a runebook accepts: a marked rune, which becomes an entry and is
/// consumed, or a Recall scroll, which recharges it.
///
/// In `items` rather than in `magic` for the reason `drop_scroll_on_book` is:
/// the components are `state`'s and consuming an item is this crate's own door.
/// What a bound destination *means* stays in `magic`, which is where casting to
/// one lives.
fn drop_onto_runebook(state: &mut WorldState, connection: ConnectionId, held: HeldItem, book: EntityId) {
    let (Some(&player), Some(book_serial)) = (state.players.get(&connection), state.registry.serial_of(book))
    else {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    };
    if !crate::in_reach(state, book, player) {
        bounce(state, connection, held, DragCancelReason::OutOfRange);
        return;
    }
    let graphic = state.registry.get::<Drawn>(held.entity).map(|g| g.id);

    // A Recall scroll recharges it — ServUO's `Runebook.OnDragDrop`.
    if graphic.and_then(scroll_spell) == Some(RECALL_SPELL) {
        recharge_runebook(state, connection, held, book, player, book_serial);
        return;
    }

    bind_rune_into_runebook(state, connection, held, book, player, book_serial);
}

fn recharge_runebook(
    state: &mut WorldState,
    connection: ConnectionId,
    held: HeldItem,
    book: EntityId,
    player: EntityId,
    book_serial: Serial,
) {
    let Some(mut owned) = state.registry.get::<Runebook>(book).cloned() else {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    };
    if owned.charges >= owned.max_charges {
        state.system_message(player, "That book is fully charged.");
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    }
    // How many of the pile the book can take. The surplus stays on the cursor
    // rather than vanishing: clamping here would eat the difference, which is
    // the shape of every quiet item-loss bug.
    let room = u32::from(owned.max_charges - owned.charges);
    let held_amount = u32::from(amount_of(state, held.entity));
    let taken = room.min(held_amount);
    owned.charges += taken as u8;
    state.registry.insert(book, owned);
    if taken >= held_amount {
        despawn_item(state, held.entity);
    } else {
        // Put the remainder back where it came from, still a pile.
        let left = u16::try_from(held_amount - taken).unwrap_or(u16::MAX);
        set_stack_amount(state, held.entity, left);
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    }
    state.system_message(player, "You recharge the book.");
    tell_watchers_updated(state, book_serial, book);
}

fn bind_rune_into_runebook(
    state: &mut WorldState,
    connection: ConnectionId,
    held: HeldItem,
    book: EntityId,
    player: EntityId,
    book_serial: Serial,
) {
    // A marked rune becomes an entry, and the rune itself is consumed — ServUO
    // deletes it, which is why the entry carries its own description rather than
    // pointing back at a rune that will not be there.
    let Some(&mark) = state.registry.get::<RuneMark>(held.entity) else {
        state.system_message(player, "You can only place marked runes in a runebook.");
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    };
    let Some(mut owned) = state.registry.get::<Runebook>(book).cloned() else {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    };
    if owned.entries.len() >= RUNEBOOK_ENTRIES {
        state.system_message(player, "That book is full.");
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    }
    let description = state
        .registry
        .get::<Name>(held.entity)
        .map_or_else(|| "an unknown place".to_owned(), |name| name.0.clone());
    owned.entries.push(RunebookEntry {
        facet: mark.facet,
        destination: mark.destination,
        description,
    });
    if owned.default_entry.is_none() {
        owned.default_entry = Some(0);
    }
    state.registry.insert(book, owned);
    despawn_item(state, held.entity);
    state.system_message(player, "You bind the rune into the book.");
    tell_watchers_updated(state, book_serial, book);
}

/// Recall's spell id — what a scroll has to be to recharge a runebook.
const RECALL_SPELL: SpellId = SpellId(31);

/// A Magery scroll dropped on a spellbook is learned into it and spent. A
/// non-scroll, a book out of reach, or a spell the book already holds bounces
/// back — no scroll is wasted on a spell you have.
fn drop_scroll_on_book(state: &mut WorldState, connection: ConnectionId, held: HeldItem, book: EntityId) {
    let spell = state
        .registry
        .get::<Drawn>(held.entity)
        .and_then(|g| scroll_spell(g.id));
    let (Some(spell), Some(&player), Some(book_serial)) = (
        spell,
        state.players.get(&connection),
        state.registry.serial_of(book),
    ) else {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    };
    if !crate::in_reach(state, book, player) {
        bounce(state, connection, held, DragCancelReason::OutOfRange);
        return;
    }
    let mut mask = state.registry.get::<Spellbook>(book).copied().unwrap_or_default();
    if mask.has(spell) {
        // Already in the book — keep the scroll rather than burn it for nothing.
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    }
    mask.learn(spell);
    state.registry.insert(book, mask);
    despawn_item(state, held.entity);
    // Refresh the open book so the new spell appears at once.
    state.send_packet(
        connection,
        &ServerPacket::SpellbookContent(SpellbookContent {
            serial:  book_serial,
            graphic: SPELLBOOK_GRAPHIC,
            offset:  1,
            content: mask.0,
        }),
    );
    debug!(spell = spell.0, "learned a spell from a scroll");
}

/// Put a held item back where it was lifted and tell the client the drag is
/// off, so it stops showing the item on the cursor.
pub fn bounce(state: &mut WorldState, connection: ConnectionId, held: HeldItem, reason: DragCancelReason) {
    restore(state, held);
    reject_drag(state, connection, reason);
}

/// Put a held item back exactly where it came from — the ground it lay on or
/// the container it was in.
pub fn restore(state: &mut WorldState, held: HeldItem) {
    // A trusted command can consume an item while its drag is in flight. That
    // path clears the cursor before despawning it, but keep recovery total for
    // old/stale state too: never try to attach location components to an entity
    // that no longer exists.
    if !state.registry.contains(held.entity) {
        return;
    }
    match held.origin {
        Origin::Ground { position, facet } => {
            place_on_ground(state, held.entity, position, facet);
        }
        Origin::Container(contained) => {
            relocate_item(state, held.entity, ItemLocation::contained(contained))
                .expect("a bounced item returns to its one container parent");
        }
        Origin::Worn(worn) => {
            relocate_item(state, held.entity, ItemLocation::equipped(worn))
                .expect("a bounced item returns to its one paperdoll parent");
            // Back on the mobile, and back on every screen that shows it.
            if let Some(mobile) = state.registry.entity_of(worn.mobile) {
                broadcast_equip(state, held.entity, mobile);
            }
        }
    }
}

/// Remember that `taker` took something out of `container`, if the container is a
/// corpse — ServUO's `Corpse.Looters`, which Forensic Evaluation reads back out.
///
/// Only a corpse keeps the list: an ordinary chest has no story, and a shard that
/// logged who opened every crate would grow a list nothing ever reads. A name is
/// recorded once, however many items are taken, and it is a *name* for the same
/// reason the killer is — the corpse outlives the session.
fn note_looter(state: &mut WorldState, container: Serial, taker: EntityId) {
    let Some(container) = state.registry.entity_of(container) else {
        return;
    };
    let Some(story) = state.registry.get::<Corpse>(container) else {
        return; // not a corpse: nothing keeps a guest list
    };
    let Some(name) = state.registry.get::<Name>(taker).map(|n| n.0.clone()) else {
        return;
    };
    if story.looters.contains(&name) {
        return;
    }
    let mut story = story.clone();
    story.looters.push(name);
    state.registry.insert(container, story);
}

/// Send a `0x27`, cancelling whatever drag the client thinks it has.
pub fn reject_drag(state: &mut WorldState, connection: ConnectionId, reason: DragCancelReason) {
    state.send_packet(connection, &ServerPacket::DragCancel(DragCancel { reason }));
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use openshard_protocol::access::AccessLevel;
    use openshard_protocol::identity::AccountName;
    use openshard_protocol::item_kind::{
        ItemKindId,
        MaterialId,
    };
    use openshard_protocol::serial::{
        ITEM_MAX,
        RawSerial,
    };
    use openshard_protocol::version::ClientVersion;
    use openshard_state::FacetState;
    use openshard_state::connection::Connection;
    use proptest::prelude::*;

    use super::*;

    fn world() -> WorldState {
        let tiles = openshard_tiles::TileData::empty();
        let mut facets = BTreeMap::new();
        facets.insert(
            Facet(0),
            FacetState::new(
                None,
                None,
                64,
                64,
                openshard_state::facet_rules::FacetRules::classic(Facet(0)),
                None,
                &tiles,
            ),
        );
        WorldState::new(
            facets,
            Facet(0),
            tiles,
            Default::default(),
            openshard_map::grid::Tile::new(0, 0),
            1,
        )
    }

    fn connected_player(state: &mut WorldState, at: Point) -> (ConnectionId, EntityId) {
        let connection = ConnectionId::from_raw(7);
        let (player, _) = state
            .registry
            .spawn_with_serial(SerialKind::Mobile)
            .expect("a mobile serial");
        state.registry.insert(
            player,
            Body {
                id:  Graphic(0x0190),
                hue: Hue(0),
            },
        );
        state.registry.insert(player, Position(at));
        state.registry.insert(player, Facet(0));
        state.connections.insert(
            connection,
            Connection::new(
                ClientVersion::TOL,
                AccountName("split-test".to_owned()),
                AccessLevel::Player,
            ),
        );
        state.players.insert(connection, player);
        (connection, player)
    }

    fn stack(state: &mut WorldState, drawn: Drawn, amount: u16) -> (EntityId, Serial) {
        let (item, serial) = state
            .registry
            .spawn_with_serial(SerialKind::Item)
            .expect("an item serial");
        state.registry.insert(item, drawn);
        state.registry.insert(item, Stackable);
        set_stack_amount(state, item, amount);
        (item, serial)
    }

    fn exhaust_item_serials(state: &mut WorldState) {
        state
            .registry
            .reserve_serial(Serial::new(ITEM_MAX).expect("the final item serial"));
    }

    prop_compose! {
        fn split_case()(total in 2u16..=MAX_STACK)
            (taken in 1u16..total, total in Just(total)) -> (u16, u16) {
            (total, taken)
        }
    }

    #[test]
    #[should_panic(expected = "an item on a cursor must have art")]
    fn a_held_entity_without_art_is_not_silently_treated_as_graphic_zero() {
        let mut state = world();
        let entity = state.registry.spawn();
        held_graphic(&state, entity);
    }

    #[test]
    fn ground_split_refuses_without_quantity_or_location_loss_when_serials_are_exhausted() {
        let mut state = world();
        let at = Point::new(10, 10, 0);
        let (connection, player) = connected_player(&mut state, at);
        let (item, serial) = stack(
            &mut state,
            Drawn {
                id:  Graphic(0x1BF2),
                hue: Hue(0x08AB),
            },
            10,
        );
        state.registry.insert(item, ItemKind(ItemKindId(1)));
        state.registry.insert(item, Material(MaterialId(9)));
        establish_item_location(&mut state, item, ItemLocation::ground(Facet(0), at)).unwrap();
        state.place_item(Facet(0), item, at);
        state.seen.entry(player).or_default().insert(item);
        exhaust_item_serials(&mut state);

        pick_up(&mut state, connection, RawSerial(serial.raw()), 4);

        assert_eq!(amount_of(&state, item), 10);
        assert_eq!(
            item_location(&state, item),
            Some(ItemLocation::ground(Facet(0), at))
        );
        assert_eq!(state.held_of(connection), None);
        assert!(state.seen.get(&player).is_some_and(|seen| seen.contains(&item)));
        assert_eq!(state.registry.query::<Drawn>().count(), 1);
        assert!(openshard_state::audit_item_graph(&state).is_empty());
    }

    #[test]
    fn contained_split_refuses_without_quantity_or_membership_loss_when_serials_are_exhausted() {
        let mut state = world();
        let at = Point::new(10, 10, 0);
        let (connection, _) = connected_player(&mut state, at);
        let (container, container_serial) = stack(
            &mut state,
            Drawn {
                id:  Graphic(0x0E75),
                hue: Hue(0),
            },
            1,
        );
        state.registry.insert(
            container,
            Container {
                gump: Graphic(0x003C),
            },
        );
        establish_item_location(&mut state, container, ItemLocation::ground(Facet(0), at)).unwrap();
        let (item, serial) = stack(
            &mut state,
            Drawn {
                id:  GOLD_GRAPHIC,
                hue: Hue(0),
            },
            10,
        );
        let contained = Contained {
            container: container_serial,
            position:  GumpPoint::new(30, 40),
            grid:      GridSlot(2),
        };
        establish_item_location(&mut state, item, ItemLocation::contained(contained)).unwrap();
        exhaust_item_serials(&mut state);

        pick_up(&mut state, connection, RawSerial(serial.raw()), 4);

        assert_eq!(amount_of(&state, item), 10);
        assert_eq!(
            item_location(&state, item),
            Some(ItemLocation::contained(contained))
        );
        assert_eq!(state.held_of(connection), None);
        assert_eq!(contained_items(&state, container_serial).count(), 1);
        assert_eq!(state.registry.query::<Drawn>().count(), 2);
        assert!(openshard_state::audit_item_graph(&state).is_empty());
    }

    #[test]
    fn a_typed_ground_split_copies_identity_to_its_remainder() {
        let mut state = world();
        let at = Point::new(10, 10, 0);
        let (connection, _) = connected_player(&mut state, at);
        let (item, serial) = stack(
            &mut state,
            Drawn {
                id:  Graphic(0x1BF2),
                hue: Hue(0x08AB),
            },
            10,
        );
        state.registry.insert(item, ItemKind(ItemKindId(1)));
        state.registry.insert(item, Material(MaterialId(9)));
        establish_item_location(&mut state, item, ItemLocation::ground(Facet(0), at)).unwrap();
        state.place_item(Facet(0), item, at);

        pick_up(&mut state, connection, RawSerial(serial.raw()), 4);

        let (remainder, _) = state
            .registry
            .query::<ItemLocation>()
            .find(|(candidate, location)| {
                *candidate != item && **location == ItemLocation::ground(Facet(0), at)
            })
            .expect("the remainder stays on the ground");
        assert_eq!(amount_of(&state, item), 4);
        assert_eq!(amount_of(&state, remainder), 6);
        assert_eq!(
            state.registry.get::<ItemKind>(remainder),
            Some(&ItemKind(ItemKindId(1)))
        );
        assert_eq!(
            state.registry.get::<Material>(remainder),
            Some(&Material(MaterialId(9)))
        );
        assert!(openshard_state::audit_item_graph(&state).is_empty());
    }

    #[test]
    fn a_legacy_contained_split_keeps_its_exact_presentation() {
        let mut state = world();
        let at = Point::new(10, 10, 0);
        let (connection, _) = connected_player(&mut state, at);
        let (container, container_serial) = stack(
            &mut state,
            Drawn {
                id:  Graphic(0x0E75),
                hue: Hue(0),
            },
            1,
        );
        state.registry.insert(
            container,
            Container {
                gump: Graphic(0x003C),
            },
        );
        establish_item_location(&mut state, container, ItemLocation::ground(Facet(0), at)).unwrap();
        let drawn = Drawn {
            id:  Graphic(0x2222),
            hue: Hue(0x0444),
        };
        let (item, serial) = stack(&mut state, drawn, 10);
        let contained = Contained {
            container: container_serial,
            position:  GumpPoint::new(30, 40),
            grid:      GridSlot(2),
        };
        establish_item_location(&mut state, item, ItemLocation::contained(contained)).unwrap();

        pick_up(&mut state, connection, RawSerial(serial.raw()), 4);

        let (remainder, _) = contained_items(&state, container_serial)
            .next()
            .expect("the remainder stays in the container");
        assert_ne!(remainder, item);
        assert_eq!(amount_of(&state, item), 4);
        assert_eq!(amount_of(&state, remainder), 6);
        assert_eq!(state.registry.get::<Drawn>(remainder), Some(&drawn));
        assert!(!state.registry.has::<ItemKind>(remainder));
        assert!(!state.registry.has::<Material>(remainder));
        assert!(openshard_state::audit_item_graph(&state).is_empty());
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn partial_ground_lifts_conserve_quantity((total, taken) in split_case()) {
            let mut state = world();
            let at = Point::new(10, 10, 0);
            let (connection, _) = connected_player(&mut state, at);
            let (item, serial) = stack(
                &mut state,
                Drawn {
                    id: GOLD_GRAPHIC,
                    hue: Hue(0),
                },
                total,
            );
            establish_item_location(&mut state, item, ItemLocation::ground(Facet(0), at)).unwrap();
            state.place_item(Facet(0), item, at);

            pick_up(&mut state, connection, RawSerial(serial.raw()), taken);

            let (remainder, _) = state
                .registry
                .query::<ItemLocation>()
                .find(|(candidate, location)| {
                    *candidate != item && **location == ItemLocation::ground(Facet(0), at)
                })
                .expect("a partial lift leaves one remainder");
            prop_assert_eq!(amount_of(&state, item), taken);
            prop_assert_eq!(amount_of(&state, remainder), total - taken);
            prop_assert_eq!(
                u32::from(amount_of(&state, item)) + u32::from(amount_of(&state, remainder)),
                u32::from(total),
            );
            prop_assert_eq!(state.held_of(connection).map(|held| held.entity), Some(item));
            prop_assert!(openshard_state::audit_item_graph(&state).is_empty());
        }
    }
}
