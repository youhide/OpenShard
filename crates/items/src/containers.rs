use super::*;

/// Handle a double-click. See `Command::DoubleClick`.
///
/// A door toggles open or shut; a container opens its gump; a mobile shows its
/// paperdoll. Any other item is handed to the pack as an [`ItemUsed`] trigger,
/// keyed by graphic — the engine has no default "use" for a bare item, so a
/// shard gives it one; without a pack the double-click is simply silent.
pub fn double_click(state: &mut WorldState, connection: ConnectionId, serial: u32) {
    let Some(&player) = state.players.get(&connection) else {
        return;
    };
    let Some(target_serial) = Serial::new(serial) else {
        return;
    };
    let Some(target) = state.registry.entity_of(target_serial) else {
        return;
    };

    // A door toggles; a container opens its gump; a mobile shows its paperdoll;
    // anything else is a "use" rule not written yet, and a wrong guess is worse
    // than silence. A door is checked before Container because it is neither — it
    // is its own interaction.
    if state.registry.has::<Door>(target) {
        toggle_door(state, player, target, target_serial);
    } else if state.registry.has::<Spellbook>(target) {
        open_spellbook(state, connection, player, target, target_serial);
    } else if state.registry.has::<Container>(target) {
        open_container(state, connection, player, target, target_serial);
    } else if target == player && state.registry.has::<Riding>(player) {
        // A raw self-double-click in the saddle is the dismount, war mode or
        // peace — ServUO's `Mobile.OnDoubleClick`. The paperdoll-open the client
        // sends at login never lands here: it carries bit 31 and is routed to
        // [`paperdoll_request`] before this function is called.
        dismount(state, player);
    } else if try_mount(state, player, target, target_serial) {
        // A rideable, riderless creature in reach: the double-click was a leg
        // over the saddle, not a paperdoll request.
    } else if state.registry.has::<Body>(target) {
        open_paperdoll(state, connection, player, target, target_serial);
    } else {
        // Not a door, container, spellbook, mount or mobile: an ordinary item.
        // Hand its "use" to the pack, keyed by graphic — Sphere's @DClick. The
        // engine has no default behaviour for a bare item, so this is silent
        // until a shard's script gives the graphic a meaning.
        item_used(state, player, target, target_serial);
    }
}

/// Answer a `0x06` with bit 31 set — the client's *paperdoll request*, sent by
/// the paperdoll macro and on login. ServUO's `UseReq` routes this straight to
/// `OnPaperdollRequest`, never to `Use`: it opens the paperdoll and does nothing
/// else — above all it does not dismount a mounted rider, which is exactly what
/// treating it as a raw self-double-click used to do.
pub fn paperdoll_request(state: &mut WorldState, connection: ConnectionId, serial: u32) {
    let Some(&player) = state.players.get(&connection) else {
        return;
    };
    let Some(target_serial) = Serial::new(serial) else {
        return;
    };
    let Some(target) = state.registry.entity_of(target_serial) else {
        return;
    };
    if state.registry.has::<Body>(target) {
        open_paperdoll(state, connection, player, target, target_serial);
    }
}

/// Open a spellbook: draw it as a book (`0x24` with the `0xFFFF` gump) and send
/// the client the spells it holds (`0xBF 0x1B`), so the spell circles fill in.
/// A book carried in the pack is in reach; one on the ground within `ITEM_REACH`.
pub(crate) fn open_spellbook(
    state: &mut WorldState,
    connection: ConnectionId,
    player: EntityId,
    book: EntityId,
    book_serial: Serial,
) {
    if !container_in_reach(state, book, player) {
        return;
    }
    let Some(&Client { version, .. }) = state.registry.get::<Client>(player) else {
        return;
    };
    let mask = state.registry.get::<Spellbook>(book).map_or(0, |b| b.0);
    state.send(
        connection,
        // The gump `0xFFFF` is what tells the client this container is a book.
        encode_open_container(book_serial.raw(), 0xFFFF, version),
    );
    state.send(
        connection,
        // Magery spells start at offset 1; the mask's bit `n` is spell `n`.
        encode_spellbook_content(book_serial.raw(), SPELLBOOK_GRAPHIC, 1, mask),
    );
}

/// Open the container a player wears at `layer` — its backpack, or its bank box.
///
/// The service path a banker uses: find the worn container and open it onto the
/// player's own client, the same `0x24`/`0x3C` a double-click sends. Does nothing
/// if the player wears no container there.
pub fn open_worn_container(
    state: &mut WorldState,
    connection: ConnectionId,
    player: EntityId,
    layer: u8,
) {
    let Some(mobile) = state.registry.serial_of(player) else {
        return;
    };
    let worn = state
        .registry
        .query::<Equipped>()
        .find(|(item, eq)| {
            eq.mobile == mobile && eq.layer == layer && state.registry.has::<Container>(*item)
        })
        .map(|(item, _)| item);
    if let Some(item) = worn {
        if let Some(serial) = state.registry.serial_of(item) {
            open_container(state, connection, player, item, serial);
        }
    }
}

/// Open a container onto the acting client, if it may reach it.
///
/// The container is reachable when it is on the ground within [`ITEM_REACH`], or
/// worn on the player itself (its backpack), or worn on another mobile in reach.
/// A worn container has no `Position` of its own — its wearer's stands in.
pub(crate) fn open_container(
    state: &mut WorldState,
    connection: ConnectionId,
    player: EntityId,
    container: EntityId,
    container_serial: Serial,
) {
    let Some(&Container { gump }) = state.registry.get::<Container>(container) else {
        return;
    };
    if !container_in_reach(state, container, player) {
        return;
    }

    let Some(&Client { version, .. }) = state.registry.get::<Client>(player) else {
        return;
    };
    let contents = contents_of(state, container_serial);
    state.send(
        connection,
        encode_open_container(container_serial.raw(), gump, version),
    );
    state.send(
        connection,
        encode_container_contents(container_serial.raw(), &contents, version),
    );
    // Remember it is open, so a later change to its contents can be pushed here.
    state
        .open_containers
        .entry(container_serial)
        .or_default()
        .insert(connection);
    debug!(
        %container_serial,
        gump = format!("0x{gump:04X}"),
        items = contents.len(),
        "container opened"
    );
}

/// Whether `player` may reach `container` to open it or drop into it.
///
/// A container sits in one of two places, and the reach check has to handle both:
/// on the ground it stands on its own tile, and worn it has no `Position` of its
/// own — its wearer's tile stands in. Your own backpack (worn on you) is always in
/// reach; another mobile's worn container is reachable only within [`ITEM_REACH`]
/// of that mobile, on the same facet. The whole reason a worn backpack could not be
/// opened or filled before this: its reach was measured against a `Position` it
/// does not have.
pub(crate) fn container_in_reach(
    state: &WorldState,
    container: EntityId,
    player: EntityId,
) -> bool {
    let Some(&Position(player_pos)) = state.registry.get::<Position>(player) else {
        return false;
    };
    // Where the container effectively is: its own ground tile, or its wearer's.
    let anchor = if let Some(&Position(pos)) = state.registry.get::<Position>(container) {
        Some((state.facet_of(container), pos))
    } else if let Some(&Equipped { mobile, .. }) = state.registry.get::<Equipped>(container) {
        if Some(mobile) == state.registry.serial_of(player) {
            return true; // one's own worn pack is always in reach
        }
        state.registry.entity_of(mobile).and_then(|wearer| {
            Some((
                state.facet_of(wearer),
                state.registry.get::<Position>(wearer)?.0,
            ))
        })
    } else if let Some(&Contained {
        container: outer, ..
    }) = state.registry.get::<Contained>(container)
    {
        // Nested — a spellbook in the pack, a bag in a bag: in reach when the
        // container holding it is. Recurse to that one's own reach test.
        return state
            .registry
            .entity_of(outer)
            .is_some_and(|outer| container_in_reach(state, outer, player));
    } else {
        None
    };
    let Some((facet, at)) = anchor else {
        return false;
    };
    facet == state.facet_of(player) && in_range(at, player_pos, ITEM_REACH)
}

/// Send the acting client a mobile's paperdoll — the reply to double-clicking a
/// mobile. The `can lift` bit is set for one's own, so the client lets you drag
/// your own equipment off it.
pub(crate) fn open_paperdoll(
    state: &mut WorldState,
    connection: ConnectionId,
    player: EntityId,
    mobile: EntityId,
    mobile_serial: Serial,
) {
    let name = state
        .registry
        .get::<Name>(mobile)
        .map_or(String::new(), |n| n.0.clone());
    let mut flags = 0u8;
    if state
        .registry
        .get::<Combat>(mobile)
        .is_some_and(|combat| combat.warmode)
    {
        flags |= PAPERDOLL_WARMODE;
    }
    if mobile == player {
        flags |= PAPERDOLL_CAN_LIFT;
    }
    state.send(
        connection,
        encode_open_paperdoll(mobile_serial.raw(), &name, flags),
    );
    debug!(%mobile_serial, "paperdoll opened");
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

/// Put `amount` of an item into a container by decree — a vendor handing over
/// goods, a sale paying out gold. Merges onto an existing stackable pile of the
/// same art and hue; otherwise a fresh stackable item appears. Everyone with
/// the container open sees the change. Returns the pile touched, or `None`
/// when the serial pool is dry.
pub fn give(
    state: &mut WorldState,
    container: Serial,
    graphic: u16,
    hue: u16,
    amount: u16,
) -> Option<EntityId> {
    if amount == 0 {
        return None;
    }
    // A spellbook is a single item, not a stack, and carries its (empty) contents
    // — the behaviour a bought or spawned spellbook needs to be a real book. A
    // full book is dealt out elsewhere (a staff command); one off the shelf is
    // blank until scrolls fill it.
    if graphic == SPELLBOOK_GRAPHIC {
        let Ok((entity, _serial)) = state.registry.spawn_with_serial(SerialKind::Item) else {
            warn!("out of item serials; nothing given");
            return None;
        };
        state.registry.insert(entity, Graphic { id: graphic, hue });
        state.registry.insert(
            entity,
            Contained {
                container,
                x: 60,
                y: 60,
                grid: 0,
            },
        );
        state.registry.insert(entity, Spellbook::default());
        tell_watchers_updated(state, container, entity);
        return Some(entity);
    }
    let existing = state
        .registry
        .query::<Contained>()
        .filter(|(_, held)| held.container == container)
        .find(|(entity, _)| {
            state.registry.has::<Stackable>(*entity)
                && state
                    .registry
                    .get::<Graphic>(*entity)
                    .is_some_and(|g| g.id == graphic && g.hue == hue)
        })
        .map(|(entity, _)| entity);
    if let Some(pile) = existing {
        let total = amount_of(state, pile).saturating_add(amount);
        state.registry.insert(pile, Amount(total));
        tell_watchers_updated(state, container, pile);
        return Some(pile);
    }
    let Ok((entity, _serial)) = state.registry.spawn_with_serial(SerialKind::Item) else {
        warn!("out of item serials; nothing given");
        return None;
    };
    state.registry.insert(entity, Graphic { id: graphic, hue });
    state.registry.insert(
        entity,
        Contained {
            container,
            x: 60,
            y: 60,
            grid: 0,
        },
    );
    state.registry.insert(entity, Amount(amount));
    state.registry.insert(entity, Stackable);
    tell_watchers_updated(state, container, entity);
    Some(entity)
}

/// Take `amount` off a contained stack by decree — stock sold out of a
/// vendor's crate, goods sold out of a player's pack. Returns how many were
/// actually taken; a stack that reaches zero is despawned and forgotten by
/// everyone watching the container.
pub fn remove_from_stack(
    state: &mut WorldState,
    container: Serial,
    item: EntityId,
    amount: u16,
) -> u16 {
    let have = amount_of(state, item);
    let take = have.min(amount);
    if take == 0 {
        return 0;
    }
    if take == have {
        if let Some(serial) = state.registry.serial_of(item) {
            tell_watchers_removed(state, container, serial);
        }
        state.registry.despawn(item);
    } else {
        state.registry.insert(item, Amount(have - take));
        tell_watchers_updated(state, container, item);
    }
    take
}

/// Tell every client with `container` open that `item` has left it — a `0x1D`,
/// the same "forget that" the interest system draws with, so a reagent consumed
/// out of an open pack disappears from the gump live.
pub(crate) fn tell_watchers_removed(state: &mut WorldState, container: Serial, item: Serial) {
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
pub(crate) fn tell_watchers_updated(state: &mut WorldState, container: Serial, entity: EntityId) {
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
