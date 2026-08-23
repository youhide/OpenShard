//! What a container will hold, and what it refuses.
//!
//! ServUO's `Container.CheckHold` (`Server/Items/Container.cs`), which is the one
//! gate every "put this in there" path goes through and which this engine did not
//! have: a backpack took anything, forever, and the harvest system's "your pack is
//! full" line could only fire for a mobile wearing no pack at all.
//!
//! # Two ceilings, and only one of them is reliable here
//!
//! **Items** is a count — 125, ServUO's `GlobalMaxItems` — and it works on any
//! shard, because counting rows needs nothing but the registry.
//!
//! **Weight** is in stones and comes from the tiledata, which is a *client file*.
//! A shard with no map loaded weighs everything at zero, so the weight ceiling
//! silently does not apply — the same bargain [`total_weight`](crate::total_weight)
//! and the step checks already make, and the reason the item count is the half
//! worth trusting.
//!
//! # Both are recursive, and both walk upward
//!
//! A bag holds its own contents *and* counts against the pack it is in: ServUO's
//! `TotalItems` is the whole subtree, and `CheckHold` then asks the same question
//! of every container up the chain. Filling a pack with bags of bags is the trick
//! that closes off, and it is why neither half of this is a one-level scan.

use super::*;
use openshard_state::components::Container;

/// How many items a container holds — ServUO's `Container.GlobalMaxItems`.
///
/// Counted over the whole subtree, so a bag of fifty inside a backpack is
/// fifty-one against the backpack's allowance.
pub const MAX_ITEMS: usize = 125;

/// How many stones an ordinary container holds — ServUO's `GlobalMaxWeight`.
pub const MAX_WEIGHT: u16 = 400;

/// How many stones a *player's own backpack* holds from Mondain's Legacy on —
/// ServUO's `Backpack.DefaultMaxWeight`.
///
/// The one container with a ceiling of its own, and the expansion gate is real:
/// before ML a player's pack is an ordinary container at [`MAX_WEIGHT`].
pub const PLAYER_BACKPACK_MAX_WEIGHT: u16 = 550;

/// Why a container would not take something.
///
/// Two variants and not a `bool`, because the two say different things to a
/// player: one pack is full of *things* and the other is full of *stone*, and a
/// player who cannot tell which is a player who unpacks the wrong item.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Full {
    /// It already holds [`MAX_ITEMS`].
    Items,
    /// One more would put it over its weight ceiling.
    Weight,
}

impl Full {
    /// What ServUO says to a player who tried. Both are plain system lines in the
    /// reference — `SendFullItemsMessage` and `SendFullWeightMessage` — rather
    /// than clilocs, so they are English here for the same reason.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::Items => "That container cannot hold more items.",
            Self::Weight => "That container cannot hold more weight.",
        }
    }
}

/// Whether `container` will take `plus_items` more items weighing `plus_weight`
/// stones, asked on `mobile`'s behalf.
///
/// `None` means yes. Staff are never refused — ServUO's `IsStaff` guard, which is
/// what lets a game master fill a chest to test what a full one does.
///
/// Every container up the chain is asked, because putting a bag into a pack puts
/// the bag's contents into the pack too.
#[must_use]
pub fn check_hold(
    state: &WorldState,
    mobile: EntityId,
    container: Serial,
    plus_items: usize,
    plus_weight: u16,
) -> Option<Full> {
    if state.is_staff(mobile) {
        return None;
    }
    let contents = contents_index(state);
    let mut at = Some(container);
    // Bounded by construction — a container cannot be inside itself — and the
    // visited list is `carried_with`'s belt and braces against a hand-edited save
    // where a cycle would otherwise hang the tick.
    let mut visited: Vec<Serial> = Vec::new();
    while let Some(serial) = at {
        if visited.contains(&serial) {
            break;
        }
        visited.push(serial);
        if let Some(full) = holds_one_more(state, &contents, serial, plus_items, plus_weight) {
            return Some(full);
        }
        at = state
            .registry
            .entity_of(serial)
            .and_then(|entity| state.registry.get::<Contained>(entity))
            .map(|held| held.container);
    }
    None
}

/// The two ceilings for one container, with nothing above it asked about.
fn holds_one_more(
    state: &WorldState,
    contents: &Contents,
    container: Serial,
    plus_items: usize,
    plus_weight: u16,
) -> Option<Full> {
    if subtree_items(state, contents, container) + plus_items > MAX_ITEMS {
        return Some(Full::Items);
    }
    let ceiling = weight_ceiling(state, container);
    let carried = subtree_weight(state, contents, container);
    // A shard with no tiledata weighs everything at zero, so this never fires —
    // said here rather than left to be inferred from a passing test on a
    // terrainless shard.
    if carried.saturating_add(plus_weight) > ceiling {
        return Some(Full::Weight);
    }
    None
}

/// How many stones this container holds — [`PLAYER_BACKPACK_MAX_WEIGHT`] for a
/// player's own pack once Mondain's Legacy is on, [`MAX_WEIGHT`] otherwise.
fn weight_ceiling(state: &WorldState, container: Serial) -> u16 {
    let is_player_backpack = state
        .registry
        .entity_of(container)
        .and_then(|entity| state.registry.get::<Equipped>(entity))
        .filter(|worn| worn.layer == BACKPACK_LAYER)
        .and_then(|worn| state.registry.entity_of(worn.mobile))
        .is_some_and(|owner| state.registry.has::<openshard_state::components::Client>(owner));
    if is_player_backpack && state.gameplay.is_ml() {
        PLAYER_BACKPACK_MAX_WEIGHT
    } else {
        MAX_WEIGHT
    }
}

/// Everything in a container and in everything in it — ServUO's `TotalItems`.
///
/// A stack is **one** item however many are on it: fifty ingots take one slot in
/// the reference and take one here.
fn subtree_items(state: &WorldState, contents: &Contents, container: Serial) -> usize {
    let mut count = 0;
    let mut stack = vec![container];
    let mut visited: Vec<Serial> = Vec::new();
    while let Some(serial) = stack.pop() {
        if visited.contains(&serial) {
            continue;
        }
        visited.push(serial);
        for &item in contents.get(&serial).into_iter().flatten() {
            count += 1;
            if state.registry.has::<Container>(item) {
                if let Some(inner) = state.registry.serial_of(item) {
                    stack.push(inner);
                }
            }
        }
    }
    count
}

/// What everything in a container weighs, in stones, the subtree included.
fn subtree_weight(state: &WorldState, contents: &Contents, container: Serial) -> u16 {
    if state.registry.entity_of(container).is_none() {
        return 0;
    }
    let mut hundredths: u32 = 0;
    let mut stack = vec![container];
    let mut visited: Vec<Serial> = Vec::new();
    while let Some(serial) = stack.pop() {
        if visited.contains(&serial) {
            continue;
        }
        visited.push(serial);
        for &item in contents.get(&serial).into_iter().flatten() {
            hundredths = hundredths.saturating_add(item_weight_hundredths(state, item));
            if state.registry.has::<Container>(item) {
                if let Some(inner) = state.registry.serial_of(item) {
                    stack.push(inner);
                }
            }
        }
    }
    u16::try_from(hundredths / 100).unwrap_or(u16::MAX)
}

/// One item's weight in hundredths of a stone, its stack amount counted.
///
/// Gold is the special case [`total_weight`](crate::total_weight) already makes,
/// repeated rather than shared because that walk starts from a mobile and this one
/// from a container.
fn item_weight_hundredths(state: &WorldState, item: EntityId) -> u32 {
    let Some(&Drawn { id, .. }) = state.registry.get::<Drawn>(item) else {
        return 0;
    };
    let amount = state.registry.get::<Amount>(item).map_or(1, |a| a.0.max(1));
    let each = if id == GOLD_GRAPHIC {
        GOLD_WEIGHT_HUNDREDTHS
    } else {
        // A shard with no client files holds an empty table, where every graphic
        // weighs nothing — no encumbrance, the same bargain a terrainless shard
        // already makes with its step checks.
        u32::from(state.tiles().item_weight(id.0)) * 100
    };
    each.saturating_mul(u32::from(amount))
}

/// What one loose item would cost a container it were dropped into: the item
/// itself, everything inside it if it is a bag, and the weight of all of that.
///
/// ServUO's `item.TotalItems + 1` and `item.TotalWeight + item.PileWeight`, taken
/// together because the two walks are the same walk.
#[must_use]
pub fn cost_of(state: &WorldState, item: EntityId) -> (usize, u16) {
    let contents = contents_index(state);
    let own_weight = item_weight_hundredths(state, item);
    let Some(serial) = state.registry.serial_of(item) else {
        return (1, u16::try_from(own_weight / 100).unwrap_or(u16::MAX));
    };
    let inside_items = if state.registry.has::<Container>(item) {
        subtree_items(state, &contents, serial)
    } else {
        0
    };
    let inside_weight = if state.registry.has::<Container>(item) {
        subtree_weight(state, &contents, serial)
    } else {
        0
    };
    (
        inside_items + 1,
        inside_weight.saturating_add(u16::try_from(own_weight / 100).unwrap_or(u16::MAX)),
    )
}
