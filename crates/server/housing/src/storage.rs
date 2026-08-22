//! Lockdowns and secures: what a house will hold, and who may open it.
//!
//! An item dropped inside a house is ordinarily loose, liftable by anyone who
//! walks in and eventually decayed. A **lockdown** is one pinned in place; a
//! **secure** is a lockdown that is also a container, opening only for people the
//! house names. Both are counted against an allowance, which is what stops a
//! house being a bank box with a roof.
//!
//! # The allowance is derived from the multi, not declared per house
//!
//! ServUO's is a table: `HousePlacementEntry` carries a lockdown count and a
//! storage count for each of its thirty-odd multi ids, hand-written beside the
//! price and the placement offset. That is the same per-house-type table the
//! door positions and the sign offsets are, and it is not copied here for the
//! same reason — it is content, and no client file says it.
//!
//! What the table *is*, when you plot it, is roughly linear in the house's own
//! area. Three of its rows, against the `Area` rectangles the matching house
//! class declares:
//!
//! | house | tiles | ServUO lockdowns | per tile |
//! |---|---|---|---|
//! | small old house | 52 | 212 | 4.08 |
//! | small tower | 59 | 290 | 4.92 |
//! | two-storey villa | 125 | 550 | 4.40 |
//!
//! So [`LOCKDOWNS_PER_TILE`] is **4**, and the derived numbers land within a
//! sixth of the reference's on every row — a shard's own tuning knob rather than
//! a promise of parity, and one an operator can turn without editing a table of
//! thirty ids.
//!
//! The area is [`tiles_of`](crate::tiles_of), which is deduplicated by `(x, y)`:
//! a two-storey house is not counted twice for having two floors over one
//! column.
//!
//! # Two ceilings, and the second is ServUO's own AoS rule
//!
//! Post-AoS the reference caps two separate things: how many items are locked
//! down, and how many are *inside* the secures. Its own table has the second at
//! exactly twice the first on every row, so [`STORAGE_PER_LOCKDOWN`] is 2 and
//! there is one number to derive.

use openshard_entities::EntityId;
use openshard_movement::Tile;
use openshard_protocol::serial::Serial;
use openshard_state::WorldState;
use openshard_state::components::{Contained, Container, House, LockedDown, Position, Standing};

/// How many lockdowns a house gets per tile of its own footprint. See the module
/// header for where the 4 comes from.
pub const LOCKDOWNS_PER_TILE: usize = 4;

/// How many items the secures may hold between them, per lockdown. ServUO's AoS
/// table has this at exactly 2 on every row.
pub const STORAGE_PER_LOCKDOWN: usize = 2;

/// What a house will hold.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Allowance {
    /// How many items may be locked down, secures included.
    pub lockdowns: usize,
    /// How many items may sit inside the secures, between them.
    pub storage: usize,
}

/// Why a lockdown or a secure was refused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StorageRefusal {
    /// The actor is not trusted enough. A co-owner and above, like every other
    /// change to a house.
    NotYours,
    /// The item is not standing inside this house.
    NotInThisHouse,
    /// It is already locked down, or already loose.
    NoChange,
    /// A secure has to be a container.
    NotAContainer,
    /// The house has no room for another lockdown.
    NoRoom,
}

impl StorageRefusal {
    /// What to say to whoever tried.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::NotYours => "That is not your house to change.",
            Self::NotInThisHouse => "That is not inside this house.",
            Self::NoChange => "That is already the way you are asking for.",
            Self::NotAContainer => "Only a container can be made secure.",
            Self::NoRoom => "This house cannot hold any more.",
        }
    }
}

/// What a footprint of `tiles` tiles is worth, in lockdowns.
///
/// Called once, at placement, and the answer is stored on the
/// [`House`](openshard_state::components::House) component — D2's "computed at
/// placement and stored" one level up. Recomputing it per drop would be asking
/// the multi table on a path that has no business holding one.
#[must_use]
pub const fn allowance_for(tiles: usize) -> Allowance {
    let lockdowns = tiles * LOCKDOWNS_PER_TILE;
    Allowance {
        lockdowns,
        storage: lockdowns * STORAGE_PER_LOCKDOWN,
    }
}

/// What a placed house will hold, read off its component.
///
/// Zero for a house placed by a shard with no client files: the area came from
/// the multi table, and a house whose size this shard cannot know is one nothing
/// may be locked down in. That is the right direction — the alternative is an
/// unbounded allowance on exactly the shards that can check nothing else either.
#[must_use]
pub fn allowance(state: &WorldState, house: EntityId) -> Allowance {
    let lockdowns = state
        .registry
        .get::<House>(house)
        .map_or(0, |entry| entry.lockdowns as usize);
    Allowance {
        lockdowns,
        storage: lockdowns * STORAGE_PER_LOCKDOWN,
    }
}

/// The ground a house covers, or nothing if it is not a house.
fn house_tiles(state: &WorldState, house: EntityId) -> Vec<Tile> {
    let (Some(entry), Some(&Position(at))) = (
        state.registry.get::<House>(house),
        state.registry.get::<Position>(house),
    ) else {
        return Vec::new();
    };
    let shape = crate::design::shape_of_house(state, house);
    crate::tiles_of(state, at, entry.multi, shape.as_deref())
}

/// Everything locked down in a house, secures included.
#[must_use]
pub fn locked_down(state: &WorldState, house: EntityId) -> Vec<EntityId> {
    let Some(serial) = state.registry.serial_of(house) else {
        return Vec::new();
    };
    state
        .registry
        .query::<LockedDown>()
        .filter(|(_, pinned)| pinned.house == serial)
        .map(|(entity, _)| entity)
        .collect()
}

/// How many items are sitting inside a house's secures.
///
/// One level deep, not the whole subtree: a bag inside a secure chest is one
/// item against the allowance and its own contents are the bag's problem. That
/// is `items::capacity`'s own split — the *container* ceiling counts the subtree,
/// and this is a house ceiling counting what was put in the house.
#[must_use]
pub fn stored(state: &WorldState, house: EntityId) -> usize {
    let secures: Vec<Serial> = locked_down(state, house)
        .into_iter()
        .filter(|&item| {
            state
                .registry
                .get::<LockedDown>(item)
                .is_some_and(|pinned| pinned.secure.is_some())
        })
        .filter_map(|item| state.registry.serial_of(item))
        .collect();
    state
        .registry
        .query::<Contained>()
        .filter(|(_, held)| secures.contains(&held.container))
        .count()
}

/// Whether `who` may open a secure container.
///
/// The rule itself is
/// [`WorldState::may_open_secure`](openshard_state::WorldState::may_open_secure),
/// which lives beside the data for the reason [`Standing`] does: the container's
/// double-click is `openshard-items`', which has no business depending on this
/// crate. Named here too so that a reader of the storage rules finds it where
/// the other four are.
#[must_use]
pub fn may_open(state: &WorldState, who: EntityId, container: EntityId) -> bool {
    state.may_open_secure(who, container)
}

/// Pin an item inside a house, or make it a secure container.
///
/// `secure` is the least standing that may open it, and `None` makes a plain
/// lockdown. Changing an existing lockdown's access level is allowed and costs
/// nothing — it is the same item on the same list.
pub fn lock_down(
    state: &mut WorldState,
    actor: EntityId,
    house: EntityId,
    item: EntityId,
    secure: Option<Standing>,
) -> Result<(), StorageRefusal> {
    let serial = trusted(state, actor, house)?;
    if secure.is_some() && !state.registry.has::<Container>(item) {
        return Err(StorageRefusal::NotAContainer);
    }
    // Inside the house, and loose. A thing in somebody's pack is not in the
    // house even when they are standing in it.
    let Some(&Position(at)) = state.registry.get::<Position>(item) else {
        return Err(StorageRefusal::NotInThisHouse);
    };
    if state.facet_of(item) != state.facet_of(house)
        || !house_tiles(state, house).contains(&Tile::new(at.x, at.y))
    {
        return Err(StorageRefusal::NotInThisHouse);
    }
    match state.registry.get::<LockedDown>(item) {
        // Already on the list: only the access level can change, and it costs
        // nothing against the allowance.
        Some(pinned) if pinned.house == serial => {
            if pinned.secure == secure {
                return Err(StorageRefusal::NoChange);
            }
            if secure.is_some() && !state.registry.has::<Container>(item) {
                return Err(StorageRefusal::NotAContainer);
            }
        }
        // Somebody else's lockdown, standing inside this house. Refused rather
        // than stolen: two houses whose footprints touch is a placement bug, and
        // taking the item would hide it.
        Some(_) => return Err(StorageRefusal::NotInThisHouse),
        None => {
            if locked_down(state, house).len() >= allowance(state, house).lockdowns {
                return Err(StorageRefusal::NoRoom);
            }
        }
    }
    state.registry.insert(
        item,
        LockedDown {
            house: serial,
            secure,
        },
    );
    // Off the decay clock. The sweep skips a lockdown anyway, but leaving the
    // component behind would restart the rot the moment it is released — with
    // whatever remained of a twenty-minute timer set before the house was built.
    state.registry.remove::<openshard_state::components::Decays>(item);
    Ok(())
}

/// Let an item go loose again.
pub fn release(
    state: &mut WorldState,
    actor: EntityId,
    house: EntityId,
    item: EntityId,
) -> Result<(), StorageRefusal> {
    let serial = trusted(state, actor, house)?;
    if state
        .registry
        .get::<LockedDown>(item)
        .is_none_or(|pinned| pinned.house != serial)
    {
        return Err(StorageRefusal::NoChange);
    }
    state.registry.remove::<LockedDown>(item);
    Ok(())
}

/// Whether one more item will fit inside a house's secures.
///
/// The drop path asks
/// [`WorldState::secure_has_room`](openshard_state::WorldState::secure_has_room)
/// instead — it holds a container rather than a house, and it is in a crate that
/// does not depend on this one. Named here so the storage rules are readable in
/// one place.
#[must_use]
pub fn has_room_for(state: &WorldState, house: EntityId, more: usize) -> bool {
    stored(state, house) + more <= allowance(state, house).storage
}

/// The house's serial, if `actor` is trusted enough to change it.
fn trusted(state: &WorldState, actor: EntityId, house: EntityId) -> Result<Serial, StorageRefusal> {
    let (Some(entry), Some(who)) = (
        state.registry.get::<House>(house),
        state.registry.serial_of(actor),
    ) else {
        return Err(StorageRefusal::NotYours);
    };
    if entry.standing_of(who, state.is_staff(actor)) < Standing::CoOwner {
        return Err(StorageRefusal::NotYours);
    }
    state.registry.serial_of(house).ok_or(StorageRefusal::NotYours)
}
