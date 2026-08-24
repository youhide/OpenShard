//! Decay, demolition, and the crate that catches what was inside.
//!
//! A shard whose houses never come down is a shard that fills up: every plot
//! taken by somebody who played for a week two years ago, and no way for anyone
//! else to build. That is what this is for, and it is also the one part of
//! housing that can destroy a player's property — which is why the crate is not
//! a later refinement but the *deletion rule* itself. See `docs/housing.md`'s D8.
//!
//! # Ticks, not a clock
//!
//! [`House::age`](openshard_state::components::House::age) counts ticks and the
//! period is
//! [`Gameplay::house_decay_ticks`](openshard_state::Gameplay::house_decay_ticks).
//! D6: everything here that measures duration counts ticks, because a tick count
//! replays and a wall clock does not.
//!
//! It is an **accumulator** rather than a deadline, which every other timer in
//! this engine is. A deadline is an absolute tick, and `WorldState::ticks` starts
//! at zero every boot — the world saves a clock in UO minutes and not a tick
//! count — so a house's deadline would be meaningless the moment it was read
//! back, and a shard that restarted nightly would have no decay at all. Counting
//! up costs one add per house per tick over a handful of houses, and it saves and
//! restores as the one number it is.
//!
//! # What refreshes it
//!
//! Opening the sign, by anybody the house trusts. ServUO refreshes on its own
//! house menu post-AoS and on the owner *walking in* before it, and the walk is
//! not copied: `house_at` is a scan over every house on the shard, which is
//! cheap when somebody presses a button and is not cheap ten times a second per
//! player. The sign is the one the reference itself moved to.

use openshard_entities::EntityId;
use openshard_protocol::serial::Serial;
use openshard_protocol::wire::{Graphic, Hue};
use openshard_protocol::world::Point;
use openshard_state::WorldState;
use openshard_state::components::{Contained, Drawn, House, HouseDoor, HouseSign, LockedDown, Position};

/// The crate a demolished house's contents land in — ServUO's `MovingCrate`,
/// graphic and hue both.
pub const CRATE_GRAPHIC: u16 = 0x0E3D;
/// Its hue, which is the only thing telling it apart from any other crate on a
/// shard.
pub const CRATE_HUE: u16 = 0x08A5;
/// The gump a crate opens as — the ordinary wooden-box window.
pub const CRATE_GUMP: u16 = 0x003C;

/// How far gone a house is.
///
/// ServUO's `DecayLevel` minus its two states this engine has no concept for:
/// `Ageless` (a staff flag) and `DemolitionPending` (a rented vendor still
/// standing inside). The six that are left are its own `GetOldDecayLevel`
/// thresholds, in per-mille of the decay period.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Condition {
    /// Under 0.5% of the period gone.
    LikeNew,
    /// Under 25%.
    Slightly,
    /// Under 50%.
    Somewhat,
    /// Under 75%.
    Fairly,
    /// Under 95%.
    Greatly,
    /// The last 5% — the one a player watches for.
    InDanger,
    /// The period is up, and the next sweep takes it.
    Collapsed,
}

impl Condition {
    /// What the sign says.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::LikeNew => "This structure is like new.",
            Self::Slightly => "This structure is slightly worn.",
            Self::Somewhat => "This structure is somewhat worn.",
            Self::Fairly => "This structure is fairly worn.",
            Self::Greatly => "This structure is greatly worn.",
            Self::InDanger => "This structure is in danger of collapsing.",
            Self::Collapsed => "This structure has collapsed.",
        }
    }

    /// The stage `per_mille` of the way through the period is.
    ///
    /// ServUO's own ladder: 1000, 950, 750, 500, 250, 5. Written as thresholds
    /// rather than as a divide, because they are not evenly spaced — the last
    /// stage is 5% of the period and the first is half a percent.
    #[must_use]
    pub const fn at(per_mille: u64) -> Self {
        match per_mille {
            1000.. => Self::Collapsed,
            950..=999 => Self::InDanger,
            750..=949 => Self::Greatly,
            500..=749 => Self::Fairly,
            250..=499 => Self::Somewhat,
            5..=249 => Self::Slightly,
            _ => Self::LikeNew,
        }
    }
}

/// How far gone `house` is.
///
/// [`LikeNew`](Condition::LikeNew) for a shard whose `house_decay_ticks` is zero,
/// which is how an operator turns decay off: no period means nothing is ever a
/// fraction of the way through one, and a division by zero is not a house that
/// falls down instantly.
#[must_use]
pub fn condition(state: &WorldState, house: EntityId) -> Condition {
    let Some(entry) = state.registry.get::<House>(house) else {
        return Condition::LikeNew;
    };
    let period = state.gameplay.house_decay_ticks;
    if period == 0 {
        return Condition::LikeNew;
    }
    Condition::at(entry.age.saturating_mul(1000) / period)
}

/// Start the clock again.
pub fn refresh(state: &mut WorldState, house: EntityId) {
    if let Some(entry) = state.registry.get_mut::<House>(house) {
        entry.age = 0;
    }
}

/// Age every house by a tick, and answer with the ones the period has run out
/// on.
///
/// The two together because they are one pass: the caller demolishes what comes
/// back, and a shard with decay switched off ages nothing rather than counting up
/// to a threshold that will never be read.
pub fn age_and_collect(state: &mut WorldState) -> Vec<EntityId> {
    if state.gameplay.house_decay_ticks == 0 {
        return Vec::new();
    }
    let houses: Vec<EntityId> = state
        .registry
        .query::<House>()
        .map(|(entity, _)| entity)
        .collect();
    let mut down = Vec::new();
    for house in houses {
        if let Some(entry) = state.registry.get_mut::<House>(house) {
            entry.age = entry.age.saturating_add(1);
        }
        if condition(state, house) == Condition::Collapsed {
            down.push(house);
        }
    }
    down
}

/// Take a house down, and return the crate its contents went into.
///
/// The one call that destroys a house, whether the owner asked for it or the
/// clock did. `None` if it was not a house, or if there was nothing to save and
/// no crate was needed.
///
/// # What comes out
///
/// Everything locked down, everything secured, and everything *inside* a secure,
/// all of it into one crate on the house's own tile. Not the loose clutter: an
/// item somebody dropped on the floor and never pinned is on the ground before
/// the demolition and on the ground after it, which is where it already was.
///
/// # What the crate does not do
///
/// It does not decay, and nothing collects it. ServUO internalises its own after
/// three hours and hands it to the owner's bank; that is a real feature and this
/// is not it. The bargain taken here is D8's: the contents survive, on the
/// ground, in one container with the owner's name on nothing. A crate that
/// rotted would be a shard that eats somebody's belongings on the day their
/// house came down, which is the failure this whole phase exists to avoid.
pub fn demolish(state: &mut WorldState, house: EntityId) -> Option<EntityId> {
    let serial = state.registry.serial_of(house)?;
    let &Position(at) = state.registry.get::<Position>(house)?;
    let facet = state.facet_of(house);

    // Everything the house was keeping, and everything inside the secures. Read
    // before anything is unpinned, because the pin is what identifies it.
    let pinned: Vec<EntityId> = state
        .registry
        .query::<LockedDown>()
        .filter(|(_, held)| held.house == serial)
        .map(|(entity, _)| entity)
        .collect();
    let secures: Vec<Serial> = pinned
        .iter()
        .filter(|&&item| {
            state
                .registry
                .get::<LockedDown>(item)
                .is_some_and(|held| held.secure.is_some())
        })
        .filter_map(|&item| state.registry.serial_of(item))
        .collect();
    let stored: Vec<EntityId> = state
        .registry
        .query::<Contained>()
        .filter(|(_, held)| secures.contains(&held.container))
        .map(|(entity, _)| entity)
        .collect();

    let crate_entity = if pinned.is_empty() {
        None
    } else {
        pack_into_a_crate(state, at, facet, &pinned, &stored)
    };

    // The doors stop being the house's. `may_pass` already opens a door whose
    // house is gone, so this changes nothing a player sees — it is the serial
    // that stops dangling, which matters the day the pool reissues it.
    let doors: Vec<EntityId> = state
        .registry
        .query::<HouseDoor>()
        .filter(|(_, door)| door.house == serial)
        .map(|(entity, _)| entity)
        .collect();
    for door in doors {
        state.registry.remove::<HouseDoor>(door);
    }

    // The sign comes down with the house; it is derived from it and means
    // nothing without it.
    let signs: Vec<EntityId> = state
        .registry
        .query::<HouseSign>()
        .filter(|(_, sign)| sign.house == serial)
        .map(|(entity, _)| entity)
        .collect();
    for sign in signs {
        take_off_the_ground(state, sign);
    }

    // The walls come out before the entity does: the footprint is derived from
    // where it stood. A house restored with no client files has none to remove,
    // and `unblock` over an empty list is the right no-op for it.
    let multi = state
        .registry
        .get::<House>(house)
        .map_or(openshard_protocol::wire::MultiId(0), |entry| entry.multi);
    // The shape it actually has: a designed house's walls are on the entity, and
    // unblocking the foundation's instead would leave every tile the two do not
    // share blocked by something that is no longer there.
    let shape = crate::design::shape_of_house(state, house);
    let footprint = crate::footprint_of(state, at, multi, shape.as_deref()).unwrap_or_default();
    crate::unblock(state, house, facet, &footprint);
    take_off_the_ground(state, house);
    crate_entity
}

/// Put everything into one crate on the house's tile.
fn pack_into_a_crate(
    state: &mut WorldState,
    at: Point,
    facet: openshard_protocol::world::Facet,
    pinned: &[EntityId],
    stored: &[EntityId],
) -> Option<EntityId> {
    let (crate_entity, crate_serial) = state
        .registry
        .spawn_with_serial(openshard_protocol::serial::SerialKind::Item)
        .ok()?;
    state.registry.insert(
        crate_entity,
        Drawn {
            id: Graphic(CRATE_GRAPHIC),
            hue: Hue(CRATE_HUE),
        },
    );
    state.registry.insert(
        crate_entity,
        openshard_state::components::Container {
            gump: Graphic(CRATE_GUMP),
        },
    );
    state.registry.insert(crate_entity, Position(at));
    state.registry.insert(crate_entity, facet);
    state.place_item(facet, crate_entity, at);

    // The secures go in whole — a chest keeps its contents, and the things
    // already inside it are left alone. Everything else goes in loose, which is
    // why `stored` is filtered against what is already going in as a container.
    let already_inside: Vec<Serial> = pinned
        .iter()
        .filter(|&&item| {
            state
                .registry
                .get::<LockedDown>(item)
                .is_some_and(|held| held.secure.is_some())
        })
        .filter_map(|&item| state.registry.serial_of(item))
        .collect();
    let loose = stored
        .iter()
        .copied()
        .filter(|&item| {
            state
                .registry
                .get::<Contained>(item)
                .is_none_or(|held| !already_inside.contains(&held.container))
        })
        .collect::<Vec<_>>();

    for (slot, &item) in pinned.iter().chain(loose.iter()).enumerate() {
        state.registry.remove::<LockedDown>(item);
        // Off the ground and off every screen that had it: it is inside a
        // container now, and a client told about both would draw it twice. Not
        // despawned — this is the half of `take_off_the_ground` that keeps the
        // entity, which is the whole point of a crate.
        forget_everywhere(state, item);
        state.registry.remove::<Position>(item);
        state.registry.insert(
            item,
            Contained {
                container: crate_serial,
                position: openshard_protocol::gump::GumpPoint::new(0, 0),
                grid: openshard_protocol::containers::GridSlot(u8::try_from(slot).unwrap_or(u8::MAX)),
            },
        );
    }
    Some(crate_entity)
}

/// Take a thing off every screen that has it and off the sector grid, leaving
/// the entity alive.
///
/// The half of [`take_off_the_ground`] a crate needs: an item going into one is
/// no longer on the ground and is still an item.
fn forget_everywhere(state: &mut WorldState, item: EntityId) {
    let Some(serial) = state.registry.serial_of(item) else {
        return;
    };
    let facet = state.facet_of(item);
    for watcher in state.watchers_of(item) {
        state.forget(watcher, item, serial);
    }
    state.unplace(facet, item);
}

/// And out of the registry as well.
///
/// `items::remove_ground_item`'s job, written here because `openshard-housing`
/// does not depend on `openshard-items` and should not start: the dependency
/// would be one function deep and would drag the whole drag-and-drop layer under
/// a crate that places buildings.
pub(crate) fn take_off_the_ground(state: &mut WorldState, item: EntityId) {
    forget_everywhere(state, item);
    state.registry.despawn(item);
}
