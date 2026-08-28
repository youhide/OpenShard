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
    entity: EntityId,
    serial: Serial,
    location: ItemLocation,
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
    let held = HeldItem {
        entity: item.entity,
        origin: Origin::Ground { position, facet },
    };
    // Taking part of a stack: leave the remainder behind as a new pile and lift
    // the original, now reduced to what was taken. The original keeps its serial
    // and goes to the cursor — the client's drag and its eventual drop still name
    // it — so only the leftover is a new object.
    let total = amount_of(state, item.entity);
    let stackable = state.registry.has::<Stackable>(item.entity)
        || state
            .registry
            .get::<Drawn>(item.entity)
            .is_some_and(|drawn| intrinsically_stackable(drawn.id));
    if amount > 0 && amount < total && stackable {
        // Normalise legacy gold while it participates, so the correction
        // survives persistence and every later stack operation.
        state.registry.insert(item.entity, Stackable);
        spawn_leftover(state, item.entity, total - amount, position, facet);
        set_stack_amount(state, item.entity, amount);
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
    relocate_item(
        state,
        item.entity,
        ItemLocation::Held {
            connection,
            origin: settled_from_origin(held.origin),
        },
    )
    .expect("a lifted ground item has one cursor parent");
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
    // Taking part of a stack out of a container: leave the remainder behind in
    // the same slot as a new pile and lift the original, reduced to what was
    // taken — the ground split's `UnStackSplit`, but the leftover stays contained
    // rather than dropping to the floor.
    let total = amount_of(state, item.entity);
    let stackable = state.registry.has::<Stackable>(item.entity)
        || state
            .registry
            .get::<Drawn>(item.entity)
            .is_some_and(|drawn| intrinsically_stackable(drawn.id));
    if amount > 0 && amount < total && stackable {
        state.registry.insert(item.entity, Stackable);
        spawn_contained_leftover(state, item.entity, total - amount, contained);
        set_stack_amount(state, item.entity, amount);
    }
    // Out of a container. The lifter's own client takes it out of the gump
    // itself, but anybody *else* looking in has to be told — a second viewer
    // of a chest, and both parties to a trade, where watching the other side
    // take something back is the whole point.
    note_looter(state, contained.container, player);
    tell_watchers_removed_except(state, contained.container, item.serial, Some(connection));
    relocate_item(
        state,
        item.entity,
        ItemLocation::Held {
            connection,
            origin: SettledItemLocation::Contained(contained),
        },
    )
    .expect("a lifted contained item has one cursor parent");
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

    place_on_ground(state, held.entity, position, state.facet_of(player));
    let graphic = state
        .registry
        .get::<Drawn>(held.entity)
        .map_or(Graphic(0), |drawn| drawn.id);
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
        position: at,
        grid: GridSlot(grid),
    };
    relocate_item(state, held.entity, ItemLocation::contained(contained))
        .expect("an accepted container drop has one valid parent");
    let graphic = state
        .registry
        .get::<Drawn>(held.entity)
        .map_or(Graphic(0), |drawn| drawn.id);
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
    let held_amount = u32::from(state.registry.get::<Amount>(held.entity).map_or(1, |a| a.0));
    let taken = room.min(held_amount);
    owned.charges += taken as u8;
    state.registry.insert(book, owned);
    if taken >= held_amount {
        despawn_item(state, held.entity);
    } else {
        // Put the remainder back where it came from, still a pile.
        let left = u16::try_from(held_amount - taken).unwrap_or(u16::MAX);
        state.registry.insert(held.entity, Amount(left));
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
            serial: book_serial,
            graphic: SPELLBOOK_GRAPHIC,
            offset: 1,
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
