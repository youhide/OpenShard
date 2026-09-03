//! Shearing a sheep: the second thing a blade does to an animal.
//!
//! ServUO's `Sheep : ICarvable`, reached through the same `BladedItemTarget` as
//! [`carve`](crate::carve) — which is why this is a branch of that target rather
//! than a tool of its own. A blade on a *live* sheep in fleece takes the wool; a
//! blade on any other live thing is told it can only skin the dead.
//!
//! **Wool's one source on the shard.** [`spin`](crate::spin) turns it into the
//! dark yarn a loom weaves, and until this it reached a player from a vendor's
//! shelf like the cotton beside it — the leather gap of `cut`, one material over
//! and a live animal rather than a carcass.
//!
//! The fleece grows back on a timer the sheep carries ([`Shorn`]), and the body
//! is the timer's public face: `0xCF` in wool, `0xDF` shorn, the pair ServUO
//! re-derives on every `OnThink`.

use openshard_state::components::{
    SHORN_SHEEP,
    Shorn,
    WOOLLY_SHEEP,
};

use super::*;

/// ServUO's `TimeSpan.FromHours(2.0)` between one fleece and the next.
const FLEECE_TICKS: u64 = 2 * 60 * 60 * TICKS_PER_SECOND;

/// The wool a shearing pays, `0xDF8` — [`Fibre::Wool`], three balls of dark yarn
/// on the wheel.
///
/// Public for [`yield_of`](crate::carve::yield_of)'s reason: a sheep is where wool
/// enters the shard, and the reachability audit reads the fact here rather than
/// keeping a second spelling of it.
///
/// [`Fibre::Wool`]: openshard_state::components::Fibre::Wool
pub const WOOL: Graphic = Graphic(0x0DF8);

/// How much of it, on the facet the shard ships. ServUO pays `Map == Map.Felucca
/// ? 2 : 1`, the era's own reward for shearing in the dangerous world, and this
/// keeps the distinction rather than folding it into the one facet that exists:
/// a Trammel added later would otherwise quietly pay Felucca's rate.
const FELUCCA: Facet = Facet(0);
/// Wool per shearing on Felucca.
const FELUCCA_WOOL: u32 = 2;
/// Wool per shearing anywhere else.
const OTHER_WOOL: u32 = 1;

/// "This sheep is not yet ready to be shorn."
const NOT_READY: ClilocId = ClilocId(500_449);
/// "You can only skin dead creatures."
const ONLY_THE_DEAD: ClilocId = ClilocId(500_450);
/// "You place the gathered wool into your backpack."
const WOOL_STOWED: ClilocId = ClilocId(500_452);

/// Answer a blade aimed at something that is still alive.
///
/// A sheep in fleece is shorn; a shorn one says so; anything else gets ServUO's
/// line about skinning the dead. Called by [`carve`](crate::carve) once it knows
/// the target is a mobile rather than a carcass.
pub(crate) fn shear(state: &mut WorldState, shearer: EntityId, sheep: EntityId) {
    let Some(body) = state.registry.get::<Body>(sheep).copied() else {
        return;
    };
    // Its own reach, because the carve's is an item's: a mobile has no item
    // location at all, and asking `in_reach` about one refuses every sheep on
    // the shard. The distance is the crate's own, as `equip` measures a mobile.
    let (Some(&Position(sheep_at)), Some(&Position(shearer_at))) = (
        state.registry.get::<Position>(sheep),
        state.registry.get::<Position>(shearer),
    ) else {
        return;
    };
    if state.facet_of(sheep) != state.facet_of(shearer)
        || !in_range(sheep_at, shearer_at, crate::drag::ITEM_REACH)
    {
        return;
    }
    if body.id != WOOLLY_SHEEP {
        // A sheep between fleeces is refused by its own line; everything else
        // alive is refused by the blade's.
        let refusal = if body.id == SHORN_SHEEP {
            NOT_READY
        } else {
            ONLY_THE_DEAD
        };
        state.localized_message(shearer, refusal, "");
        return;
    }
    let amount = if state.facet_of(sheep) == FELUCCA {
        FELUCCA_WOOL
    } else {
        OTHER_WOOL
    };
    // The fleece comes off first and the wool is handed over second: a pack too
    // full to take it drops the difference at the shearer's feet rather than
    // leaving a sheep that can be sheared again in the same breath.
    redraw_body(state, sheep, SHORN_SHEEP);
    state.registry.insert(
        sheep,
        Shorn {
            regrows: state.ticks + FLEECE_TICKS,
        },
    );
    let stowed = state
        .registry
        .serial_of(shearer)
        .and_then(|owner| backpack_of(state, owner))
        .map_or(0, |pack| give(state, pack, WOOL, Hue(0), amount).given);
    if stowed != 0 {
        state.localized_message(shearer, WOOL_STOWED, "");
    }
    // ServUO's `AddToBackpack` puts what will not fit at the mobile's own feet,
    // and a shearer with no pack at all — a creature, a staff test fixture — is
    // the same case.
    drop_at_feet(state, shearer, amount - stowed);
}

/// Put every sheep whose two hours are up back in fleece.
///
/// ServUO re-derives the body from `NextWoolTime` on every `OnThink`; this is
/// the same fact asked once a tick, beside [`advance_spins`](crate::advance_spins)
/// and the same shape — a scan over the handful of sheep mid-timer rather than a
/// queue of deadlines to keep in step.
pub fn regrow_fleece(state: &mut WorldState) {
    let now = state.ticks;
    let ready: Vec<EntityId> = state
        .registry
        .query::<Shorn>()
        .filter(|(_, shorn)| shorn.regrows <= now)
        .map(|(entity, _)| entity)
        .collect();
    for sheep in ready {
        state.registry.remove::<Shorn>(sheep);
        redraw_body(state, sheep, WOOLLY_SHEEP);
    }
}

/// Lay what a mobile could not hold on its own tile.
fn drop_at_feet(state: &mut WorldState, mobile: EntityId, amount: u32) {
    if amount == 0 {
        return;
    }
    let (Some(&Position(at)), facet) = (state.registry.get::<Position>(mobile), state.facet_of(mobile))
    else {
        return;
    };
    // Clamped to one pile: no shearing pays more than two of anything, so the
    // clamp can never bite.
    let amount = u16::try_from(amount).unwrap_or(u16::MAX);
    spawn_item(state, WOOL, Hue(0), amount, true, at, facet);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shorn_sheep_is_not_a_woolly_one() {
        // The two bodies *are* the timer's public face: one value for both would
        // make every sheep on the shard permanently shearable.
        assert_ne!(WOOLLY_SHEEP, SHORN_SHEEP);
    }

    #[test]
    fn wool_is_the_fibre_a_wheel_spins() {
        assert_eq!(
            openshard_state::components::Fibre::from_graphic(WOOL),
            Some(openshard_state::components::Fibre::Wool)
        );
    }
}
