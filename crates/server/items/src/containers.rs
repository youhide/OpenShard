use super::*;
use openshard_protocol::item_kind::{ItemKindId, MaterialId};
use openshard_protocol::wire::{Graphic, Hue};
use openshard_state::item_definition::item_definition;

/// Handle a double-click. See `Command::DoubleClick`.
///
/// A door toggles open or shut; a container opens its gump; a mobile shows its
/// paperdoll. A weapon in the user's backpack is equipped. Any other item is
/// handed to the pack as an [`ItemUsed`] trigger, keyed by graphic — the engine
/// has no default "use" for a bare item, so a shard gives it one; without a
/// pack the double-click is simply silent.
///
/// Returns whether the engine performed an item action that must keep later
/// double-click dispatch (skill tools and shipped-item rules) from reusing it.
pub fn double_click(state: &mut WorldState, connection: ConnectionId, target_serial: Serial) -> bool {
    let Some(&player) = state.players.get(&connection) else {
        return false;
    };
    let Some(target) = state.registry.entity_of(target_serial) else {
        return false;
    };

    if equip_weapon_from_backpack(state, connection, target) {
        return true;
    }

    // A door toggles; a container opens its gump; a mobile shows its paperdoll;
    // anything else is a "use" rule not written yet, and a wrong guess is worse
    // than silence. A door is checked before Container because it is neither — it
    // is its own interaction.
    if state.registry.has::<Door>(target) {
        toggle_door(state, player, target, target_serial);
    } else if state.registry.has::<KeyValue>(target) {
        // A key raises a cursor for the lock it is about to turn.
        crate::use_key(state, connection, player, target);
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
        // The paperdoll. The `MobileUsed` trigger that lets a pack or the quest
        // rules give this mobile a meaning is *not* fired here — it is fired for
        // every double-clicked mobile before the click is interpreted at all, so
        // that a vendor (which never reaches this branch) is heard too.
        open_paperdoll(state, connection, player, target, target_serial);
    } else {
        // Not a door, container, spellbook, mount or mobile: an ordinary item.
        // Hand its "use" to the pack, keyed by graphic — Sphere's @DClick. The
        // engine has no default behaviour for a bare item, so this is silent
        // until a shard's script gives the graphic a meaning.
        item_used(state, player, target, target_serial);
    }
    false
}

/// Answer a `0x06` with bit 31 set — the client's *paperdoll request*, sent by
/// the paperdoll macro and on login. ServUO's `UseReq` routes this straight to
/// `OnPaperdollRequest`, never to `Use`: it opens the paperdoll and does nothing
/// else — above all it does not dismount a mounted rider, which is exactly what
/// treating it as a raw self-double-click used to do.
pub fn paperdoll_request(state: &mut WorldState, connection: ConnectionId, target_serial: Serial) {
    let Some(&player) = state.players.get(&connection) else {
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
    if !in_reach(state, book, player) {
        return;
    }
    let Some(version) = state.version_of(connection) else {
        return;
    };
    let mask = state.registry.get::<Spellbook>(book).map_or(0, |b| b.0);
    state.send(
        connection,
        // `BOOK_GUMP` is what tells the client this container is a book.
        encode_open_container(book_serial, BOOK_GUMP, version),
    );
    // Magery spells start at offset 1; the mask's bit `n` is spell `n`.
    state.send_packet(
        connection,
        &ServerPacket::SpellbookContent(SpellbookContent {
            serial: book_serial,
            graphic: SPELLBOOK_GRAPHIC,
            offset: 1,
            content: mask,
        }),
    );
}

/// The layer a bank box is worn on — UO's `Layer.Bank`.
///
/// Here rather than with the banker that opens it, because two rules need it and
/// only one of them is the banker's: what a character *carries* stops at the bank
/// box (see [`carried`](crate::carried)). `npc` re-exports this, so there is one
/// number and the two rules cannot drift apart.
pub const BANK_LAYER: Layer = Layer(0x1D);

/// Open the container a player wears at `layer` — its backpack, or its bank box.
///
/// The service path a banker uses: find the worn container and open it onto the
/// player's own client, the same `0x24`/`0x3C` a double-click sends. Does nothing
/// if the player wears no container there.
pub fn open_worn_container(state: &mut WorldState, connection: ConnectionId, player: EntityId, layer: Layer) {
    let Some(mobile) = state.registry.serial_of(player) else {
        return;
    };
    let worn = equipped_items(state, mobile)
        .find(|(item, eq)| eq.layer == layer && state.registry.has::<Container>(*item))
        .map(|(item, _)| item);
    if let Some(item) = worn {
        if let Some(serial) = state.registry.serial_of(item) {
            open_container(state, connection, player, item, serial);
        }
    }
}

/// What a house's secure says to somebody it does not know. The door's line for
/// a chest — see `doors::NOT_YOUR_DOOR`, whose reasoning this is the other half
/// of.
pub const NOT_YOUR_SECURE: &str = "You cannot open that.";

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
    if !in_reach(state, container, player) {
        return;
    }
    // A locked chest does not open — ServUO's `LockableContainer.OnDoubleClick`, which
    // says cliloc 501747 ("It appears to be locked.") and stops. Staff walk through it,
    // as they do the door.
    if crate::is_locked(state, container) && !state.is_staff(player) {
        state.system_message(player, crate::LOCKED_MESSAGE);
        return;
    }
    // A house's secure opens only for the people the house names. Said with the
    // *door's* line rather than the lock's, and that is the point of asking it
    // separately: a stranger at a secure is refused for being a stranger, and
    // "that is locked" would send them looking for a key that does not exist.
    if !state.may_open_secure(player, container) {
        state.system_message(player, NOT_YOUR_SECURE);
        return;
    }

    let Some(version) = state.version_of(connection) else {
        return;
    };
    let contents = contents_of(state, container_serial);
    state.send(connection, encode_open_container(container_serial, gump, version));
    state.send_packet(
        connection,
        &ServerPacket::ContainerContents(ContainerContents {
            container: Some(container_serial),
            items: contents.clone(),
        }),
    );
    // A corpse's contents have the item pictures, but a `0x3C` deliberately
    // has no field for the layers those items occupied on the living body.  The
    // companion `0x89` supplies that one relationship once the client has the
    // pictures it names; an ordinary chest has no such fact and sends nothing.
    if let Some(story) = state.registry.get::<Corpse>(container) {
        state.send_packet(
            connection,
            &ServerPacket::CorpseEquipment(CorpseEquipment {
                corpse: container_serial,
                items: story.equipment.clone(),
            }),
        );
    }
    // Remember it is open, so a later change to its contents can be pushed here.
    state
        .open_containers
        .entry(container_serial)
        .or_default()
        .insert(connection);
    debug!(
        %container_serial,
        gump = format!("0x{:04X}", gump.0),
        items = contents.len(),
        "container opened"
    );
}

/// Whether `player` may reach `container` — to open it, drop into it, use it or
/// aim a spell at it.
///
/// An item sits in one of three places, and the reach check has to handle all of
/// them: on the ground it stands on its own tile; worn, it has no `Position` of
/// its own and its wearer's tile stands in; contained, the question recurses to
/// the container holding it, so a rune in a pouch in a pack is reached through
/// the pack. Your own worn backpack is always in reach; another mobile's is
/// reachable only within [`ITEM_REACH`] of that mobile, on the same facet. The
/// whole reason a worn backpack could not be opened or filled before this: its
/// reach was measured against a `Position` it does not have.
///
/// Not named for containers any more, because it stopped being about them some
/// time ago — a spellbook, a door's key target and an item trigger all ask it.
pub fn in_reach(state: &WorldState, container: EntityId, player: EntityId) -> bool {
    let Some(&Position(player_pos)) = state.registry.get::<Position>(player) else {
        return false;
    };
    // Where the container effectively is: its own ground tile, or its wearer's.
    let anchor = match item_location(state, container) {
        Some(ItemLocation::Settled(SettledItemLocation::Ground { facet, position })) => {
            Some((facet, position))
        }
        Some(ItemLocation::Settled(SettledItemLocation::Equipped(Equipped { mobile, .. }))) => {
            if Some(mobile) == state.registry.serial_of(player) {
                return true; // one's own worn pack is always in reach
            }
            state
                .registry
                .entity_of(mobile)
                .and_then(|wearer| Some((state.facet_of(wearer), state.registry.get::<Position>(wearer)?.0)))
        }
        Some(ItemLocation::Settled(SettledItemLocation::Contained(Contained {
            container: outer, ..
        }))) => {
            // Nested — a spellbook in the pack, a bag in a bag: in reach when the
            // container holding it is. Recurse to that one's own reach test.
            return state
                .registry
                .entity_of(outer)
                .is_some_and(|outer| in_reach(state, outer, player));
        }
        Some(ItemLocation::Held { .. }) | None => None,
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
    let mut flags = PaperdollFlags::NONE;
    if state
        .registry
        .get::<Combat>(mobile)
        .is_some_and(|combat| combat.warmode())
    {
        flags = flags.with(PaperdollFlags::WARMODE);
    }
    if mobile == player {
        flags = flags.with(PaperdollFlags::CAN_LIFT);
    }
    state.send_packet(
        connection,
        &ServerPacket::OpenPaperdoll(OpenPaperdoll {
            serial: mobile_serial,
            text: name,
            flags,
        }),
    );
    debug!(%mobile_serial, "paperdoll opened");
}

/// Everything inside a container, as the wire records `0x3C`/`0x25` need.
pub fn contents_of(state: &WorldState, container: Serial) -> Vec<ContainedItem> {
    contained_items(state, container)
        .filter_map(|(entity, _)| contained_record(state, entity))
        .collect()
}

/// How many items a container already holds — the next free grid slot.
pub fn item_count(state: &WorldState, container: Serial) -> u8 {
    contained_items(state, container).count().min(u8::MAX as usize) as u8
}

/// How many of `graphic` a container holds, counting stack amounts.
#[must_use]
pub fn count_in_container(state: &WorldState, container: Serial, graphic: Graphic) -> u32 {
    contained_items(state, container)
        .filter(|(entity, _)| {
            state
                .registry
                .get::<Drawn>(*entity)
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
pub fn take_from_container(state: &mut WorldState, container: Serial, graphic: Graphic, count: u32) -> bool {
    if count == 0 {
        return true;
    }
    let matches: Vec<(EntityId, u16)> = contained_items(state, container)
        .filter(|(entity, _)| {
            state
                .registry
                .get::<Drawn>(*entity)
                .is_some_and(|g| g.id == graphic)
        })
        .map(|(entity, _)| (entity, state.registry.get::<Amount>(entity).map_or(1, |a| a.0)))
        .collect();
    let total: u32 = matches.iter().map(|(_, amount)| u32::from(*amount)).sum();
    if total < count {
        return false;
    }

    // Counted in `u32` because a total can exceed one stack's `u16` — a bank
    // purchase runs to six figures of gold, spread over as many piles as it takes.
    let mut remaining = count;
    for (entity, amount) in matches {
        let amount = u32::from(amount);
        if remaining == 0 {
            break;
        }
        if amount <= remaining {
            // The whole item goes: a contained item is on no sector grid and no
            // screen, so despawning it is all it takes.
            remaining -= amount;
            let serial = state.registry.serial_of(entity);
            despawn_item(state, entity);
            if let Some(serial) = serial {
                tell_watchers_removed(state, container, serial);
            }
        } else {
            // The remainder fits a stack by construction: it is what is left of an
            // `Amount` after taking less than all of it.
            set_stack_amount(state, entity, (amount - remaining) as u16);
            remaining = 0;
            tell_watchers_updated(state, container, entity);
        }
    }
    true
}

/// Put a discrete, non-stacking item into a container — a looted weapon, a suit
/// of armour, a gem. Unlike [`give`], it never merges: two identical swords are
/// two swords, not a stack of two. `amount` rides along for the odd counted
/// single, but no [`Stackable`] is set. Returns the item, or `None` when the
/// serial pool is dry. The counterpart of `give` for loot the pack drops.
pub fn place_one(
    state: &mut WorldState,
    container: Serial,
    graphic: Graphic,
    hue: Hue,
    amount: u16,
) -> Option<EntityId> {
    let Ok((entity, _serial)) = state.registry.spawn_with_serial(SerialKind::Item) else {
        warn!("out of item serials; nothing placed");
        return None;
    };
    let drawn = Drawn { id: graphic, hue };
    state.registry.insert(entity, drawn);
    crate::spawn::install_legacy_identity(state, entity, drawn);
    let contained = Contained {
        container,
        position: GumpPoint::new(60, 60),
        grid: GridSlot(0),
    };
    establish_item_location(state, entity, ItemLocation::contained(contained))
        .expect("a newly placed item has one valid container parent");
    if amount > 1 {
        state.registry.insert(entity, Amount(amount));
    }
    // A lute gets its tunes and a bottle its poison here, because a graphic alone
    // cannot say how many are left in either.
    crate::apply_core_defaults(state, entity, graphic);
    tell_watchers_updated(state, container, entity);
    Some(entity)
}

/// [`place_one`] for a semantic, non-stacking item.
///
/// The caller supplies identity, never a display pair. Keeping this separate
/// from the legacy constructor makes quality-bearing crafted equipment retain
/// its exact kind and material even if another kind later shares its art.
pub fn place_one_kind(
    state: &mut WorldState,
    container: Serial,
    kind: ItemKindId,
    material: Option<MaterialId>,
    amount: u16,
) -> Option<EntityId> {
    let drawn = presentation_of(kind, material)?;
    let Ok((entity, _serial)) = state.registry.spawn_with_serial(SerialKind::Item) else {
        warn!("out of item serials; nothing placed");
        return None;
    };
    state.registry.insert(entity, drawn);
    crate::spawn::install_identity(state, entity, kind, material);
    let contained = Contained {
        container,
        position: GumpPoint::new(60, 60),
        grid: GridSlot(0),
    };
    establish_item_location(state, entity, ItemLocation::contained(contained))
        .expect("a newly placed typed item has one valid container parent");
    if amount > 1 {
        state.registry.insert(entity, Amount(amount));
    }
    crate::apply_core_defaults(state, entity, drawn.id);
    tell_watchers_updated(state, container, entity);
    Some(entity)
}

/// Leave the remainder of a split stack behind *inside a container*, at the same
/// grid slot the original is vacating. The container sibling of `spawn_leftover`
/// (Sphere's `CItem::UnStackSplit`): the original keeps its serial and goes onto
/// the cursor with the taken amount, and this dupe — a new serial — holds the
/// remainder in the container, drawn into every open gump with a `0x25`.
pub fn spawn_contained_leftover(
    state: &mut WorldState,
    original: EntityId,
    amount: u16,
    contained: Contained,
) -> Option<EntityId> {
    let &Drawn { id, hue } = state.registry.get::<Drawn>(original)?;
    let leftover = match state.registry.spawn_with_serial(SerialKind::Item) {
        Ok((entity, _)) => entity,
        Err(error) => {
            warn!(?error, "out of item serials; a split remainder is lost");
            return None;
        }
    };
    let drawn = Drawn { id, hue };
    state.registry.insert(leftover, drawn);
    crate::spawn::copy_identity(state, original, leftover);
    state.registry.insert(leftover, Stackable);
    set_stack_amount(state, leftover, amount);
    let location = Contained {
        container: contained.container,
        position: contained.position,
        grid: contained.grid,
    };
    establish_item_location(state, leftover, ItemLocation::contained(location))
        .expect("a contained split remainder has one valid parent");
    tell_watchers_updated(state, contained.container, leftover);
    Some(leftover)
}

/// What [`give`] managed to put in a container.
///
/// The count matters: running out of item serials can happen after existing
/// piles were filled or one new pile was made, so an `Option` of the last pile
/// cannot distinguish a complete payout from a partial one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[must_use = "a payout can be partial when the item serial pool is exhausted"]
pub struct GiveOutcome {
    /// What the caller asked to put in the container.
    pub requested: u32,
    /// What was actually put in the container.
    pub given: u32,
    /// The last existing or newly-created pile touched, if any.
    pub last: Option<EntityId>,
}

impl GiveOutcome {
    /// Whether the whole requested amount arrived.
    #[must_use]
    pub const fn is_complete(self) -> bool {
        self.given == self.requested
    }

    /// How much could not be put in the container.
    #[must_use]
    pub const fn missing(self) -> u32 {
        self.requested.saturating_sub(self.given)
    }
}

#[derive(Clone, Copy)]
struct GiveProgress {
    requested: u32,
    left: u32,
    last: Option<EntityId>,
}

impl GiveProgress {
    const fn new(requested: u32) -> Self {
        Self {
            requested,
            left: requested,
            last: None,
        }
    }

    const fn outcome(self) -> GiveOutcome {
        GiveOutcome {
            requested: self.requested,
            given: self.requested - self.left,
            last: self.last,
        }
    }
}

/// Put `amount` of an item into a container by decree — a vendor handing over
/// goods, a sale paying out gold. Merges onto the existing stackable piles of
/// the same art and hue, and starts as many new ones as the remainder needs;
/// everyone with the container open sees each change.
///
/// `amount` is a `u32` because a payout is not bounded by what one pile holds: a
/// large sale earns more gold than [`MAX_STACK`], and taking a `u16` here made
/// the caller clamp — paying 65,535 for a 100,000 sale and saying nothing.
pub fn give(
    state: &mut WorldState,
    container: Serial,
    graphic: Graphic,
    hue: Hue,
    amount: u32,
) -> GiveOutcome {
    if amount == 0 {
        return GiveProgress::new(0).outcome();
    }
    // Two books are single items, never a stack: each carries contents of its
    // own, and two of them merged into one pile of two would share the learned
    // spells or the bound destinations of neither. A full spellbook is dealt out
    // elsewhere (a staff command); one off the shelf is blank until scrolls fill
    // it, and a runebook until runes do.
    if graphic == SPELLBOOK_GRAPHIC || graphic == RUNEBOOK_GRAPHIC {
        return give_book(state, container, graphic, hue, amount);
    }
    // The legacy API is still the entry point for old callers, but a pair the
    // registry recognizes must immediately use the semantic payout path. This
    // closes the last art-only merge door for ingots, ore and other migrated
    // stack kinds without forcing every caller to change in one patch.
    if let Some((kind, material)) = kind_from_drawn(Drawn { id: graphic, hue }) {
        return give_kind(state, container, kind, material, amount)
            .expect("an audited legacy identity has a valid typed presentation");
    }

    let progress = fill_existing_piles(state, container, graphic, hue, amount);
    spawn_remaining_piles(state, container, graphic, hue, progress)
}

/// Put a semantically identified stack into a container.
///
/// The drawing is derived once from the definition registry. This is the typed
/// counterpart of [`give`] for harvesting, crafting and vendors as they move off
/// the legacy graphic/hue construction API.
pub fn give_kind(
    state: &mut WorldState,
    container: Serial,
    kind: ItemKindId,
    material: Option<MaterialId>,
    amount: u32,
) -> Option<GiveOutcome> {
    let drawn = presentation_of(kind, material)?;
    if amount == 0 {
        return Some(GiveProgress::new(0).outcome());
    }
    if item_definition(kind)
        .and_then(|definition| definition.container_gump)
        .is_some()
    {
        return Some(give_container_kind(
            state, container, kind, material, drawn, amount,
        ));
    }
    let progress = fill_existing_kind_piles(state, container, kind, material, amount);
    Some(spawn_remaining_kind_piles(
        state, container, kind, material, drawn, progress,
    ))
}

/// Give semantic containers as separate usable entities, never as a stack.
///
/// `give_kind` is also used by typed loot and vendor paths, whose historic
/// `stackable` flag must not turn an openable bag into a pile of bag art.
fn give_container_kind(
    state: &mut WorldState,
    container: Serial,
    kind: ItemKindId,
    material: Option<MaterialId>,
    drawn: Drawn,
    amount: u32,
) -> GiveOutcome {
    let mut progress = GiveProgress::new(amount);
    while progress.left > 0 {
        let Ok((entity, _serial)) = state.registry.spawn_with_serial(SerialKind::Item) else {
            let outcome = progress.outcome();
            warn!(
                requested = outcome.requested,
                given = outcome.given,
                missing = outcome.missing(),
                ?container,
                ?kind,
                "out of item serials; semantic container payout is partial"
            );
            return outcome;
        };
        state.registry.insert(entity, drawn);
        crate::spawn::install_identity(state, entity, kind, material);
        establish_item_location(
            state,
            entity,
            ItemLocation::contained(Contained {
                container,
                position: GumpPoint::new(60, 60),
                grid: GridSlot(0),
            }),
        )
        .expect("a newly given semantic container has one valid parent");
        crate::apply_core_defaults(state, entity, drawn.id);
        tell_watchers_updated(state, container, entity);
        progress.left -= 1;
        progress.last = Some(entity);
    }
    progress.outcome()
}

fn give_book(
    state: &mut WorldState,
    container: Serial,
    graphic: Graphic,
    hue: Hue,
    requested: u32,
) -> GiveOutcome {
    let Ok((entity, _serial)) = state.registry.spawn_with_serial(SerialKind::Item) else {
        warn!(
            requested,
            given = 0,
            missing = requested,
            ?container,
            "out of item serials; payout failed"
        );
        return GiveProgress::new(requested).outcome();
    };
    let drawn = Drawn { id: graphic, hue };
    state.registry.insert(entity, drawn);
    crate::spawn::install_legacy_identity(state, entity, drawn);
    let contained = Contained {
        container,
        position: GumpPoint::new(60, 60),
        grid: GridSlot(0),
    };
    establish_item_location(state, entity, ItemLocation::contained(contained))
        .expect("a newly given book has one valid container parent");
    // The common factory reads a registered book's semantic role.  The graphic
    // argument remains solely for a genuinely legacy book with no identity.
    crate::apply_core_defaults(state, entity, graphic);
    tell_watchers_updated(state, container, entity);
    GiveOutcome {
        requested,
        given: 1,
        last: Some(entity),
    }
}

fn fill_existing_piles(
    state: &mut WorldState,
    container: Serial,
    graphic: Graphic,
    hue: Hue,
    requested: u32,
) -> GiveProgress {
    // Every pile of the same art already in there, in registry order.
    let piles: Vec<EntityId> = contained_items(state, container)
        .filter(|(entity, _)| {
            (state.registry.has::<Stackable>(*entity)
                || state
                    .registry
                    .get::<Drawn>(*entity)
                    .is_some_and(|g| intrinsically_stackable(g.id)))
                && crate::stack_compatible_instance_state(state, *entity)
                && state
                    .registry
                    .get::<Drawn>(*entity)
                    .is_some_and(|g| g.id == graphic && g.hue == hue)
        })
        .map(|(entity, _)| entity)
        .collect();

    // Fill them in turn, and let whatever is left over start a pile of its own —
    // the way a container ends up holding two gold piles after a large payout.
    // Clamping the sum instead would quietly destroy the difference, which is the
    // bug this exists to not have.
    let mut progress = GiveProgress::new(requested);
    for pile in piles {
        if progress.left == 0 {
            break;
        }
        let room = u32::from(MAX_STACK.saturating_sub(amount_of(state, pile)));
        let moved = progress.left.min(room);
        if moved > 0 {
            let total = amount_of(state, pile) + moved as u16;
            state.registry.insert(pile, Amount(total));
            if intrinsically_stackable(graphic) {
                state.registry.insert(pile, Stackable);
            }
            tell_watchers_updated(state, container, pile);
            progress.last = Some(pile);
            progress.left -= moved;
        }
    }
    progress
}

fn spawn_remaining_piles(
    state: &mut WorldState,
    container: Serial,
    graphic: Graphic,
    hue: Hue,
    mut progress: GiveProgress,
) -> GiveOutcome {
    // Whatever is still in hand starts new piles, one full one at a time.
    while progress.left > 0 {
        let take = progress.left.min(u32::from(MAX_STACK)) as u16;
        let Ok((entity, _serial)) = state.registry.spawn_with_serial(SerialKind::Item) else {
            let outcome = progress.outcome();
            warn!(
                requested = outcome.requested,
                given = outcome.given,
                missing = outcome.missing(),
                ?container,
                "out of item serials; payout is partial"
            );
            return outcome;
        };
        let drawn = Drawn { id: graphic, hue };
        state.registry.insert(entity, drawn);
        crate::spawn::install_legacy_identity(state, entity, drawn);
        let contained = Contained {
            container,
            position: GumpPoint::new(60, 60),
            grid: GridSlot(0),
        };
        establish_item_location(state, entity, ItemLocation::contained(contained))
            .expect("a newly given stack has one valid container parent");
        state.registry.insert(entity, Amount(take));
        state.registry.insert(entity, Stackable);
        tell_watchers_updated(state, container, entity);
        progress.left -= u32::from(take);
        progress.last = Some(entity);
    }
    progress.outcome()
}

/// The semantic counterpart of [`fill_existing_piles`].
///
/// A typed payout deliberately does not merge with an unmigrated legacy pile
/// even where their current drawing matches: that would make a later art reuse
/// an identity-changing operation. The migration bridge can be made explicit
/// by its caller when a one-time upgrade is desired.
fn fill_existing_kind_piles(
    state: &mut WorldState,
    container: Serial,
    kind: ItemKindId,
    material: Option<MaterialId>,
    requested: u32,
) -> GiveProgress {
    let piles: Vec<EntityId> = contained_items(state, container)
        .filter(|(entity, _)| {
            state.registry.has::<Stackable>(*entity)
                && crate::stack_compatible_instance_state(state, *entity)
                && state
                    .registry
                    .get::<ItemKind>(*entity)
                    .is_some_and(|found| found.0 == kind)
                && state.registry.get::<Material>(*entity).map(|found| found.0) == material
        })
        .map(|(entity, _)| entity)
        .collect();
    let mut progress = GiveProgress::new(requested);
    for pile in piles {
        if progress.left == 0 {
            break;
        }
        let moved = progress
            .left
            .min(u32::from(MAX_STACK.saturating_sub(amount_of(state, pile))));
        if moved > 0 {
            state
                .registry
                .insert(pile, Amount(amount_of(state, pile) + moved as u16));
            tell_watchers_updated(state, container, pile);
            progress.last = Some(pile);
            progress.left -= moved;
        }
    }
    progress
}

/// The semantic counterpart of [`spawn_remaining_piles`].
fn spawn_remaining_kind_piles(
    state: &mut WorldState,
    container: Serial,
    kind: ItemKindId,
    material: Option<MaterialId>,
    drawn: Drawn,
    mut progress: GiveProgress,
) -> GiveOutcome {
    while progress.left > 0 {
        let take = progress.left.min(u32::from(MAX_STACK)) as u16;
        let Ok((entity, _serial)) = state.registry.spawn_with_serial(SerialKind::Item) else {
            let outcome = progress.outcome();
            warn!(
                requested = outcome.requested,
                given = outcome.given,
                missing = outcome.missing(),
                ?container,
                "out of item serials; typed payout is partial"
            );
            return outcome;
        };
        state.registry.insert(entity, drawn);
        crate::spawn::install_identity(state, entity, kind, material);
        establish_item_location(
            state,
            entity,
            ItemLocation::contained(Contained {
                container,
                position: GumpPoint::new(60, 60),
                grid: GridSlot(0),
            }),
        )
        .expect("a newly given typed stack has one valid container parent");
        state.registry.insert(entity, Amount(take));
        state.registry.insert(entity, Stackable);
        tell_watchers_updated(state, container, entity);
        progress.left -= u32::from(take);
        progress.last = Some(entity);
    }
    progress.outcome()
}

/// Take `amount` off a contained stack by decree — stock sold out of a
/// vendor's crate, goods sold out of a player's pack. Returns how many were
/// actually taken; a stack that reaches zero is despawned and forgotten by
/// everyone watching the container.
pub fn remove_from_stack(state: &mut WorldState, container: Serial, item: EntityId, amount: u16) -> u16 {
    let have = amount_of(state, item);
    let take = have.min(amount);
    if take == 0 {
        return 0;
    }
    if take == have {
        if let Some(serial) = state.registry.serial_of(item) {
            tell_watchers_removed(state, container, serial);
        }
        despawn_item(state, item);
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
    tell_watchers_removed_except(state, container, item, None);
}

/// [`tell_watchers_removed`], skipping one connection.
///
/// The connection to skip is the one that *lifted* the item: its client already
/// has the thing on its cursor, and a `0x1D` for an item it is dragging reads as
/// the object going away underneath it.
pub(crate) fn tell_watchers_removed_except(
    state: &mut WorldState,
    container: Serial,
    item: Serial,
    except: Option<ConnectionId>,
) {
    let watchers: Vec<ConnectionId> = state
        .open_containers
        .get(&container)
        .map(|w| w.iter().copied().collect())
        .unwrap_or_default();
    for connection in watchers {
        if Some(connection) == except {
            continue;
        }
        state.send_packet(connection, &ServerPacket::Remove(Remove { serial: item }));
    }
}

/// Tell every client with `container` open that an item in it changed — a dipped
/// stack's new amount — by re-sending its `0x25` record.
pub(crate) fn tell_watchers_updated(state: &mut WorldState, container: Serial, entity: EntityId) {
    tell_watchers_updated_except(state, container, entity, None);
}

/// [`tell_watchers_updated`], skipping one connection — the one already told.
///
/// `0x25` both adds and updates, so this is also how an item that has just
/// *arrived* in a container appears in everyone else's open gump. Without it a
/// second viewer sees nothing until they reopen, which is a tolerable limitation
/// for a chest and a fatal one for a trade window: the whole point is watching
/// what the other party puts down.
pub(crate) fn tell_watchers_updated_except(
    state: &mut WorldState,
    container: Serial,
    entity: EntityId,
    except: Option<ConnectionId>,
) {
    let Some(record) = contained_record(state, entity) else {
        return;
    };
    let watchers: Vec<ConnectionId> = state
        .open_containers
        .get(&container)
        .map(|w| w.iter().copied().collect())
        .unwrap_or_default();
    for connection in watchers {
        if Some(connection) == except {
            continue;
        }
        if let Some(version) = state.version_of(connection) {
            state.send(connection, encode_add_to_container(record, container, version));
        }
    }
}

/// Build the `0x25`/`0x3C` record for one contained item.
pub fn contained_record(state: &WorldState, entity: EntityId) -> Option<ContainedItem> {
    let serial = state.registry.serial_of(entity)?;
    // Component and record now carry the same three types, so this is a copy
    // rather than a conversion — which is the point of having swept them.
    let ItemLocation::Settled(SettledItemLocation::Contained(Contained { position, grid, .. })) =
        item_location(state, entity)?
    else {
        return None;
    };
    let Drawn { id, hue } = *state.registry.get::<Drawn>(entity)?;
    let amount = state.registry.get::<Amount>(entity).map_or(1, |a| a.0);
    Some(ContainedItem {
        serial,
        graphic: id,
        amount: openshard_protocol::items::ItemAmount(amount),
        at: position,
        grid,
        hue,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use openshard_protocol::serial::{ITEM_MAX, ITEM_MIN};

    use super::*;

    fn world() -> WorldState {
        WorldState::new(
            BTreeMap::new(),
            Facet(0),
            openshard_tiles::TileData::empty(),
            Default::default(),
            openshard_map::grid::Tile::new(0, 0),
            1,
        )
    }

    #[test]
    fn give_reports_the_amount_that_landed_before_serial_exhaustion() {
        let mut state = world();
        let container_entity = state.registry.spawn();
        let container = Serial::new(ITEM_MIN).expect("the first item serial");
        state
            .registry
            .bind_serial(container_entity, container)
            .expect("a fresh serial");
        state
            .registry
            .insert(container_entity, Container { gump: Graphic(1) });

        // Leave exactly ITEM_MAX available. The first pile consumes it and the
        // one-unit remainder then reaches the exhausted-pool branch.
        state
            .registry
            .reserve_serial(Serial::new(ITEM_MAX - 1).expect("the penultimate item serial"));
        let outcome = give(
            &mut state,
            container,
            GOLD_GRAPHIC,
            Hue(0),
            u32::from(MAX_STACK) + 1,
        );

        assert_eq!(outcome.requested, u32::from(MAX_STACK) + 1);
        assert_eq!(outcome.given, u32::from(MAX_STACK));
        assert_eq!(outcome.missing(), 1);
        assert!(!outcome.is_complete());
        assert!(outcome.last.is_some(), "the full first pile was still issued");
        assert_eq!(
            count_in_container(&state, container, GOLD_GRAPHIC),
            u32::from(MAX_STACK)
        );
    }

    #[test]
    fn typed_give_does_not_merge_into_an_affixed_equivalent_pile() {
        use openshard_protocol::item_kind::{ItemKindId, MaterialId};

        let mut state = world();
        let container_entity = state.registry.spawn();
        let container = Serial::new(ITEM_MIN).expect("the first item serial");
        state
            .registry
            .bind_serial(container_entity, container)
            .expect("a fresh container serial");
        state
            .registry
            .insert(container_entity, Container { gump: Graphic(1) });

        let (existing, _) = state
            .registry
            .spawn_with_serial(SerialKind::Item)
            .expect("an item serial");
        state.registry.insert(
            existing,
            Drawn {
                id: Graphic(0x1BF2),
                hue: Hue(0x08AB),
            },
        );
        state.registry.insert(existing, ItemKind(ItemKindId(1)));
        state.registry.insert(existing, Material(MaterialId(9)));
        state.registry.insert(existing, Stackable);
        state.registry.insert(existing, Amount(3));
        state.registry.insert(existing, ItemAffixes::default());
        establish_item_location(
            &mut state,
            existing,
            ItemLocation::contained(Contained {
                container,
                position: GumpPoint::new(60, 60),
                grid: GridSlot(0),
            }),
        )
        .expect("the original pile is in the container");

        let outcome = give_kind(&mut state, container, ItemKindId(1), Some(MaterialId(9)), 2)
            .expect("registered ingot identity");
        let new_pile = outcome.last.expect("a new pile was issued");
        assert_ne!(new_pile, existing);
        assert_eq!(amount_of(&state, existing), 3, "the affixed item was not changed");
        assert_eq!(amount_of(&state, new_pile), 2);
    }
}
