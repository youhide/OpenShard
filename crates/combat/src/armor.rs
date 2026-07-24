//! Armour — the rating a worn suit lends its wearer, and what a blow gets past.
//!
//! Like weapon speed and damage, an armour rating is **not** in `tiledata.mul`:
//! both references keep it per armour *class*. So it lives here, a core table
//! keyed by item graphic ported from ServUO's `BaseArmor` subclasses (each row's
//! `ArmorBase` getter, its graphics from the constructor and the `Flipable`
//! attribute), exactly the shape [`super::weapons`] uses.
//!
//! What is *not* in the table is where a piece is worn: that is the item's
//! `Equipped.layer`, which the wearer already carries, and ServUO derives its own
//! `BodyPosition` from the layer the same way. So a gorget counts as a gorget
//! because it is on the neck, not because this table says so — one fact, one
//! place.
//!
//! Two numbers come out of it. [`worn_armor_rating`] is the wearer's total, the
//! `ArmorRating` a status bar shows (ServUO's `PlayerMobile.ArmorRating`: each
//! piece scaled by how much of a body it covers). [`absorb_physical`] is what a
//! swing loses to it pre-AoS — ServUO's `BaseWeapon.AbsorbDamage`, which rolls a
//! hit location, lets that piece and any shield eat their share, and then takes a
//! cut of the wearer's total. Both are read-site derivations: nothing is mirrored
//! onto the mobile, so armour coming off needs no undoing.

use openshard_entities::EntityId;
use openshard_state::components::{Armor, Equipped, Graphic};
use openshard_state::WorldState;

/// The shield layer (UO `Layer.TwoHanded`).
pub const LAYER_SHIELD: u8 = 0x02;
/// Leggings (UO `Layer.Pants`).
pub const LAYER_LEGS: u8 = 0x04;
/// Helm (UO `Layer.Helm`).
pub const LAYER_HELM: u8 = 0x06;
/// Gloves (UO `Layer.Gloves`).
pub const LAYER_GLOVES: u8 = 0x07;
/// Gorget (UO `Layer.Neck`).
pub const LAYER_GORGET: u8 = 0x0A;
/// Chest (UO `Layer.InnerTorso`).
pub const LAYER_CHEST: u8 = 0x0D;
/// Sleeves (UO `Layer.Arms`).
pub const LAYER_ARMS: u8 = 0x13;

/// How much of a body each armour layer covers, in hundredths — ServUO's
/// `BaseArmor.m_ArmorScalars` (`{ 0.07, 0.07, 0.14, 0.15, 0.22, 0.35 }` over
/// gorget, gloves, helm, arms, legs, chest). A shield is not in that array and
/// falls to ServUO's `1.0`: a shield's rating counts whole.
#[must_use]
pub const fn layer_coverage(layer: u8) -> u32 {
    match layer {
        LAYER_GORGET | LAYER_GLOVES => 7,
        LAYER_HELM => 14,
        LAYER_ARMS => 15,
        LAYER_LEGS => 22,
        LAYER_CHEST => 35,
        LAYER_SHIELD => 100,
        _ => 0,
    }
}

/// Which layer a blow lands on, given a roll in `0..100`.
///
/// ServUO's `AbsorbDamage` ladder: neck, hands, then head, arms, legs, and the
/// chest for everything above. The bands *are* [`layer_coverage`] — a piece is
/// hit as often as it covers — which is the one place this port tidies its
/// source. ServUO's two ladders disagree by a swap: its piece-selection tests
/// arms in the 14-wide band and the head in the 15-wide one, while its
/// `m_ArmorScalars` array gives the helm 0.14 and the arms 0.15. One of the two
/// is a slip, they differ by a single percentage point, and carrying both would
/// mean writing the same fact twice and having to keep them apart. The scalars
/// array wins here, because the second stage of the absorb reads it directly.
#[must_use]
pub const fn hit_layer(roll: u32) -> u8 {
    match roll {
        0..=6 => LAYER_GORGET,
        7..=13 => LAYER_GLOVES,
        14..=27 => LAYER_HELM,
        28..=42 => LAYER_ARMS,
        43..=64 => LAYER_LEGS,
        _ => LAYER_CHEST,
    }
}

/// One armour class's rating, keyed by its item [`Graphic`] id.
#[derive(Debug, Clone, Copy)]
pub struct ArmorData {
    /// The item graphic this row describes.
    pub graphic: u16,
    /// ServUO's `ArmorBase` — the class rating before body coverage.
    pub rating: u16,
}

/// The armour row for an item graphic, or `None` for anything not armour.
#[must_use]
pub fn armor_data(graphic: u16) -> Option<&'static ArmorData> {
    ARMOR.iter().find(|a| a.graphic == graphic)
}

/// One worn piece's rating: the pack's [`Armor`] override if the item carries
/// one (an enchanted breastplate), else the core table's row for its graphic,
/// else nothing.
#[must_use]
fn piece_rating(state: &WorldState, item: EntityId) -> u16 {
    if let Some(&Armor { rating }) = state.registry.get::<Armor>(item) {
        return rating;
    }
    state
        .registry
        .get::<Graphic>(item)
        .and_then(|graphic| armor_data(graphic.id))
        .map_or(0, |armor| armor.rating)
}

/// The item a mobile wears on `layer`, if any.
#[must_use]
pub fn worn_on_layer(state: &WorldState, mobile: EntityId, layer: u8) -> Option<EntityId> {
    let serial = state.registry.serial_of(mobile)?;
    state
        .registry
        .query::<Equipped>()
        .find(|(_, worn)| worn.mobile == serial && worn.layer == layer)
        .map(|(entity, _)| entity)
}

/// A mobile's whole armour rating — every worn piece scaled by how much of the
/// body it covers, ServUO's `PlayerMobile.ArmorRating`.
///
/// This is the number the status bar carries (pre-AoS it is the armour rating
/// itself; from AoS the client labels the same field physical resistance). A
/// mobile in nothing rates zero, which is why every existing combat test — none
/// of which dresses anybody — is unchanged by armour landing.
#[must_use]
pub fn worn_armor_rating(state: &WorldState, mobile: EntityId) -> u16 {
    let Some(serial) = state.registry.serial_of(mobile) else {
        return 0;
    };
    let worn: Vec<(EntityId, u8)> = state
        .registry
        .query::<Equipped>()
        .filter(|(_, worn)| worn.mobile == serial)
        .map(|(entity, worn)| (entity, worn.layer))
        .collect();
    let hundredths: u32 = worn
        .into_iter()
        .map(|(item, layer)| u32::from(piece_rating(state, item)) * layer_coverage(layer))
        .sum();
    u16::try_from(hundredths / 100).unwrap_or(u16::MAX)
}

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
        let absorbed = half
            + if half == 0 {
                0
            } else {
                state.rng.below(half + 1)
            };
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

/// The classic pre-AoS armour set, ported from
/// `ServUO/Scripts/Items/Equipment/Armor/*.cs`: each row's rating is the class's
/// `ArmorBase` getter, and its graphics are the constructor's `: base(0x…)` plus
/// the second id from the `Flipable` attribute — the client flips a piece's art
/// when it is turned, and the flipped graphic must rate the same or a rotated
/// breastplate would stop being armour.
///
/// Deliberately only the classic suits, helms and shields: the Samurai/Ninja,
/// gargoyle and artifact sets belong to expansions this shard does not run, and a
/// graphic with no row simply rates nothing.
#[rustfmt::skip]
static ARMOR: &[ArmorData] = &[
    // -- Leather (ArmorBase 13) ------------------------------------------------
    a(0x13CC, 13), a(0x13D3, 13), // Leather chest
    a(0x13CD, 13), a(0x13C5, 13), // Leather sleeves
    a(0x13CB, 13), a(0x13D2, 13), // Leather leggings
    a(0x13C6, 13),                // Leather gloves
    a(0x13C7, 13),                // Leather gorget
    a(0x1DB9, 13), a(0x1DBA, 13), // Leather cap
    a(0x1C06, 13), a(0x1C07, 13), // Female leather chest
    a(0x1C00, 13), a(0x1C01, 13), // Leather shorts
    a(0x1C08, 13), a(0x1C09, 13), // Leather skirt
    a(0x1C0A, 13), a(0x1C0B, 13), // Leather bustier sleeves
    // -- Studded (16) ----------------------------------------------------------
    a(0x13DB, 16), a(0x13E2, 16), // Studded chest
    a(0x13DC, 16), a(0x13D4, 16), // Studded sleeves
    a(0x13DA, 16), a(0x13E1, 16), // Studded leggings
    a(0x13D5, 16), a(0x13DD, 16), // Studded gloves
    a(0x13D6, 16),                // Studded gorget
    a(0x1C02, 16), a(0x1C03, 16), // Female studded chest
    a(0x1C0C, 16), a(0x1C0D, 16), // Studded bustier sleeves
    // -- Ringmail (22) ---------------------------------------------------------
    a(0x13EC, 22), a(0x13ED, 22), // Ringmail tunic
    a(0x13EE, 22), a(0x13EF, 22), // Ringmail sleeves
    a(0x13F0, 22), a(0x13F1, 22), // Ringmail leggings
    a(0x13EB, 22), a(0x13F2, 22), // Ringmail gloves
    // -- Chainmail (28) --------------------------------------------------------
    a(0x13BF, 28), a(0x13C4, 28), // Chain tunic
    a(0x13BE, 28), a(0x13C3, 28), // Chain leggings
    a(0x13BB, 28), a(0x13C0, 28), // Chain coif
    // -- Platemail (40) --------------------------------------------------------
    a(0x1415, 40), a(0x1416, 40), // Plate chest
    a(0x1410, 40), a(0x1417, 40), // Plate arms
    a(0x1411, 40), a(0x141A, 40), // Plate legs
    a(0x1414, 40), a(0x1418, 40), // Plate gloves
    a(0x1413, 40),                // Plate gorget
    a(0x1412, 40),                // Plate helm
    a(0x1C04, 30), a(0x1C05, 30), // Female plate chest
    // -- Bone (30) -------------------------------------------------------------
    a(0x144F, 30), a(0x1454, 30), // Bone chest
    a(0x144E, 30), a(0x1453, 30), // Bone arms
    a(0x1452, 30), a(0x1457, 30), // Bone legs
    a(0x1450, 30), a(0x1455, 30), // Bone gloves
    a(0x1451, 30), a(0x1456, 30), // Bone helm
    // -- Helms -----------------------------------------------------------------
    a(0x140C, 18),                // Bascinet
    a(0x1408, 30),                // Close helm
    a(0x140A, 30),                // Helmet
    a(0x140E, 30),                // Norse helm
    a(0x1F0B, 20),                // Orc helm
    // -- Shields ---------------------------------------------------------------
    a(0x1B73,  7),                // Buckler
    a(0x1B7A,  8),                // Wooden shield
    a(0x1B72, 10),                // Bronze shield
    a(0x1B7B, 11),                // Metal shield
    a(0x1B78, 12),                // Wooden kite shield
    a(0x1B74, 16),                // Metal kite shield
    a(0x1B76, 23),                // Heater shield
    a(0x1BC4, 30),                // Order shield
    a(0x1BC3, 32),                // Chaos shield
];

/// A row, so the table above reads as data.
const fn a(graphic: u16, rating: u16) -> ArmorData {
    ArmorData { graphic, rating }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_two_rows_share_a_graphic() {
        for (i, a) in ARMOR.iter().enumerate() {
            for b in &ARMOR[i + 1..] {
                assert_ne!(
                    a.graphic, b.graphic,
                    "duplicate graphic 0x{:04X}",
                    a.graphic
                );
            }
        }
    }

    #[test]
    fn the_hit_bands_match_their_coverage() {
        // The ladder and the scalars are the same fact told twice; a chest is hit
        // 35% of the time because it covers 35% of a body.
        let mut counts: [(u8, u32); 6] = [
            (LAYER_GORGET, 0),
            (LAYER_GLOVES, 0),
            (LAYER_ARMS, 0),
            (LAYER_HELM, 0),
            (LAYER_LEGS, 0),
            (LAYER_CHEST, 0),
        ];
        for roll in 0..100 {
            let layer = hit_layer(roll);
            let slot = counts
                .iter_mut()
                .find(|(l, _)| *l == layer)
                .expect("a known layer");
            slot.1 += 1;
        }
        for (layer, count) in counts {
            assert_eq!(count, layer_coverage(layer), "layer {layer:#04X}");
        }
    }
}
