use openshard_protocol::item_kind::{
    ItemKindId,
    MaterialId,
};
use openshard_protocol::wire::Graphic;

use super::*;

/// A player used (double-clicked) an item the engine has no built-in behaviour
/// for — the item-trigger seam.
///
/// Sphere's `@DClick`/`@Use`, reached the way this engine reaches everything: an
/// event, not a call. The engine handles the interactions it *knows* — a door
/// toggles, a container opens, a spellbook unfolds, a mount is ridden — and emits
/// this for every other item, keyed by its `graphic`, so that something can give
/// it a meaning: a potion drunk, a lever pulled, a sign read, a deed placed.
/// `world::tick::shipped_items` is what answers it today. The engine's own answer
/// here is *nothing*, because a bare graphic has no behaviour until a shard gives
/// it one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ItemUsed {
    /// The item that was double-clicked.
    pub item:      Serial,
    /// Its graphic, so a reader matches on the tile with no lookup.
    pub graphic:   Graphic,
    /// Semantic item kind, when the item's presentation is registry-backed.
    pub item_kind: Option<ItemKindId>,
    /// Semantic material grade, when the kind accepts one.
    pub material:  Option<MaterialId>,
    /// The mobile that used it.
    pub by:        Serial,
}

/// A player double-clicked a mobile the engine has no deeper interaction for —
/// the mobile counterpart of [`ItemUsed`].
///
/// Double-clicking a mobile opens its paperdoll (that still happens); this event
/// fires *alongside* it, so the pack can give a mobile a meaning the engine has no
/// default for — a quest giver offers its quest, a trainer its lessons. Keyed by
/// `body` the way `ItemUsed` is keyed by `graphic`, so a pack matches with no
/// lookup; `mobile` names the exact one for a serial-specific NPC. The core
/// default is nothing, as with a bare item.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MobileUsed {
    /// The mobile that was double-clicked.
    pub mobile: Serial,
    /// Its body, so a pack matches on the kind with no lookup.
    pub body:   Graphic,
    /// The mobile that used it.
    pub by:     Serial,
}

/// The result of a take request — how many of an item were taken from a
/// player's backpack, so a pack can verify a quest's "collect N" at turn-in.
///
/// The take is all-or-nothing: `taken` is the amount asked for when the player had
/// at least that many, otherwise `0` and nothing was removed. A collect quest
/// reads this to know the hand-over succeeded (or that the player is short).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ItemsTaken {
    /// Whose backpack it was taken from.
    pub player:    Serial,
    /// The item graphic asked for.
    pub graphic:   Graphic,
    /// Semantic item kind requested by a typed turn-in. `None` means the
    /// compatibility `Command::TakeItem` path matched presentation art only.
    pub item_kind: Option<ItemKindId>,
    /// Required material for a typed turn-in, where the kind is materialized.
    pub material:  Option<MaterialId>,
    /// How many were actually taken — the amount asked, or `0`.
    pub taken:     u16,
}

/// Emit [`MobileUsed`] for a double-clicked mobile.
///
/// Called *before* the engine decides what the click means, never in place of
/// that decision: the shop still opens, the paperdoll still opens, and the rules
/// layered over the mobile hear the click either way. Skips a self-click (opening
/// your own paperdoll is not "using an NPC") and anything that is not a mobile.
pub fn mobile_used(state: &mut WorldState, connection: ConnectionId, target_serial: Serial) {
    let Some(&player) = state.players.get(&connection) else {
        return;
    };
    let Some(target) = state.registry.entity_of(target_serial) else {
        return;
    };
    emit_mobile_used(state, player, target, target_serial);
}

/// The body of [`mobile_used`], once the entities are resolved.
pub(crate) fn emit_mobile_used(
    state: &mut WorldState,
    player: EntityId,
    target: EntityId,
    target_serial: Serial,
) {
    if target == player {
        return;
    }
    let Some(&Body { id: body, .. }) = state.registry.get::<Body>(target) else {
        return;
    };
    let Some(by) = state.registry.serial_of(player) else {
        return;
    };
    state.bus.send(MobileUsed {
        mobile: target_serial,
        body,
        by,
    });
}

/// Emit [`ItemUsed`] for an in-reach item — the last resort of
/// [`double_click`](crate::double_click), after every built-in interaction has
/// declined.
///
/// Reach is server-authoritative: the same [`in_reach`] a lift uses,
/// which resolves a ground item by its tile, a carried one by its holder, and a
/// worn one by its wearer — so a double-click across the map fires nothing. An
/// item that somehow has no `Drawn` is not a drawable item and is ignored.
pub(crate) fn item_used(state: &mut WorldState, player: EntityId, target: EntityId, target_serial: Serial) {
    if !in_reach(state, target, player) {
        return;
    }
    let Some(&Drawn { id, .. }) = state.registry.get::<Drawn>(target) else {
        return;
    };
    let Some(by) = state.registry.serial_of(player) else {
        return;
    };
    state.bus.send(ItemUsed {
        item: target_serial,
        graphic: id,
        item_kind: state.registry.get::<ItemKind>(target).map(|kind| kind.0),
        material: state.registry.get::<Material>(target).map(|material| material.0),
        by,
    });
}
