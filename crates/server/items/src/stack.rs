use openshard_state::components::{
    CraftedBy,
    ItemAffixes,
    PoisonCharges,
    Quality,
};
use openshard_state::weapon::{
    ARROW,
    BOLT,
};

use super::*;

/// ServUO's three coin clinks, chosen by the pile that was set down.
const GOLD_DROP_SOUNDS: [SoundId; 3] = [SoundId(0x02E4), SoundId(0x02E5), SoundId(0x02E6)];

/// The sound an item makes when it lands, or the destination's ordinary sound
/// when the item has no sound of its own.
///
/// Gold is the one classic item whose sound depends on its amount: one coin,
/// a handful, and a pile each ring differently.  This is ServUO's
/// `Gold.GetDropSound`; carrying the rule beside stack merging prevents the
/// backpack, ground and container paths from choosing different coin sounds.
pub fn drop_sound(graphic: Graphic, amount: u16, fallback: SoundId) -> SoundId {
    if graphic != GOLD_GRAPHIC {
        return fallback;
    }
    match amount {
        0 | 1 => GOLD_DROP_SOUNDS[0],
        2..=5 => GOLD_DROP_SOUNDS[1],
        _ => GOLD_DROP_SOUNDS[2],
    }
}

/// Whether an item's art is inherently stackable even when an older save or a
/// generic spawn path omitted its [`Stackable`] component.  Gold established
/// this rule first; arrows and bolts need the same treatment because a single
/// piece has no amount on the wire, so neither the client nor a restored item
/// can infer the fact from its count alone.
pub const fn intrinsically_stackable(graphic: Graphic) -> bool {
    matches!(graphic, GOLD_GRAPHIC | ARROW | BOLT)
}

/// Whether two items are one pile waiting to happen: both stackable, equal
/// semantic identity where it is known, and not the same entity.
///
/// Unmigrated legacy pairs retain their old graphic/hue comparison at that one
/// compatibility seam. A pair with semantic identities never falls back to art:
/// two different kinds may share a client drawing without becoming one good.
pub fn can_stack(state: &WorldState, a: EntityId, b: EntityId) -> bool {
    let same_identity = match (
        state.registry.get::<ItemKind>(a),
        state.registry.get::<ItemKind>(b),
    ) {
        (Some(kind_a), Some(kind_b)) => {
            kind_a == kind_b && state.registry.get::<Material>(a) == state.registry.get::<Material>(b)
        }
        (None, None) => state.registry.get::<Drawn>(a) == state.registry.get::<Drawn>(b),
        _ => false,
    };
    a != b
        && same_identity
        && stack_compatible_instance_state(state, a)
        && stack_compatible_instance_state(state, b)
        && (state.registry.has::<Stackable>(a) && state.registry.has::<Stackable>(b)
            // Older saves can contain a bare coin, arrow or bolt.  Keep those
            // single items usable too; `same_drawn` above already proves both
            // items have the same intrinsically-stackable art.
            || state
                .registry
                .get::<Drawn>(a)
                .is_some_and(|drawn| intrinsically_stackable(drawn.id)))
}

/// Whether `item` carries no per-instance fact that would be erased by a pile
/// merge.  An ordinary resource pile deliberately has none of these; a maker's
/// mark, quality, custom affix, weapon override or poison charge belongs to one
/// particular object and must survive even if its drawing and kind match.
pub(crate) fn stack_compatible_instance_state(state: &WorldState, item: EntityId) -> bool {
    !state.registry.has::<CraftedBy>(item)
        && !state.registry.has::<Quality>(item)
        && !state.registry.has::<ItemAffixes>(item)
        && !state.registry.has::<Weapon>(item)
        && !state.registry.has::<PoisonCharges>(item)
}

/// Merge a held stack onto another stack, on the ground or inside a container.
/// See `can_stack`.
pub fn merge_onto(state: &mut WorldState, connection: ConnectionId, held: HeldItem, target: EntityId) {
    let Some(&player) = state.players.get(&connection) else {
        bounce(state, connection, held, DragCancelReason::Other);
        return;
    };

    // Where the target lives decides how it is reached and redrawn. On the
    // ground it is reach-checked against the player's tile and redrawn with a
    // `0x1A`; inside a container it is reach-checked through its container and
    // redrawn with a `0x25` to every open gump, as `give` does.
    let target_location = item_location(state, target);
    if let Some(ItemLocation::Settled(SettledItemLocation::Ground {
        facet,
        position: target_pos,
    })) = target_location
    {
        let Some(&Position(player_pos)) = state.registry.get::<Position>(player) else {
            bounce(state, connection, held, DragCancelReason::Other);
            return;
        };
        if facet != state.facet_of(player) || !in_range(target_pos, player_pos, ITEM_REACH) {
            bounce(state, connection, held, DragCancelReason::OutOfRange);
            return;
        }
        let left = merge_amounts(state, held.entity, target);
        if left > 0 {
            // The target filled up. What did not fit goes back where it came
            // from rather than onto the floor under the pile — the drop is
            // refused, in Sphere's sense of "the item did not all stack, do
            // something else with it", and the player keeps every coin.
            debug!(left, "stack filled; the remainder bounced");
            bounce(state, connection, held, DragCancelReason::Other);
            return;
        }
        despawn_item(state, held.entity);
        let sound = drop_sound(
            state.registry.get::<Drawn>(target).expect("a stack has art").id,
            amount_of(state, target),
            SoundId(0x0042),
        );
        state.play_sound_to(player, sound);
        debug!("stacks merged");
    } else if let Some(ItemLocation::Settled(SettledItemLocation::Contained(contained))) = target_location {
        let container = contained.container;
        let reachable = state
            .registry
            .entity_of(container)
            .is_some_and(|c| in_reach(state, c, player));
        if !reachable {
            bounce(state, connection, held, DragCancelReason::OutOfRange);
            return;
        }
        let left = merge_amounts(state, held.entity, target);
        if left > 0 {
            debug!(left, "stack filled in a container; the remainder bounced");
            bounce(state, connection, held, DragCancelReason::Other);
            return;
        }
        // The dragged stack was on a cursor, on no screen and in no gump, so
        // despawning it needs no packet of its own.
        despawn_item(state, held.entity);
        let sound = drop_sound(
            state.registry.get::<Drawn>(target).expect("a stack has art").id,
            amount_of(state, target),
            SoundId(0x0048),
        );
        state.play_sound_to(player, sound);
        debug!("stacks merged in a container");
    } else {
        // Worn, or nowhere placeable: nothing to merge onto.
        bounce(state, connection, held, DragCancelReason::Other);
    }
}

/// The most one pile may hold — ServUO's `Item.WillStack` cap.
///
/// Below the `u16` an [`Amount`] is stored in and the wire carries, on purpose:
/// the ceiling has to be a number the arithmetic can pass without wrapping, and
/// 60,000 is the one both this engine and the reference can name.
pub const MAX_STACK: u16 = 60_000;

/// Fold as much of the held stack into the target as the target can hold.
/// Returns what is **left on the held item** — zero when it all went in.
///
/// The references disagree here and Sphere has the better answer. ServUO refuses
/// the merge outright when the sum would pass its cap (`WillStack`), so a full
/// pile simply will not take a drop. Sphere's `CItem::Stack` fills the
/// destination to its maximum, leaves the remainder on the source, and reports
/// that it did not all fit — which loses nothing and needs no explanation to the
/// player. What must not happen is what happened here before: clamping the sum
/// and despawning the source, which quietly destroyed the difference. Dropping
/// 50,000 gold onto 50,000 left one pile of 65,535 and 34,465 gone.
fn merge_amounts(state: &mut WorldState, held: EntityId, target: EntityId) -> u16 {
    // Normalise an old singleton as soon as it participates in a merge, so its
    // corrected state is retained by the next save (and a partial merge leaves
    // a usable remainder on the cursor).
    if state
        .registry
        .get::<Drawn>(target)
        .is_some_and(|drawn| intrinsically_stackable(drawn.id))
    {
        state.registry.insert(target, Stackable);
        state.registry.insert(held, Stackable);
    }
    let held_amount = amount_of(state, held);
    let moved = fill_stack(state, target, u32::from(held_amount));
    let left = held_amount - moved;
    if left > 0 && left != held_amount {
        set_stack_amount(state, held, left);
    }
    left
}

/// How many an item is: its [`Amount`], or one if it has none.
pub fn amount_of(state: &WorldState, item: EntityId) -> u16 {
    state.registry.get::<Amount>(item).map_or(1, |a| a.0)
}

/// Whether `amount` can be represented by one live pile.
///
/// Zero is absence, not a singleton, and a larger quantity has to be split
/// over several entities before any of them is published.
#[must_use]
pub const fn is_valid_stack_amount(amount: u16) -> bool {
    amount > 0 && amount <= MAX_STACK
}

/// Set a live stack's size and publish the change to its current viewers.
///
/// Whole-pile removal is deliberately not encoded as `amount == 0`: deleting
/// an entity also owns its cursor, container and sector cleanup, so the caller
/// must take that explicit path through [`despawn_item`].
///
/// # Panics
///
/// Panics when `amount` is zero or exceeds [`MAX_STACK`]. Both are programmer
/// errors after the operation has selected one physical pile.
pub fn set_stack_amount(state: &mut WorldState, item: EntityId, amount: u16) {
    write_stack_amount(state, item, amount);
    match item_location(state, item) {
        Some(ItemLocation::Settled(SettledItemLocation::Ground { .. })) => {
            redraw_ground_item(state, item);
        }
        Some(ItemLocation::Settled(SettledItemLocation::Contained(contained))) => {
            tell_watchers_updated(state, contained.container, item);
        }
        Some(ItemLocation::Settled(SettledItemLocation::Equipped(_)))
        | Some(ItemLocation::Held { .. })
        | None => {}
    }
}

/// Install the quantity of an entity that has not been published yet.
///
/// Constructors and prepared transactions use this door while presentation is
/// still incomplete. Once an item is live, [`set_stack_amount`] owns both the
/// component write and its viewer update.
pub(crate) fn initialize_stack_amount(state: &mut WorldState, item: EntityId, amount: u16) {
    write_stack_amount(state, item, amount);
}

/// Commit the original half of a prepared split without publishing an
/// intermediate amount at the old location.
///
/// The lift commits the cursor relocation immediately after this write. A
/// container update here would re-add the just-lifted serial to the lifter's
/// open gump before the relocation removes it.
pub(crate) fn commit_prepared_split_amount(state: &mut WorldState, item: EntityId, amount: u16) {
    write_stack_amount(state, item, amount);
}

/// Fill one existing pile and return how much of `offered` entered it.
pub(crate) fn fill_stack(state: &mut WorldState, item: EntityId, offered: u32) -> u16 {
    let room = MAX_STACK.saturating_sub(amount_of(state, item));
    let moved = offered.min(u32::from(room)) as u16;
    if moved > 0 {
        set_stack_amount(state, item, amount_of(state, item) + moved);
    }
    moved
}

/// The quantity result of taking from one pile.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct StackTake {
    pub taken: u16,
    pub left:  u16,
}

/// Take at most `requested` from one pile without deleting an emptied entity.
///
/// A positive remainder is committed and published here. `left == 0` tells
/// the owning operation to despawn through its location-aware cleanup door.
pub(crate) fn take_stack_amount(state: &mut WorldState, item: EntityId, requested: u16) -> StackTake {
    let have = amount_of(state, item);
    let taken = have.min(requested);
    let left = have - taken;
    if taken > 0 && left > 0 {
        set_stack_amount(state, item, left);
    }
    StackTake { taken, left }
}

fn write_stack_amount(state: &mut WorldState, item: EntityId, amount: u16) {
    assert!(
        is_valid_stack_amount(amount),
        "one live pile must contain 1..={MAX_STACK} items, got {amount}"
    );
    if amount > 1 {
        state.registry.insert(item, Amount(amount));
    } else {
        state.registry.remove::<Amount>(item);
    }
}

/// Re-send a ground item to everyone already watching it — for when its
/// amount changed and the `seen` set would otherwise suppress the redraw.
pub fn redraw_ground_item(state: &mut WorldState, item: EntityId) {
    for watcher in state.watchers_of(item) {
        let Some((connection, version)) = state.client_of(watcher) else {
            continue;
        };
        if let Some(packet) = state.draw_packet(watcher, item, version) {
            state.outbox.push(Outbound { connection, packet });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use openshard_protocol::item_kind::{
        ItemKindId,
        MaterialId,
    };
    use proptest::prelude::*;

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

    fn pile(state: &mut WorldState, amount: u16) -> EntityId {
        let item = state.registry.spawn();
        state.registry.insert(item, Stackable);
        initialize_stack_amount(state, item, amount);
        item
    }

    #[test]
    fn gold_uses_the_classic_amount_sensitive_clinks() {
        let fallback = SoundId(0x0048);
        assert_eq!(drop_sound(GOLD_GRAPHIC, 1, fallback), SoundId(0x02E4));
        assert_eq!(drop_sound(GOLD_GRAPHIC, 5, fallback), SoundId(0x02E5));
        assert_eq!(drop_sound(GOLD_GRAPHIC, 6, fallback), SoundId(0x02E6));
        assert_eq!(drop_sound(Graphic(0x0F5E), 1, fallback), fallback);
    }

    #[test]
    fn typed_piles_with_instance_facts_never_merge() {
        let mut state = world();
        let first = state.registry.spawn();
        let second = state.registry.spawn();
        for item in [first, second] {
            state.registry.insert(
                item,
                Drawn {
                    id:  Graphic(0x1BF2),
                    hue: Hue(0x08AB),
                },
            );
            state.registry.insert(item, ItemKind(ItemKindId(1)));
            state.registry.insert(item, Material(MaterialId(9)));
            state.registry.insert(item, Stackable);
        }
        assert!(
            can_stack(&state, first, second),
            "plain equivalent resources merge"
        );

        state.registry.insert(first, CraftedBy("a smith".to_owned()));
        assert!(
            !can_stack(&state, first, second),
            "a maker-marked item cannot lose its owner to a pile merge"
        );
        state.registry.remove::<CraftedBy>(first);
        state.registry.insert(first, Quality { exceptional: true });
        assert!(!can_stack(&state, first, second));
        state.registry.remove::<Quality>(first);
        state.registry.insert(first, ItemAffixes::default());
        assert!(!can_stack(&state, first, second));
    }

    #[test]
    #[should_panic(expected = "one live pile must contain")]
    fn zero_cannot_be_normalized_into_a_live_singleton() {
        let mut state = world();
        let item = state.registry.spawn();
        set_stack_amount(&mut state, item, 0);
    }

    #[test]
    #[should_panic(expected = "one live pile must contain")]
    fn one_pile_cannot_cross_the_stack_cap() {
        let mut state = world();
        let item = state.registry.spawn();
        set_stack_amount(&mut state, item, MAX_STACK + 1);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn merge_conserves_quantity_at_every_stack_boundary(
            held_amount in 1u16..=MAX_STACK,
            target_amount in 1u16..=MAX_STACK,
        ) {
            let mut state = world();
            let held = pile(&mut state, held_amount);
            let target = pile(&mut state, target_amount);

            let left = merge_amounts(&mut state, held, target);
            let expected_moved = held_amount.min(MAX_STACK - target_amount);

            prop_assert_eq!(amount_of(&state, target), target_amount + expected_moved);
            prop_assert_eq!(left, held_amount - expected_moved);
            if left > 0 {
                prop_assert_eq!(amount_of(&state, held), left);
            }
            prop_assert_eq!(
                u32::from(amount_of(&state, target)) + u32::from(left),
                u32::from(target_amount) + u32::from(held_amount),
            );
        }

        #[test]
        fn fill_and_take_match_a_small_quantity_model(
            initial in 1u16..=MAX_STACK,
            offered in 0u32..=u32::from(MAX_STACK) * 2,
            requested in 0u16..=MAX_STACK,
        ) {
            let mut state = world();
            let item = pile(&mut state, initial);

            let moved = fill_stack(&mut state, item, offered);
            let after_fill = initial + moved;
            prop_assert_eq!(moved, offered.min(u32::from(MAX_STACK - initial)) as u16);
            prop_assert_eq!(amount_of(&state, item), after_fill);

            let taken = take_stack_amount(&mut state, item, requested);
            prop_assert_eq!(taken.taken, after_fill.min(requested));
            prop_assert_eq!(u32::from(taken.taken) + u32::from(taken.left), u32::from(after_fill));
            if taken.left > 0 {
                prop_assert_eq!(amount_of(&state, item), taken.left);
            }
        }
    }
}
