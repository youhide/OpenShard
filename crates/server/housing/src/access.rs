//! House access-list rules: co-owners, friends, and bans.

use openshard_protocol::serial::Serial;
/// Where somebody stands with a house.
pub use openshard_state::Standing;
use openshard_state::components::House;

/// Maximum number of co-owners.
pub const MAX_CO_OWNERS: usize = 15;
/// Maximum number of friends.
pub const MAX_FRIENDS: usize = 140;
/// Maximum number of bans.
pub const MAX_BANS: usize = 140;

/// Why an access-list change was refused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ListRefusal {
    /// The actor lacks permission to make this change.
    NotYours,
    /// The target list is full.
    Full,
    /// An owner cannot be added, banned, or removed from their own house.
    NotTheOwner,
}

impl ListRefusal {
    /// The player-facing refusal message.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::NotYours => "That is not your house to change.",
            Self::Full => "That list is full.",
            Self::NotTheOwner => "That cannot be done to the owner.",
        }
    }
}

/// Grant friend or co-owner access, moving a player between trusted lists.
pub fn trust(
    house: &mut House,
    actor: Serial,
    who: Serial,
    standing: Standing,
    staff: bool,
) -> Result<(), ListRefusal> {
    let actor_standing = house.standing_of(actor, staff);
    let needed = match standing {
        Standing::CoOwner => Standing::Owner,
        _ => Standing::CoOwner,
    };
    if actor_standing < needed {
        return Err(ListRefusal::NotYours);
    }
    if who == house.owner {
        return Err(ListRefusal::NotTheOwner);
    }
    let (list, limit) = match standing {
        Standing::CoOwner => (&mut house.co_owners, MAX_CO_OWNERS),
        _ => (&mut house.friends, MAX_FRIENDS),
    };
    if !list.contains(&who) && list.len() >= limit {
        return Err(ListRefusal::Full);
    }
    list.insert(who);
    match standing {
        Standing::CoOwner => house.friends.remove(&who),
        _ => house.co_owners.remove(&who),
    };
    house.bans.remove(&who);
    Ok(())
}

/// Remove a player from either trusted list.
pub fn distrust(house: &mut House, actor: Serial, who: Serial, staff: bool) -> Result<(), ListRefusal> {
    let actor_standing = house.standing_of(actor, staff);
    let needed = if house.co_owners.contains(&who) {
        Standing::Owner
    } else {
        Standing::CoOwner
    };
    if actor_standing < needed {
        return Err(ListRefusal::NotYours);
    }
    if who == house.owner {
        return Err(ListRefusal::NotTheOwner);
    }
    house.co_owners.remove(&who);
    house.friends.remove(&who);
    Ok(())
}

/// Ban a player and remove any trusted access they had.
pub fn ban(house: &mut House, actor: Serial, who: Serial, staff: bool) -> Result<(), ListRefusal> {
    if house.standing_of(actor, staff) < Standing::CoOwner {
        return Err(ListRefusal::NotYours);
    }
    if who == house.owner {
        return Err(ListRefusal::NotTheOwner);
    }
    if !house.bans.contains(&who) && house.bans.len() >= MAX_BANS {
        return Err(ListRefusal::Full);
    }
    house.bans.insert(who);
    house.co_owners.remove(&who);
    house.friends.remove(&who);
    Ok(())
}

/// Remove a ban without granting any other access.
pub fn unban(house: &mut House, actor: Serial, who: Serial, staff: bool) -> Result<(), ListRefusal> {
    if house.standing_of(actor, staff) < Standing::CoOwner {
        return Err(ListRefusal::NotYours);
    }
    house.bans.remove(&who);
    Ok(())
}
