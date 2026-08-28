//! The backpack: finding it, putting things in it, taking things out.
//!
//! Every "hand this to a player" rule needs the same two steps — locate the
//! container on the backpack layer, then merge or place into it — and every
//! "collect N of these" rule needs the same all-or-nothing draw against it. Both
//! were written inline where they were first wanted, each with its own local copy
//! of the layer number; the quest turn-in would have made a third. One copy is a
//! constant, two is a coincidence, and three is how the reward path and the
//! turn-in path start disagreeing about what a backpack is.

use super::*;
use openshard_protocol::wire::{Graphic, Hue};

/// The paperdoll layer a backpack is worn on. ServUO's `Layer.Backpack`.
pub const BACKPACK_LAYER: Layer = Layer(0x15);

/// The world art of the ordinary backpack.
pub const BACKPACK_GRAPHIC: Graphic = Graphic(0x0E75);
/// The container gump the client uses for an ordinary backpack.
pub const BACKPACK_GUMP: Graphic = Graphic(0x003C);

/// The container a mobile wears as its backpack, if it has one.
///
/// A mobile without one is not an error: a creature has no pack, and a reward or
/// a turn-in aimed at it simply does nothing rather than dropping loot on the
/// floor of wherever it happened to be standing.
#[must_use]
pub fn backpack_of(state: &WorldState, mobile: Serial) -> Option<Serial> {
    equipped_items(state, mobile)
        .find(|(item, equipped)| equipped.layer == BACKPACK_LAYER && state.registry.has::<Container>(*item))
        .and_then(|(item, _)| state.registry.serial_of(item))
}

/// Put an item into a mobile's backpack: merged onto a like pile when
/// `stackable` (gold, reagents), else placed as a discrete piece.
///
/// Returns whether it landed. `false` means the mobile wears no backpack **or the
/// pack will not hold it**, and the caller decides what that means — nothing is
/// spilled on the ground here, because a reward that quietly becomes litter at the
/// giver's feet is worse than one that visibly did not arrive.
///
/// # The full-pack half arrived late
///
/// Until 2026-08-16 this could only fail for want of a pack at all, so the harvest
/// system's "your pack is full" line was a line nothing could reach: a miner mined
/// into a backpack with no bottom. See [`check_hold`](crate::check_hold), and note
/// that a *merge* onto a pile already in there costs no slot — only weight — which
/// is why the two arms below ask different questions.
pub fn give_to_backpack(
    state: &mut WorldState,
    mobile: Serial,
    graphic: Graphic,
    hue: Hue,
    amount: u16,
    stackable: bool,
) -> bool {
    let Some(backpack) = backpack_of(state, mobile) else {
        return false;
    };
    if let Some(owner) = state.registry.entity_of(mobile) {
        if !room_for(state, owner, backpack, graphic, hue, amount, stackable) {
            return false;
        }
    }
    // Currency and ammunition still belong on an existing pile when their
    // source omitted the flag (for example, a legacy one-arrow save).
    if stackable || intrinsically_stackable(graphic) {
        crate::give(state, backpack, graphic, hue, u32::from(amount)).is_complete()
    } else {
        crate::place_one(state, backpack, graphic, hue, amount).is_some()
    }
}

/// Create one or more empty containers in a mobile's backpack.
///
/// A container cannot go through [`give_to_backpack`]: that path deliberately
/// creates a bare item from a graphic.  Keeping this separate means the staff
/// catalogue's backpack is a real bag rather than an inert `0x0E75` picture.
/// The same capacity check applies before any item is made.
pub fn give_containers_to_backpack(
    state: &mut WorldState,
    mobile: Serial,
    graphic: Graphic,
    gump: Graphic,
    hue: Hue,
    amount: u16,
) -> bool {
    if amount == 0 {
        return true;
    }
    let Some(backpack) = backpack_of(state, mobile) else {
        return false;
    };
    let Some(owner) = state.registry.entity_of(mobile) else {
        return false;
    };
    let each = u32::from(state.tiles().item_weight(graphic.0)) * 100;
    let stones = u16::try_from(each.saturating_mul(u32::from(amount)) / 100).unwrap_or(u16::MAX);
    if crate::check_hold(state, owner, backpack, usize::from(amount), stones).is_some() {
        return false;
    }

    for _ in 0..amount {
        let Ok((entity, _serial)) = state.registry.spawn_with_serial(SerialKind::Item) else {
            warn!("out of item serials; container was not created");
            return false;
        };
        state.registry.insert(entity, Drawn { id: graphic, hue });
        state.registry.insert(entity, Container { gump });
        let contained = Contained {
            container: backpack,
            position: GumpPoint::new(60, 60),
            grid: GridSlot(crate::item_count(state, backpack)),
        };
        establish_item_location(state, entity, ItemLocation::contained(contained))
            .expect("a newly created container has one valid parent");
        tell_watchers_updated(state, backpack, entity);
    }
    true
}

/// Whether a pack will take what [`give_to_backpack`] is about to put in it.
///
/// The two arms differ in **slots**, not in weight. A stackable that has a pile of
/// its own art and hue already in there merges onto it and takes no new slot;
/// anything else is one more item. ServUO draws the same line — `CheckStack`
/// before `CheckHold` — and drawing it here is what stops a miner being refused a
/// hundred and twenty-sixth swing that would have gone onto the pile of ore they
/// are already carrying.
///
/// The weight is charged either way, because ore weighs what it weighs whichever
/// pile it lands on.
fn room_for(
    state: &WorldState,
    owner: EntityId,
    backpack: Serial,
    graphic: Graphic,
    hue: Hue,
    amount: u16,
    stackable: bool,
) -> bool {
    let merges = (stackable || intrinsically_stackable(graphic))
        && contained_items(state, backpack).any(|(entity, _)| {
            (state.registry.has::<Stackable>(entity)
                || state
                    .registry
                    .get::<Drawn>(entity)
                    .is_some_and(|drawn| intrinsically_stackable(drawn.id)))
                && state
                    .registry
                    .get::<Drawn>(entity)
                    .is_some_and(|drawn| drawn.id == graphic && drawn.hue == hue)
        });
    let each = if graphic == GOLD_GRAPHIC {
        crate::GOLD_WEIGHT_HUNDREDTHS
    } else {
        u32::from(state.tiles().item_weight(graphic.0)) * 100
    };
    let stones = u16::try_from(each.saturating_mul(u32::from(amount)) / 100).unwrap_or(u16::MAX);
    crate::check_hold(state, owner, backpack, usize::from(!merges), stones).is_none()
}

/// Take `amount` of a graphic out of a mobile's backpack — **all or nothing**.
///
/// Returns what was taken: `amount` when the player had at least that many across
/// however many piles, otherwise `0` with nothing removed. The partial take is
/// refused on purpose: a hand-in that swallows four of the five items asked for
/// and then reports failure has destroyed four items for nothing, and the player
/// has no way to see where they went.
///
/// Piles are drawn down oldest first, which is only the registry's order — no
/// rule depends on which identical pile is emptied.
pub fn take_from_backpack(state: &mut WorldState, mobile: Serial, graphic: Graphic, amount: u16) -> u16 {
    take_from_backpack_of_hue(state, mobile, graphic, None, amount)
}

/// [`take_from_backpack`], for a particular hue.
///
/// A crafting material's hue *is* its identity — valorite ingots and iron ingots
/// are one graphic and two colours, exactly as [`openshard_state::harvest`] keeps
/// the nine ores — so a recipe that asks for verite must not be paid in iron.
/// `None` takes any hue, which is what every caller that predates materials wants:
/// a quest that asks for ten apples does not care whether one of them was dyed.
pub fn take_from_backpack_of_hue(
    state: &mut WorldState,
    mobile: Serial,
    graphic: Graphic,
    hue: Option<Hue>,
    amount: u16,
) -> u16 {
    let Some(backpack) = backpack_of(state, mobile) else {
        return 0;
    };
    let piles: Vec<(Serial, u16)> = contained_items(state, backpack)
        .filter(|(item, _)| {
            state
                .registry
                .get::<Drawn>(*item)
                .is_some_and(|g| g.id == graphic && hue.is_none_or(|want| g.hue == want))
        })
        .filter_map(|(item, _)| {
            state
                .registry
                .serial_of(item)
                .map(|serial| (serial, crate::amount_of(state, item)))
        })
        .collect();
    let total: u32 = piles.iter().map(|(_, held)| u32::from(*held)).sum();
    if total < u32::from(amount) {
        return 0;
    }
    let mut remaining = amount;
    for (pile, held) in &piles {
        if remaining == 0 {
            break;
        }
        let take = remaining.min(*held);
        crate::consume(state, *pile, take);
        remaining -= take;
    }
    amount
}

/// How many of a graphic a mobile carries in its backpack, counting every pile.
///
/// A read, not a take: a collect objective needs to know how far along it is
/// without destroying the evidence. Only the backpack itself — a bag *inside* it
/// counts for weight (see [`carried_with`](crate::carried_with)) but not here, so
/// that "in your pack" means the one place a player can see at a glance.
///
/// Walks the containment column once. Callers asking about several graphics, or
/// about several players in a pass, should build a [`Contents`](crate::Contents)
/// and use [`carried_amount_with`] instead — otherwise it is a full column scan
/// per question.
#[must_use]
pub fn carried_amount(state: &WorldState, mobile: Serial, graphic: Graphic) -> u32 {
    carried_amount_with(state, &crate::contents_index(state), mobile, graphic)
}

/// [`carried_amount`], for a particular hue — the read half of
/// [`take_from_backpack_of_hue`], and what a craft's "have you enough metal"
/// check asks before it takes anything.
#[must_use]
pub fn carried_amount_of_hue(state: &WorldState, mobile: Serial, graphic: Graphic, hue: Option<Hue>) -> u32 {
    let Some(backpack) = backpack_of(state, mobile) else {
        return 0;
    };
    crate::contents_index(state)
        .get(&backpack)
        .into_iter()
        .flatten()
        .filter(|item| {
            state
                .registry
                .get::<Drawn>(**item)
                .is_some_and(|g| g.id == graphic && hue.is_none_or(|want| g.hue == want))
        })
        .map(|item| u32::from(crate::amount_of(state, *item)))
        .sum()
}

/// [`carried_amount`], against an index already built.
#[must_use]
pub fn carried_amount_with(
    state: &WorldState,
    contents: &crate::Contents,
    mobile: Serial,
    graphic: Graphic,
) -> u32 {
    let Some(backpack) = backpack_of(state, mobile) else {
        return 0;
    };
    contents
        .get(&backpack)
        .into_iter()
        .flatten()
        .filter(|item| {
            state
                .registry
                .get::<Drawn>(**item)
                .is_some_and(|g| g.id == graphic)
        })
        .map(|item| u32::from(crate::amount_of(state, *item)))
        .sum()
}
