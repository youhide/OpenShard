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
    // The seam, and a refusal rather than silence: a lift the server will not
    // do is answered with a `0x27`, or the client keeps the item on its cursor
    // for ever.
    let Some(item_serial) = serial.validate() else {
        reject_drag(state, connection, DragCancelReason::CannotLift);
        return;
    };
    let Some(item) = state.registry.entity_of(item_serial) else {
        reject_drag(state, connection, DragCancelReason::CannotLift);
        return;
    };
    // Only a thing with a graphic is an item. A mobile has none, so this
    // rejects trying to pick up a person.
    if !state.registry.has::<Drawn>(item) {
        reject_drag(state, connection, DragCancelReason::CannotLift);
        return;
    }
    // A town's fittings are not loot: script-placed decoration cannot be lifted.
    if state.registry.has::<Decoration>(item) {
        reject_drag(state, connection, DragCancelReason::CannotLift);
        return;
    }
    // Nor is the trade window itself, which is a worn container and would
    // otherwise lift like any other — ServUO's `CheckLift` refusing outright.
    // What is *inside* it lifts normally; that is how an offer is taken back.
    if state.registry.has::<TradeWindow>(item) {
        reject_drag(state, connection, DragCancelReason::CannotLift);
        return;
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
        return;
    }
    // Nor is somebody's hair. See `FIXED_LAYERS`.
    if state
        .registry
        .get::<Equipped>(item)
        .is_some_and(|worn| FIXED_LAYERS.contains(&worn.layer))
    {
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
        let held = HeldItem {
            entity: item,
            origin: Origin::Ground {
                position: item_pos,
                facet,
            },
        };
        if state.hold(connection, held).is_err() {
            reject_drag(state, connection, DragCancelReason::AlreadyHolding);
            return;
        }
        // Taking part of a stack: leave the remainder behind as a new pile
        // and lift the original, now reduced to what was taken. The original
        // keeps its serial and goes to the cursor — the client's drag and its
        // eventual drop still name it — so only the leftover is a new object.
        let total = amount_of(state, item);
        let stackable = state.registry.has::<Stackable>(item)
            || state
                .registry
                .get::<Drawn>(item)
                .is_some_and(|drawn| drawn.id == GOLD_GRAPHIC);
        if amount > 0 && amount < total && stackable {
            // Normalise legacy gold while it participates, so the correction
            // survives persistence and every later stack operation.
            state.registry.insert(item, Stackable);
            spawn_leftover(state, item, total - amount, item_pos, facet);
            set_stack_amount(state, item, amount);
        }
        // Off the sector grid, off every screen but the picker's — whose own
        // client already put it on the cursor, so a 0x1D there would fight it.
        state.unplace(facet, item);
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
    } else if let Some(&contained) = state.registry.get::<Contained>(item) {
        // Both halves of a secure trade are visible to both players, but only
        // the owner of a half may remove its contents.  Visibility is not
        // authority: otherwise a partner could lift an offered item straight
        // into their own pack before either checkbox was ticked.
        if trade_container_owner(state, contained.container).is_some_and(|owner| owner != player) {
            reject_drag(state, connection, DragCancelReason::CannotLift);
            return;
        }
        if state
            .hold(
                connection,
                HeldItem {
                    entity: item,
                    origin: Origin::Container(contained),
                },
            )
            .is_err()
        {
            reject_drag(state, connection, DragCancelReason::AlreadyHolding);
            return;
        }
        // Taking part of a stack out of a container: leave the remainder behind
        // in the same slot as a new pile and lift the original, reduced to what
        // was taken — the ground split's `UnStackSplit`, but the leftover stays
        // contained rather than dropping to the floor.
        let total = amount_of(state, item);
        let stackable = state.registry.has::<Stackable>(item)
            || state
                .registry
                .get::<Drawn>(item)
                .is_some_and(|drawn| drawn.id == GOLD_GRAPHIC);
        if amount > 0 && amount < total && stackable {
            state.registry.insert(item, Stackable);
            spawn_contained_leftover(state, item, total - amount, contained);
            set_stack_amount(state, item, amount);
        }
        // Out of a container. The lifter's own client takes it out of the gump
        // itself, but anybody *else* looking in has to be told — a second viewer
        // of a chest, and both parties to a trade, where watching the other side
        // take something back is the whole point.
        note_looter(state, contained.container, player);
        tell_watchers_removed_except(state, contained.container, item_serial, Some(connection));
        state.registry.remove::<Contained>(item);
    } else if let Some(&worn) = state.registry.get::<Equipped>(item) {
        if state
            .hold(
                connection,
                HeldItem {
                    entity: item,
                    origin: Origin::Worn(worn),
                },
            )
            .is_err()
        {
            reject_drag(state, connection, DragCancelReason::AlreadyHolding);
            return;
        }
        // Off a mobile. The picker's own client drags it off the paperdoll;
        // everyone else watching the mobile is told to forget it, because
        // they knew it only as part of that mobile.
        state.registry.remove::<Equipped>(item);
        if let Some(mobile) = state.registry.entity_of(worn.mobile) {
            for watcher in equip_audience(state, mobile) {
                if watcher == player {
                    continue;
                }
                if let Some(&Client { connection: to, .. }) = state.registry.get::<Client>(watcher) {
                    state.send_packet(to, &ServerPacket::Remove(Remove { serial: item_serial }));
                }
            }
        }
    } else {
        // Neither on the ground nor in a container: already on a cursor, or
        // nowhere. Nothing to lift.
        reject_drag(state, connection, DragCancelReason::CannotLift);
        return;
    }
    // Reaching for something gives you away and breaks concentration — ServUO
    // calls `DisruptiveAction` from `Mobile.Lift` and from both `Use` overloads,
    // and a thief who could loot from hiding would never need Stealing at all.
    state.break_cover(player);
    debug!(%item_serial, "lifted onto the cursor");
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

    state.take_held(connection);
    place_on_ground(state, held.entity, position, state.facet_of(player));
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
    state.take_held(connection);
    state.registry.insert(
        held.entity,
        Contained {
            container: container_serial,
            position: at,
            grid: GridSlot(grid),
        },
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
        let Some(mut owned) = state.registry.get::<Runebook>(book).cloned() else {
            bounce(state, connection, held, DragCancelReason::Other);
            return;
        };
        if owned.charges >= owned.max_charges {
            state.system_message(player, "That book is fully charged.");
            bounce(state, connection, held, DragCancelReason::Other);
            return;
        }
        // How many of the pile the book can take. The surplus stays on the
        // cursor rather than vanishing: clamping here would eat the difference,
        // which is the shape of every quiet item-loss bug.
        let room = u32::from(owned.max_charges - owned.charges);
        let held_amount = u32::from(state.registry.get::<Amount>(held.entity).map_or(1, |a| a.0));
        let taken = room.min(held_amount);
        owned.charges += taken as u8;
        state.registry.insert(book, owned);
        if taken >= held_amount {
            state.take_held(connection);
            state.registry.despawn(held.entity);
        } else {
            // Put the remainder back where it came from, still a pile.
            let left = u16::try_from(held_amount - taken).unwrap_or(u16::MAX);
            state.registry.insert(held.entity, Amount(left));
            bounce(state, connection, held, DragCancelReason::Other);
            return;
        }
        state.system_message(player, "You recharge the book.");
        tell_watchers_updated(state, book_serial, book);
        return;
    }

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
    state.take_held(connection);
    state.registry.despawn(held.entity);
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
    state.take_held(connection);
    state.registry.despawn(held.entity);
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
    state.take_held(connection);
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
