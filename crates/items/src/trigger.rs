use super::*;

/// A player used (double-clicked) an item the engine has no built-in behaviour
/// for — the item-trigger seam.
///
/// Sphere's `@DClick`/`@Use`, reached the way this engine reaches everything: an
/// event, not a call. The engine handles the interactions it *knows* — a door
/// toggles, a container opens, a spellbook unfolds, a mount is ridden — and hands
/// every other item to the pack, keyed by its `graphic`, to give it a meaning: a
/// potion drunk, a lever pulled, a sign read, a deed placed. Nothing about what
/// the item *does* lives in the engine; the pack reads this off `onEvent` and
/// answers with ops, the same "default in core, customise in the pack" split
/// spells and skills use — except here the core default is *nothing*, because a
/// bare graphic has no behaviour until a shard gives it one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ItemUsed {
    /// The item that was double-clicked.
    pub item: Serial,
    /// Its graphic, so a pack matches on the tile with no lookup.
    pub graphic: u16,
    /// The mobile that used it.
    pub by: Serial,
}

/// Emit [`ItemUsed`] for an in-reach item — the last resort of
/// [`double_click`](crate::double_click), after every built-in interaction has
/// declined.
///
/// Reach is server-authoritative: the same [`container_in_reach`] a lift uses,
/// which resolves a ground item by its tile, a carried one by its holder, and a
/// worn one by its wearer — so a double-click across the map fires nothing. An
/// item that somehow has no `Graphic` is not a drawable item and is ignored.
pub(crate) fn item_used(
    state: &mut WorldState,
    player: EntityId,
    target: EntityId,
    target_serial: Serial,
) {
    if !container_in_reach(state, target, player) {
        return;
    }
    let Some(&Graphic { id, .. }) = state.registry.get::<Graphic>(target) else {
        return;
    };
    let Some(by) = state.registry.serial_of(player) else {
        return;
    };
    state.bus.send(ItemUsed {
        item: target_serial,
        graphic: id,
        by,
    });
}
