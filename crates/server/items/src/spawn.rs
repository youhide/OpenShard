use openshard_protocol::item_kind::{
    ItemKindId,
    MaterialId,
};
use openshard_protocol::wire::{
    Graphic,
    Hue,
};
use openshard_protocol::world::PoisonLevel;

use super::*;

/// An item appeared in the world.
///
/// Emitted when the server puts a thing on the ground — the item counterpart of
/// `PlayerEntered`. What a script or persistence does with it is their affair;
/// the world's part is only to say it happened.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ItemSpawned {
    /// The entity.
    pub entity:    EntityId,
    /// Its wire identity.
    pub serial:    Serial,
    /// Semantic kind, present for migrated definitions.
    pub item_kind: Option<ItemKindId>,
    /// Semantic material grade, when this item has one.
    pub material:  Option<MaterialId>,
    /// Where it lies.
    pub position:  Point,
}

/// Override a weapon item's speed and damage — the pack's magic sword, its stats
/// standing in for the core weapon table's for that graphic. See
/// `Command::SetWeapon`. A stray or non-existent serial sets nothing.
pub fn set_weapon(state: &mut WorldState, serial: Serial, speed: u16, min: u16, max: u16) {
    let Some(entity) = state.registry.entity_of(serial) else {
        return;
    };
    state.registry.insert(entity, Weapon { speed, min, max });
}

/// Replace one item's typed custom properties.
///
/// This is the one mutation door for loot, crafting and staff tooling. An empty
/// list removes the component so ordinary items stay component-free.
pub fn set_affixes(state: &mut WorldState, serial: Serial, affixes: Vec<ItemAffix>) {
    let Some(entity) = state.registry.entity_of(serial) else {
        return;
    };
    if affixes.is_empty() {
        state.registry.remove::<ItemAffixes>(entity);
    } else {
        state.registry.insert(entity, ItemAffixes(affixes));
    }
}

/// Put poison on an item, or take it off. See `Command::SetPoison`.
///
/// The pack's door to the poison economy: all four poison potions are the same
/// bottle on the wire, so which poison one holds is on the item and something has to
/// put it there. `charges` of zero clears it, which is also how a spent coating is
/// wiped — one door in and out, so "is this poisoned" never has two answers.
pub fn set_poison(state: &mut WorldState, serial: Serial, level: PoisonLevel, charges: u16) {
    let Some(entity) = state.registry.entity_of(serial) else {
        return;
    };
    if charges == 0 {
        state.registry.remove::<PoisonCharges>(entity);
        return;
    }
    state.registry.insert(entity, PoisonCharges { level, charges });
}

/// Put an item on the ground. See `Command::SpawnItem`.
///
/// Returns the entity so `spawn_container` can make
/// the same thing and then say it holds others.
pub fn spawn_item(
    state: &mut WorldState,
    graphic: Graphic,
    hue: Hue,
    amount: u16,
    stackable: bool,
    position: Point,
    facet: Facet,
) -> Option<EntityId> {
    if !is_valid_stack_amount(amount) {
        warn!(amount, "an item needs a positive representable amount");
        return None;
    }
    let facet = if state.facets.contains_key(&facet) {
        facet
    } else {
        warn!(facet = %facet, "unloaded facet; spawning the item on the default");
        state.default_facet
    };
    let (entity, serial) = match state.registry.spawn_with_serial(SerialKind::Item) {
        Ok(pair) => pair,
        Err(error) => {
            warn!(?error, "out of item serials; not spawning");
            return None;
        }
    };
    let drawn = Drawn { id: graphic, hue };
    state.registry.insert(entity, drawn);
    // This is the compatibility door for the many legacy graphic/hue callers
    // that have not moved to `spawn_item_kind` yet. An unknown pair is kept
    // visibly intact but deliberately receives no invented semantic id.
    install_legacy_identity(state, entity, drawn);
    establish_item_location(state, entity, ItemLocation::ground(facet, position))
        .expect("a newly spawned ground item has one valid location");
    // Only a real stack carries an amount; a single item stays a bare graphic.
    initialize_stack_amount(state, entity, amount);
    // Coins and ammunition are stackable even as a single item.  Callers
    // commonly spawn one item at a time (notably the staff `.add` command),
    // and making their stackability depend on the requested amount leaves
    // otherwise identical items unable to merge.
    if stackable || intrinsically_stackable(graphic) {
        state.registry.insert(entity, Stackable);
    }
    mark_decay(state, entity);
    state.place_item(facet, entity, position);
    state.bus.send(ItemSpawned {
        entity,
        serial,
        item_kind: state.registry.get::<ItemKind>(entity).map(|kind| kind.0),
        material: state.registry.get::<Material>(entity).map(|material| material.0),
        position,
    });
    state.reveal(entity);
    debug!(%serial, graphic = graphic.0, position = %position, "item on the ground");
    crate::apply_core_defaults(state, entity, graphic);
    Some(entity)
}

/// Put a semantically identified item on the ground.
///
/// This is the constructor new gameplay code uses. Its `Drawn` component is
/// always the checked projection from the item-definition registry, never a
/// caller-supplied graphic/hue pair.
pub fn spawn_item_kind(
    state: &mut WorldState,
    kind: ItemKindId,
    material: Option<MaterialId>,
    amount: u16,
    stackable: bool,
    position: Point,
    facet: Facet,
) -> Option<EntityId> {
    let drawn = presentation_of(kind, material)?;
    let entity = spawn_item(state, drawn.id, drawn.hue, amount, stackable, position, facet)?;
    install_identity(state, entity, kind, material);
    Some(entity)
}

/// Install a semantic identity after a validated construction or migration.
///
/// Kept beside item construction rather than exposed as an arbitrary component
/// mutation, so a caller cannot leave `Drawn` and the semantic identity
/// disagreeing. The registry projection is installed here as well: callers may
/// never supply an arbitrary drawing alongside a semantic identity.
pub fn install_identity(
    state: &mut WorldState,
    entity: EntityId,
    kind: ItemKindId,
    material: Option<MaterialId>,
) {
    openshard_state::item_identity::install_item_identity(state, entity, kind, material);
}

/// Upgrade an audited legacy presentation pair when the registry names it.
pub(crate) fn install_legacy_identity(state: &mut WorldState, entity: EntityId, drawn: Drawn) {
    if let Some((kind, material)) = kind_from_drawn(drawn) {
        install_identity(state, entity, kind, material);
    }
}

/// Copy the identity an item already carries into one of its physical copies.
///
/// Stack splitting is a clone operation, not another legacy migration: if two
/// future kinds share a presentation, looking at the art here would turn one
/// into the other. An unmigrated original retains its explicit legacy state.
pub(crate) fn copy_identity(state: &mut WorldState, original: EntityId, copy: EntityId) {
    if let Some(kind) = state.registry.get::<ItemKind>(original).copied() {
        let material = state
            .registry
            .get::<Material>(original)
            .copied()
            .map(|material| material.0);
        install_identity(state, copy, kind.0, material);
    }
}

/// Put a container on the ground. See `Command::SpawnContainer`.
///
/// A container is an ordinary ground item that also carries a [`Container`],
/// which is the only thing that makes it openable. So it is spawned exactly
/// like one and then marked.
pub fn spawn_container(
    state: &mut WorldState,
    graphic: Graphic,
    gump: Graphic,
    hue: Hue,
    position: Point,
    facet: Facet,
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
    graphic: Graphic,
    gump: Graphic,
    hue: Hue,
    layer: Layer,
) -> Option<EntityId> {
    let (entity, serial) = match state.registry.spawn_with_serial(SerialKind::Item) {
        Ok(pair) => pair,
        Err(error) => {
            warn!(?error, "out of item serials; not equipping a container");
            return None;
        }
    };
    let drawn = Drawn { id: graphic, hue };
    state.registry.insert(entity, drawn);
    install_legacy_identity(state, entity, drawn);
    state.registry.insert(entity, Container { gump });
    let equipped = Equipped { mobile, layer };
    establish_item_location(state, entity, ItemLocation::equipped(equipped))
        .expect("a newly equipped container has one valid location");
    debug!(%serial, graphic = graphic.0, layer = layer.0, "container equipped");
    Some(entity)
}

/// Land an item on the ground at `position` and draw it for everyone in range.
pub fn place_on_ground(state: &mut WorldState, item: EntityId, position: Point, facet: Facet) {
    relocate_item(state, item, ItemLocation::ground(facet, position))
        .expect("a ground drop must have a valid canonical location");
    // Back on the ground, back on the decay clock.
    mark_decay(state, item);
    state.place_item(facet, item, position);
    state.reveal(item);
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use openshard_protocol::item_kind::{
        ItemKindId,
        MaterialId,
    };

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
    fn semantic_identity_replaces_a_mismatched_legacy_drawing() {
        let mut state = world();
        let item = state.registry.spawn();
        state.registry.insert(
            item,
            Drawn {
                id:  Graphic(0x0EFA), // spellbook art, deliberately not a sword
                hue: Hue::NONE,
            },
        );

        install_identity(&mut state, item, ItemKindId(4), Some(MaterialId(9)));

        assert_eq!(
            state.registry.get::<Drawn>(item),
            presentation_of(ItemKindId(4), Some(MaterialId(9))).as_ref(),
            "identity owns the visible projection"
        );
        assert_eq!(
            state.registry.get::<ItemKind>(item),
            Some(&ItemKind(ItemKindId(4)))
        );
        assert_eq!(
            state.registry.get::<Material>(item),
            Some(&Material(MaterialId(9)))
        );
    }

    #[test]
    fn invalid_spawn_amounts_refuse_before_allocating_an_item() {
        let mut state = world();
        let at = Point::new(10, 10, 0);

        assert_eq!(
            spawn_item(&mut state, GOLD_GRAPHIC, Hue::NONE, 0, true, at, Facet(0)),
            None,
        );
        assert_eq!(
            spawn_item(
                &mut state,
                GOLD_GRAPHIC,
                Hue::NONE,
                MAX_STACK + 1,
                true,
                at,
                Facet(0),
            ),
            None,
        );
        assert_eq!(
            spawn_item(&mut state, GOLD_GRAPHIC, Hue::NONE, u16::MAX, true, at, Facet(0),),
            None,
        );
        assert_eq!(
            state.registry.query::<Drawn>().count(),
            0,
            "a refused amount must not consume an entity or item serial"
        );
    }
}
