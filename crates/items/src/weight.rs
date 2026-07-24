//! What a mobile is carrying: the inventory walk, and the two numbers the status
//! bar reads off it.
//!
//! One tree walk ([`carried`]) answers both questions — how much gold is in the
//! pack and how heavy everything is — so there is a single place that knows what
//! "carried" means. It is worn gear plus everything nested inside it: the
//! backpack, a bag in the backpack, the bank box. A held item on the cursor is
//! *not* counted; it is in limbo, off every screen but the holder's, and it
//! bounces back to where it came from if the drag fails.
//!
//! These are **read-site derivations**, like `combat::equipped_weapon`: nothing is
//! mirrored onto the mobile, so an item moving needs no bookkeeping to undo. The
//! cost is a walk per read, which is why the status pass that calls them runs
//! twice a second over the handful of online players rather than every tick over
//! the world.

use super::*;
use std::collections::HashMap;

/// The graphic of a gold coin — the pile every payout, corpse drop and vendor
/// sale is made of, and what the status bar counts.
pub const GOLD_GRAPHIC: u16 = 0x0EED;

/// What a single coin weighs, in hundredths of a stone.
///
/// Gold is the one item whose tiledata weight is a lie worth correcting: at the
/// tile's own weight a purse of 5,000 coins would pin a character to the floor.
/// Both references special-case it — ServUO's `Gold.DefaultWeight` is 0.02 — so a
/// 10,000 gold fortune is 200 stones, heavy but carryable, which is the classic
/// feel of a bank run.
const GOLD_WEIGHT_HUNDREDTHS: u32 = 2;

/// Who is inside what: every contained item, keyed by the container holding it.
///
/// One scan of the containment column, shared by everyone read against it. Built
/// because the obvious walk — ask "what is in this container" once per container
/// — costs a full column scan *per bag*, and the status pass reads several bags
/// for every online player twice a second. Built once, it is a scan per pass
/// instead of a scan per bag per player.
pub type Contents = HashMap<Serial, Vec<EntityId>>;

/// Index every contained item by its container. See [`Contents`].
#[must_use]
pub fn contents_index(state: &WorldState) -> Contents {
    let mut index: Contents = HashMap::new();
    for (entity, held) in state.registry.query::<Contained>() {
        index.entry(held.container).or_default().push(entity);
    }
    index
}

/// Everything a mobile carries, as `(graphic, amount)` pairs.
///
/// Worn items first, then the contents of any container among them, recursively —
/// a bag inside the backpack is carried as surely as the backpack — and the item
/// on the cursor, if it is holding one. The order is the registry's and means
/// nothing; callers sum.
///
/// **The bank box is not carried.** ServUO marks it `IsVirtualItem`, and
/// `Mobile.UpdateTotals` skips a virtual item outright — neither its weight nor
/// its gold reaches its owner. That is the whole point of a bank: what is in it
/// is *there*, not on you, which is why the banker has to tell you your balance
/// rather than the status bar showing it.
///
/// **A held item is.** `UpdateTotals` adds `m_Holding` explicitly, so lifting a
/// pile onto the cursor cannot make it lighter — the classic trick of walking
/// home holding the anvil, closed off in the reference and closed off here.
///
/// The recursion is bounded by construction: a container cannot be inside itself
/// (`drop_into_container` refuses it), and each item has exactly one home. The
/// visited set is belt-and-braces against a save restored from a store someone
/// hand-edited, where a cycle would otherwise hang the tick.
#[must_use]
pub fn carried_with(state: &WorldState, contents: &Contents, mobile: EntityId) -> Vec<(u16, u16)> {
    let Some(serial) = state.registry.serial_of(mobile) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut visited = Vec::new();
    let worn: Vec<EntityId> = state
        .registry
        .query::<Equipped>()
        .filter(|(_, worn)| worn.mobile == serial && worn.layer != BANK_LAYER)
        .map(|(entity, _)| entity)
        .collect();
    for item in worn {
        gather(state, contents, item, &mut out, &mut visited);
    }
    if let Some(held) = held_by(state, mobile) {
        gather(state, contents, held, &mut out, &mut visited);
    }
    out
}

/// What a mobile has on its cursor, if it is a player mid-drag.
///
/// A held item is in limbo — off the sector grid, off every screen but the
/// picker's — so it is on no layer and in no container, and the only record of it
/// is the drag itself.
fn held_by(state: &WorldState, mobile: EntityId) -> Option<EntityId> {
    let connection = state
        .players
        .iter()
        .find(|(_, &player)| player == mobile)
        .map(|(&connection, _)| connection)?;
    state.held.get(&connection).map(|held| held.entity)
}

/// The same for a single mobile, indexing the world itself. For one-off reads;
/// a caller doing this for every player wants [`contents_index`] once and
/// [`carried_with`] per player.
#[must_use]
pub fn carried(state: &WorldState, mobile: EntityId) -> Vec<(u16, u16)> {
    carried_with(state, &contents_index(state), mobile)
}

/// Add one item and, if it is a container, everything inside it.
fn gather(
    state: &WorldState,
    contents: &Contents,
    item: EntityId,
    out: &mut Vec<(u16, u16)>,
    visited: &mut Vec<Serial>,
) {
    let Some(serial) = state.registry.serial_of(item) else {
        return;
    };
    if visited.contains(&serial) {
        return;
    }
    visited.push(serial);
    if let Some(graphic) = state.registry.get::<Graphic>(item) {
        let amount = state.registry.get::<Amount>(item).map_or(1, |a| a.0.max(1));
        out.push((graphic.id, amount));
    }
    if !state.registry.has::<Container>(item) {
        return;
    }
    for &held in contents.get(&serial).into_iter().flatten() {
        gather(state, contents, held, out, visited);
    }
}

/// Everything inside a mobile's bank box, as `(graphic, amount)` pairs.
///
/// The other side of the line [`carried`] draws. A bank box holds a whole tree
/// like any container — a purse inside it is still banked — so it is walked the
/// same way, just from a different root.
#[must_use]
pub fn banked_with(state: &WorldState, contents: &Contents, mobile: EntityId) -> Vec<(u16, u16)> {
    let Some(serial) = state.registry.serial_of(mobile) else {
        return Vec::new();
    };
    let Some(bank) = state
        .registry
        .query::<Equipped>()
        .find(|(item, worn)| {
            worn.mobile == serial
                && worn.layer == BANK_LAYER
                && state.registry.has::<Container>(*item)
        })
        .map(|(item, _)| item)
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut visited = Vec::new();
    gather(state, contents, bank, &mut out, &mut visited);
    out
}

/// How much gold a mobile has in the bank.
///
/// The one place that counts it, for all three readers: the banker answering
/// "balance", the status bar when `bank_gold_in_status` is on, and a vendor
/// falling back to the bank for a purchase. Counts a purse *inside* the box too,
/// which a one-level scan of the box's own contents would miss.
#[must_use]
pub fn banked_gold(state: &WorldState, mobile: EntityId) -> u32 {
    banked_gold_with(state, &contents_index(state), mobile)
}

/// The same against an index built once for several mobiles.
#[must_use]
pub fn banked_gold_with(state: &WorldState, contents: &Contents, mobile: EntityId) -> u32 {
    banked_with(state, contents, mobile)
        .into_iter()
        .filter(|&(graphic, _)| graphic == GOLD_GRAPHIC)
        .map(|(_, amount)| u32::from(amount))
        .sum()
}

/// How much gold a mobile is carrying, counting every container it wears.
///
/// The status bar's gold field. ServUO's `Mobile.TotalGold` sums the whole pack
/// tree the same way — coins in a pouch in the backpack are still yours. The
/// bank box is not carried, so its gold is [`banked_gold`]'s.
#[must_use]
pub fn total_gold(state: &WorldState, mobile: EntityId) -> u32 {
    total_gold_with(state, &contents_index(state), mobile)
}

/// The same against an index built once for several mobiles.
#[must_use]
pub fn total_gold_with(state: &WorldState, contents: &Contents, mobile: EntityId) -> u32 {
    carried_with(state, contents, mobile)
        .into_iter()
        .filter(|&(graphic, _)| graphic == GOLD_GRAPHIC)
        .map(|(_, amount)| u32::from(amount))
        .sum()
}

/// What a mobile weighs, carried gear included, in stones.
///
/// The body's own weight plus everything worn and packed, each item's tiledata
/// weight times its stack amount. Summed in hundredths of a stone so gold — the
/// one item lighter than a whole stone — does not round to nothing, and so a
/// thousand light items do not accumulate a thousand roundings.
#[must_use]
pub fn total_weight(state: &WorldState, mobile: EntityId, body_weight: u16) -> u16 {
    total_weight_with(state, &contents_index(state), mobile, body_weight)
}

/// The same against an index built once for several mobiles.
#[must_use]
pub fn total_weight_with(
    state: &WorldState,
    contents: &Contents,
    mobile: EntityId,
    body_weight: u16,
) -> u16 {
    let facet = state.facet_of(mobile);
    let terrain = state
        .facets
        .get(&facet)
        .and_then(|facet| facet.terrain.as_deref());
    let hundredths: u32 = carried_with(state, contents, mobile)
        .into_iter()
        .map(|(graphic, amount)| {
            let each = if graphic == GOLD_GRAPHIC {
                GOLD_WEIGHT_HUNDREDTHS
            } else {
                // No map, no tiledata, no encumbrance — the same bargain a
                // terrainless shard already makes with its step checks.
                u32::from(terrain.map_or(0, |terrain| terrain.item_weight(graphic))) * 100
            };
            each.saturating_mul(u32::from(amount))
        })
        .sum();
    let stones = u16::try_from(hundredths / 100).unwrap_or(u16::MAX);
    stones.saturating_add(body_weight)
}
