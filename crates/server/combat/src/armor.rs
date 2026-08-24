//! What a worn suit does for its wearer: the rules over
//! [`openshard_state::armor`]'s data.
//!
//! The table itself — every armour class's rating keyed by graphic, the coverage
//! each layer lends, and which layer a blow lands on — is data and lives in
//! `state`, because `skills` reads the same ratings to answer an Arms Lore
//! question. Two numbers come out of it here.
//!
//! `worn_armor_rating` — the wearer's total, the `ArmorRating` a status bar shows —
//! moved down to the data with it, because three crates read it now (the bar, this,
//! and Stealth, which refuses to move quietly in plate). What is left here is what
//! only a fight and a trance care about. [`absorb_physical`] is what a swing loses
//! to armour pre-AoS — ServUO's
//! `BaseWeapon.AbsorbDamage`, which rolls a hit location, lets that piece and any
//! shield eat their share, and then takes a cut of the wearer's total. Both are
//! read-site derivations: nothing is mirrored onto the mobile, so armour coming off
//! needs no undoing.

use openshard_entities::EntityId;
use openshard_protocol::wire::Layer;
use openshard_state::WorldState;
use openshard_state::armor::{
    LAYER_ARMS, LAYER_CHEST, LAYER_GLOVES, LAYER_GORGET, LAYER_HELM, LAYER_LEGS, LAYER_SHIELD, MedAllowance,
    armor_data, hit_layer, layer_coverage, piece_rating, worn_armor_rating, worn_on_layer,
};
use openshard_state::components::{Drawn, Equipped};

/// How much a mobile's worn armour gets in the way of meditating, in hundredths
/// of a rating point — ServUO's `RegenRates.GetArmorOffset`.
///
/// Each piece contributes by its material (`MedAllowance`: leather nothing,
/// studded half its rating, metal all of it) and the total is quartered. Pre-AoS
/// the shield counts too, which is the one difference from the AoS version and the
/// reason a sword-and-board mage regenerates like a warrior.
///
/// In hundredths because the quarter and the half are both fractions and the tick
/// must replay: the whole regen formula is fixed point for that reason.
#[must_use]
pub fn meditation_offset(state: &WorldState, mobile: EntityId) -> u32 {
    let Some(serial) = state.registry.serial_of(mobile) else {
        return 0;
    };
    let hundredths: u32 = state
        .registry
        .query::<Equipped>()
        .filter(|(_, worn)| worn.mobile == serial && MEDITATION_LAYERS.contains(&worn.layer))
        .map(|(item, _)| {
            let rating = u32::from(piece_rating(state, item)) * 100;
            match state
                .registry
                .get::<Drawn>(item)
                .and_then(|graphic| armor_data(graphic.id))
                .map_or(MedAllowance::All, |armor| armor.meditation)
            {
                // A piece the tables do not know is not armour, so it hinders
                // nothing — the same answer they give for its rating.
                MedAllowance::All => 0,
                MedAllowance::Half => rating / 2,
                MedAllowance::None => rating,
            }
        })
        .sum();
    hundredths / 4
}

/// The layers `meditation_offset` counts, pre-AoS: the six armour positions and
/// the shield. ServUO adds the shield only outside AoS, and it is the difference
/// between a mage who can meditate and one who cannot.
const MEDITATION_LAYERS: [Layer; 7] = [
    LAYER_SHIELD,
    LAYER_LEGS,
    LAYER_HELM,
    LAYER_GLOVES,
    LAYER_GORGET,
    LAYER_CHEST,
    LAYER_ARMS,
];

/// What a physical blow loses to the defender's armour, pre-AoS.
///
/// ServUO's `BaseWeapon.AbsorbDamage` outside AoS, in its three stages: a shield
/// eats its share first, then the piece on a rolled hit location eats its own
/// (`BaseArmor.OnHit`: half the piece's rating plus up to half again), and
/// finally the wearer's *total* rating gives up a slice sized by that same
/// location. Returns the damage that gets through.
///
/// Every roll spends the world's seeded `rng`, so a fight still replays.
pub fn absorb_physical(state: &mut WorldState, defender: EntityId, damage: u16) -> u16 {
    let total = worn_armor_rating(state, defender);
    let location = hit_layer(state.rng.below(100));
    let shield = worn_on_layer(state, defender, LAYER_SHIELD).map(|item| piece_rating(state, item));
    let piece = worn_on_layer(state, defender, location).map(|item| piece_rating(state, item));

    let mut left = u32::from(damage);
    for rating in [shield, piece].into_iter().flatten() {
        // `HalfAr + HalfAr * RandomDouble()` — half the rating always, up to half
        // again by luck. In integer terms: half, plus 0..=half.
        let half = u32::from(rating) / 2;
        let absorbed = half + if half == 0 { 0 } else { state.rng.below(half + 1) };
        left = left.saturating_sub(absorbed);
    }

    if total > 0 {
        // `from = (virtualArmor * scalar) / 2`, `to = virtualArmor * scalar`, and a
        // uniform roll between them.
        let to = u32::from(total) * layer_coverage(location) / 100;
        let from = to / 2;
        let absorbed = from
            + if to > from {
                state.rng.below(to - from + 1)
            } else {
                0
            };
        left = left.saturating_sub(absorbed);
    }
    u16::try_from(left).unwrap_or(u16::MAX)
}
