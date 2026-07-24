use super::*;

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

/// Override a weapon item's speed and damage — the pack's magic sword, its stats
/// standing in for the core weapon table's for that graphic. See
/// `Command::SetWeapon`. A stray or non-existent serial sets nothing.
pub fn set_weapon(state: &mut WorldState, serial: u32, speed: u16, min: u16, max: u16) {
    let Some(entity) = Serial::new(serial).and_then(|serial| state.registry.entity_of(serial))
    else {
        return;
    };
    state.registry.insert(entity, Weapon { speed, min, max });
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

/// Give a mobile a container to *wear* — a backpack, a bank box — rather than one
/// on the ground. It is an item like any other, but worn: an [`Equipped`] instead
/// of a [`Position`], so it is off the sector grid and off every screen except as
/// part of its wearer's `0x78`, and it never decays. Returns the item's entity, or
/// `None` if the item-serial pool is empty.
///
/// This is how a fresh character gets its backpack: without one the paperdoll's
/// bag is dead and there is nowhere to put anything picked up.
pub fn equip_new_container(
    state: &mut WorldState,
    mobile: Serial,
    graphic: u16,
    gump: u16,
    hue: u16,
    layer: u8,
) -> Option<EntityId> {
    let (entity, serial) = match state.registry.spawn_with_serial(SerialKind::Item) {
        Ok(pair) => pair,
        Err(error) => {
            warn!(?error, "out of item serials; not equipping a container");
            return None;
        }
    };
    state.registry.insert(entity, Graphic { id: graphic, hue });
    state.registry.insert(entity, Container { gump });
    state.registry.insert(entity, Equipped { mobile, layer });
    debug!(%serial, graphic, layer, "container equipped");
    Some(entity)
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

/// Land an item on the ground at `position` and draw it for everyone in range.
pub fn place_on_ground(state: &mut WorldState, item: EntityId, position: Point, facet: u8) {
    state.registry.insert(item, Position(position));
    state.registry.insert(item, Facet(facet));
    // Back on the ground, back on the decay clock.
    mark_decay(state, item);
    state.facet_state_mut(facet).sectors.insert(item, position);
    state.reveal(item);
}
