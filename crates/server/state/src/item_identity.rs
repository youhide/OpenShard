//! The one door through which an item gets a semantic identity.
//!
//! An item's `Drawn` is a *projection* of its [`ItemKind`] and [`Material`],
//! never a caller-supplied pair sitting beside them: two writers could
//! otherwise leave a stone oven deed drawn as a scroll of one kind and typed as
//! another. This function is that projection, and it lives in `openshard-state`
//! rather than in `openshard-items` because the item graph is not the only
//! thing that constructs typed items — a demolished house hands one back as a
//! deed, and `openshard-housing` deliberately does not depend on the
//! drag-and-drop layer (see `housing::decay::take_off_the_ground`).

use openshard_entities::EntityId;
use openshard_protocol::item_kind::{
    ItemKindId,
    MaterialId,
};

use crate::WorldState;
use crate::components::{
    ItemKind,
    Material,
};
use crate::item_definition::presentation_of;

/// Install a semantic identity and the drawing the registry derives from it.
///
/// # Panics
///
/// If `kind` and `material` are not a pair the item-definition registry
/// accepts. Every caller resolves the pair from the registry first — a recipe
/// row, an addon's deed kind, an admin form — so an unprojectable pair is a
/// bug in that resolution, not a runtime condition to recover from.
pub fn install_item_identity(
    state: &mut WorldState,
    entity: EntityId,
    kind: ItemKindId,
    material: Option<MaterialId>,
) {
    let drawn = presentation_of(kind, material).expect("only validated item identities are installed");
    state.registry.insert(entity, drawn);
    state.registry.insert(entity, ItemKind(kind));
    match material {
        Some(material) => {
            state.registry.insert(entity, Material(material));
        }
        None => {
            state.registry.remove::<Material>(entity);
        }
    }
    state.invalidate_house_inventory_for_item(entity);
}
