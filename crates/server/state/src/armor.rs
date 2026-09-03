//! Armour — the rating an armour class lends its wearer, and where a piece sits.
//!
//! Like weapon speed and damage, an armour rating is **not** in `tiledata.mul`:
//! both references keep it per armour *class*. So it lives here, a core table keyed
//! by item graphic for legacy art and `ItemKindId` for registered armour,
//! ported from ServUO's `BaseArmor` subclasses (each row's `ArmorBase` getter,
//! its graphics from the constructor and the `Flipable`
//! attribute), exactly the shape [`crate::weapon`] uses — and in `state` for the
//! same reason: `combat` turns a rating into what a blow gets past, `skills` reads
//! the same rating to tell an Arms Lore student how well a piece protects.
//!
//! What is *not* in the table is where a piece is worn: that is the item's
//! `Equipped.layer`, which the wearer already carries, and ServUO derives its own
//! `BodyPosition` from the layer the same way. So a gorget counts as a gorget
//! because it is on the neck, not because this table says so — one fact, one
//! place.
//!
//! The rules built on it stay in `combat`: `worn_armor_rating` (the wearer's total,
//! the number the status bar shows) and `absorb_physical` (what a swing loses to it
//! pre-AoS, rolled on the world's generator). Both are read-site derivations, so
//! armour coming off needs no undoing.

/// The shield layer (UO `Layer.TwoHanded`).
pub const LAYER_SHIELD: Layer = Layer(0x02);
/// Leggings (UO `Layer.Pants`).
pub const LAYER_LEGS: Layer = Layer(0x04);
/// Helm (UO `Layer.Helm`).
pub const LAYER_HELM: Layer = Layer(0x06);
/// Gloves (UO `Layer.Gloves`).
pub const LAYER_GLOVES: Layer = Layer(0x07);
/// Gorget (UO `Layer.Neck`).
pub const LAYER_GORGET: Layer = Layer(0x0A);
/// Chest (UO `Layer.InnerTorso`).
pub const LAYER_CHEST: Layer = Layer(0x0D);
/// Sleeves (UO `Layer.Arms`).
pub const LAYER_ARMS: Layer = Layer(0x13);

/// How much of a body each armour layer covers, in hundredths — ServUO's
/// `BaseArmor.m_ArmorScalars` (`{ 0.07, 0.07, 0.14, 0.15, 0.22, 0.35 }` over
/// gorget, gloves, helm, arms, legs, chest). A shield is not in that array and
/// falls to ServUO's `1.0`: a shield's rating counts whole.
#[must_use]
pub const fn layer_coverage(layer: Layer) -> u32 {
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
pub const fn hit_layer(roll: u32) -> Layer {
    match roll {
        0..=6 => LAYER_GORGET,
        7..=13 => LAYER_GLOVES,
        14..=27 => LAYER_HELM,
        28..=42 => LAYER_ARMS,
        43..=64 => LAYER_LEGS,
        _ => LAYER_CHEST,
    }
}

/// How much of a piece's rating gets in the way of meditating in it — ServUO's
/// `ArmorMeditationAllowance`, a property of the *material* rather than of the
/// piece.
///
/// It is the whole reason Meditation is a mage's skill and not everyone's: leather
/// costs nothing, studded costs half, and every metal suit costs its full rating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MedAllowance {
    /// Meditate freely — leather.
    All,
    /// Half the rating counts against you — studded.
    Half,
    /// The whole rating does. ServUO's default for anything that does not say
    /// otherwise, which is every metal and bone suit, every helm and every shield.
    None,
}

/// One armour class's rating, keyed by its item [`Drawn`](crate::Drawn) id.
#[derive(Debug, Clone, Copy)]
pub struct ArmorData {
    /// The durable item kind for a registered armour piece. Rows without one
    /// are legacy classes still addressed through their drawing graphic.
    pub item_kind:  Option<ItemKindId>,
    /// The item graphic this row describes.
    pub graphic:    Graphic,
    /// ServUO's `ArmorBase` — the class rating before body coverage.
    pub rating:     u16,
    /// How much it hinders meditation — its material's `DefMedAllowance`.
    pub meditation: MedAllowance,
}

/// The armour row for an item graphic, or `None` for anything not armour.
#[must_use]
pub fn armor_data(graphic: Graphic) -> Option<&'static ArmorData> {
    ARMOR.iter().find(|a| a.graphic == graphic)
}

/// The armour row for a registered item kind.
///
/// A declared kind not present in this semantic column cannot acquire armour
/// properties from its presentation art.
#[must_use]
pub fn armor_data_for_kind(kind: ItemKindId) -> Option<&'static ArmorData> {
    ARMOR.iter().find(|armor| armor.item_kind == Some(kind))
}

use openshard_entities::EntityId;
use openshard_protocol::item_kind::{
    ItemKindId,
    MaterialId,
};
use openshard_protocol::wire::{
    Graphic,
    Hue,
    Layer,
};

use crate::components::{
    Armor,
    Drawn,
    ItemKind,
    Material,
    Quality,
};
use crate::{
    WorldState,
    equipped_items,
    item_definition,
    material_definition,
    material_from_legacy_hue,
};

/// What an exceptional piece is worth — ServUO's `ar += -8 + 8 * (int)m_Quality`
/// with `ItemQuality.Exceptional` being 2, so eight points over an ordinary one.
const EXCEPTIONAL_BONUS: u16 = 8;

/// The armour a material is worth over plain iron or plain leather — ServUO's
/// `ArmorRating` switch on `CraftResource`.
///
/// Keyed by [`MaterialId`]. The legacy hue adapter below exists only while old
/// item construction still reaches this reader; a valorite breastplate is worth
/// sixteen points because it carries valorite, not because its client tint
/// happens to be `0x08AB`.
#[must_use]
pub fn material_bonus(material: MaterialId) -> u16 {
    material_definition(material).map_or(0, |definition| definition.armor_bonus)
}

/// The pre-`Material` compatibility boundary for restored or manually created
/// legacy items. New readers call [`material_bonus`] with the component.
#[must_use]
fn legacy_material_bonus(hue: Hue) -> u16 {
    material_from_legacy_hue(hue).map_or(0, material_bonus)
}

/// One worn piece's rating: the pack's [`Armor`] override if the item carries
/// one (an enchanted breastplate), else the core table's row for its graphic,
/// plus what its material and its craftsmanship are worth.
///
/// A **read-site derivation**, like a weapon's swing speed: nothing is folded
/// into the wearer when a piece goes on, so nothing has to be undone when it
/// comes off. A pack override is taken whole and gets neither bonus — a scripted
/// rating is the shard saying exactly what the piece is worth.
#[must_use]
pub fn piece_rating(state: &WorldState, item: EntityId) -> u16 {
    if let Some(&Armor { rating }) = state.registry.get::<Armor>(item) {
        return rating;
    }
    let Some(drawn) = state.registry.get::<Drawn>(item) else {
        return 0;
    };
    // A migrated item is read by its durable kind. A legacy item with no kind
    // keeps the graphic adapter until it is loaded or created through the
    // registry. Crucially, an unknown *present* kind does not fall through to
    // its art and become armour by coincidence.
    let base_rating = match state.registry.get::<ItemKind>(item) {
        Some(kind) => item_definition(kind.0).and_then(|definition| definition.armor_rating),
        None => armor_data(drawn.id).map(|armor| armor.rating),
    };
    let Some(base_rating) = base_rating else {
        return 0;
    };
    let exceptional = state
        .registry
        .get::<Quality>(item)
        .is_some_and(|quality| quality.exceptional);
    let material_bonus = state.registry.get::<Material>(item).map_or_else(
        || legacy_material_bonus(drawn.hue),
        |material| material_bonus(material.0),
    );
    base_rating + material_bonus + if exceptional { EXCEPTIONAL_BONUS } else { 0 }
}

/// The item a mobile wears on `layer`, if any.
#[must_use]
pub fn worn_on_layer(state: &WorldState, mobile: EntityId, layer: Layer) -> Option<EntityId> {
    let serial = state.registry.serial_of(mobile)?;
    equipped_items(state, serial)
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
    let hundredths: u32 = equipped_items(state, serial)
        .map(|(item, worn)| u32::from(piece_rating(state, item)) * layer_coverage(worn.layer))
        .sum();
    u16::try_from(hundredths / 100).unwrap_or(u16::MAX)
}

/// The classic pre-AoS armour set, ported from
/// `ServUO/Scripts/Items/Equipment/Armor/*.cs`: each row's rating is the class's
/// `ArmorBase` getter, and its graphics are the constructor's `: base(0x…)` plus
/// the second id from the `Flipable` attribute — the client flips a piece's art
/// when it is turned, and the flipped graphic must rate the same or a rotated
/// breastplate would stop being armour.
///
/// The meditation column is the material's `DefMedAllowance`, and it is uniform per
/// material in ServUO's own data: leather `All`, studded `Half`, and everything
/// else the base class's default, `None`.
///
/// Deliberately only the classic suits, helms and shields: the Samurai/Ninja,
/// gargoyle and artifact sets belong to expansions this shard does not run, and a
/// graphic with no row simply rates nothing.
#[rustfmt::skip]
static ARMOR: &[ArmorData] = &[
    // -- Leather (ArmorBase 13) ------------------------------------------------
    with_item_kind(a(0x13CC, 13, ALL), ItemKindId(99)), a(0x13D3, 13, ALL), // Leather chest
    with_item_kind(a(0x13CD, 13, ALL), ItemKindId(97)), a(0x13C5, 13, ALL), // Leather sleeves
    with_item_kind(a(0x13CB, 13, ALL), ItemKindId(98)), a(0x13D2, 13, ALL), // Leather leggings
    with_item_kind(a(0x13C6, 13, ALL), ItemKindId(96)),                // Leather gloves
    with_item_kind(a(0x13C7, 13, ALL), ItemKindId(95)),                // Leather gorget
    with_item_kind(a(0x1DB9, 13, ALL), ItemKindId(107)), a(0x1DBA, 13, ALL), // Leather cap
    with_item_kind(a(0x1C06, 13, ALL), ItemKindId(106)), a(0x1C07, 13, ALL), // Female leather chest
    with_item_kind(a(0x1C00, 13, ALL), ItemKindId(131)), a(0x1C01, 13, ALL), // Leather shorts
    with_item_kind(a(0x1C08, 13, ALL), ItemKindId(132)), a(0x1C09, 13, ALL), // Leather skirt
    with_item_kind(a(0x1C0A, 13, ALL), ItemKindId(133)), a(0x1C0B, 13, ALL), // Leather bustier sleeves
    // -- Studded (16) ----------------------------------------------------------
    with_item_kind(a(0x13DB, 16, HALF), ItemKindId(104)), a(0x13E2, 16, HALF), // Studded chest
    with_item_kind(a(0x13DC, 16, HALF), ItemKindId(102)), a(0x13D4, 16, HALF), // Studded sleeves
    with_item_kind(a(0x13DA, 16, HALF), ItemKindId(103)), a(0x13E1, 16, HALF), // Studded leggings
    with_item_kind(a(0x13D5, 16, HALF), ItemKindId(101)), a(0x13DD, 16, HALF), // Studded gloves
    with_item_kind(a(0x13D6, 16, HALF), ItemKindId(100)),                // Studded gorget
    with_item_kind(a(0x1C02, 16, HALF), ItemKindId(109)), a(0x1C03, 16, HALF), // Female studded chest
    with_item_kind(a(0x1C0C, 16, HALF), ItemKindId(108)), a(0x1C0D, 16, HALF), // Studded bustier sleeves
    // -- Ringmail (22) ---------------------------------------------------------
    with_item_kind(a(0x13EC, 22, NONE), ItemKindId(46)), a(0x13ED, 22, NONE), // Ringmail tunic
    with_item_kind(a(0x13EE, 22, NONE), ItemKindId(45)), a(0x13EF, 22, NONE), // Ringmail sleeves
    with_item_kind(a(0x13F0, 22, NONE), ItemKindId(44)), a(0x13F1, 22, NONE), // Ringmail leggings
    with_item_kind(a(0x13EB, 22, NONE), ItemKindId(43)), a(0x13F2, 22, NONE), // Ringmail gloves
    // -- Chainmail (28) --------------------------------------------------------
    with_item_kind(a(0x13BF, 28, NONE), ItemKindId(49)), a(0x13C4, 28, NONE), // Chain tunic
    with_item_kind(a(0x13BE, 28, NONE), ItemKindId(48)), a(0x13C3, 28, NONE), // Chain leggings
    with_item_kind(a(0x13BB, 28, NONE), ItemKindId(47)), a(0x13C0, 28, NONE), // Chain coif
    // -- Platemail (40) --------------------------------------------------------
    with_item_kind(a(0x1415, 40, NONE), ItemKindId(5)), a(0x1416, 40, NONE), // Plate chest
    with_item_kind(a(0x1410, 40, NONE), ItemKindId(50)), a(0x1417, 40, NONE), // Plate arms
    with_item_kind(a(0x1411, 40, NONE), ItemKindId(53)), a(0x141A, 40, NONE), // Plate legs
    with_item_kind(a(0x1414, 40, NONE), ItemKindId(51)), a(0x1418, 40, NONE), // Plate gloves
    with_item_kind(a(0x1413, 40, NONE), ItemKindId(52)),                // Plate gorget
    with_item_kind(a(0x1412, 40, NONE), ItemKindId(54)),                // Plate helm
    with_item_kind(a(0x1C04, 30, NONE), ItemKindId(105)), a(0x1C05, 30, NONE), // Female plate chest
    // -- Bone (30) -------------------------------------------------------------
    with_item_kind(a(0x144F, 30, NONE), ItemKindId(134)), a(0x1454, 30, NONE), // Bone chest
    with_item_kind(a(0x144E, 30, NONE), ItemKindId(135)), a(0x1453, 30, NONE), // Bone arms
    with_item_kind(a(0x1452, 30, NONE), ItemKindId(136)), a(0x1457, 30, NONE), // Bone leggings
    with_item_kind(a(0x1450, 30, NONE), ItemKindId(137)), a(0x1455, 30, NONE), // Bone gloves
    with_item_kind(a(0x1451, 30, NONE), ItemKindId(138)), a(0x1456, 30, NONE), // Bone helm
    // -- Helms -----------------------------------------------------------------
    with_item_kind(a(0x140C, 18, NONE), ItemKindId(55)),                // Bascinet
    with_item_kind(a(0x1408, 30, NONE), ItemKindId(56)),                // Close helm
    with_item_kind(a(0x140A, 30, NONE), ItemKindId(57)),                // Helmet
    with_item_kind(a(0x140E, 30, NONE), ItemKindId(58)),                // Norse helm
    with_item_kind(a(0x1F0B, 20, NONE), ItemKindId(139)),               // Orc helm
    // -- Shields ---------------------------------------------------------------
    with_item_kind(a(0x1B73, 7, NONE), ItemKindId(59)),                 // Buckler
    with_item_kind(a(0x1B7A, 8, NONE), ItemKindId(140)),                // Wooden shield
    with_item_kind(a(0x1B72, 10, NONE), ItemKindId(60)),                // Bronze shield
    with_item_kind(a(0x1B7B, 11, NONE), ItemKindId(62)),                // Metal shield
    with_item_kind(a(0x1B78, 12, NONE), ItemKindId(141)),               // Wooden kite shield
    with_item_kind(a(0x1B74, 16, NONE), ItemKindId(63)),                // Metal kite shield
    with_item_kind(a(0x1B76, 23, NONE), ItemKindId(61)),                // Heater shield
    with_item_kind(a(0x1BC4, 30, NONE), ItemKindId(65)),                // Order shield
    with_item_kind(a(0x1BC3, 32, NONE), ItemKindId(64)),                // Chaos shield
];

/// A row, so the table above reads as data.
const fn a(graphic: u16, rating: u16, meditation: MedAllowance) -> ArmorData {
    ArmorData {
        item_kind: None,
        graphic: Graphic(graphic),
        rating,
        meditation,
    }
}

/// Attach a durable registry identity without making the combat lookup depend
/// on the row's client art.
const fn with_item_kind(mut armor: ArmorData, item_kind: ItemKindId) -> ArmorData {
    armor.item_kind = Some(item_kind);
    armor
}

// Short names for the allowance column, so a row still fits on one line.
const ALL: MedAllowance = MedAllowance::All;
const HALF: MedAllowance = MedAllowance::Half;
const NONE: MedAllowance = MedAllowance::None;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_graphic_resolves_and_an_unknown_one_does_not() {
        assert_eq!(armor_data(Graphic(0x1415)).expect("plate chest").rating, 40);
        assert_eq!(armor_data(Graphic(0x13CC)).expect("leather chest").rating, 13);
        assert_eq!(armor_data(Graphic(0x1B73)).expect("buckler").rating, 7);
        assert!(armor_data(Graphic(0x0000)).is_none());
    }

    #[test]
    fn a_registered_plate_chest_resolves_by_kind() {
        let plate = armor_data_for_kind(ItemKindId(5)).expect("plate chest kind");
        assert_eq!(plate.item_kind, Some(ItemKindId(5)));
        assert_eq!(plate.rating, 40);
        assert!(armor_data_for_kind(ItemKindId(4)).is_none()); // longsword
    }

    #[test]
    fn every_registered_armour_kind_has_its_direct_combat_row() {
        for definition in crate::item_definition::ITEM_DEFINITIONS {
            let Some(rating) = definition.armor_rating else {
                continue;
            };
            let armor = armor_data_for_kind(definition.id)
                .unwrap_or_else(|| panic!("{} has no armour row", definition.name));
            assert_eq!(armor.rating, rating, "{}", definition.name);
            assert_eq!(armor.item_kind, Some(definition.id));
        }
    }

    /// Every art this table protects with is one the registry can name, and the
    /// row it names back is this row. [`crate::weapon`]'s twin of this test says
    /// why, and armour has the sharper edge: a flipped breastplate whose facing
    /// no definition claims rates forty on the legacy path and nothing at all on
    /// the semantic one, because `piece_rating` asks the registry the moment an
    /// item carries a kind.
    #[test]
    fn every_armour_art_resolves_to_a_kind_that_protects_the_same() {
        for row in ARMOR {
            let (kind, _) = crate::item_definition::kind_from_drawn(Drawn {
                id:  row.graphic,
                hue: Hue::NONE,
            })
            .unwrap_or_else(|| panic!("armour {:#06X} is in no item definition", row.graphic.0));
            let semantic = armor_data_for_kind(kind)
                .unwrap_or_else(|| panic!("registered kind {} has no armour row", kind.0));
            assert_eq!(
                semantic.rating, row.rating,
                "{:#06X} rates differently as kind {}",
                row.graphic.0, kind.0
            );
            assert_eq!(
                semantic.meditation, row.meditation,
                "{:#06X} hinders meditation differently as kind {}",
                row.graphic.0, kind.0
            );
        }
    }

    #[test]
    fn material_bonus_is_a_material_definition_fact() {
        assert_eq!(material_bonus(MaterialId(1)), 0); // iron
        assert_eq!(material_bonus(MaterialId(9)), 16); // valorite
        assert_eq!(material_bonus(MaterialId(43)), 16); // barbed leather
        assert_eq!(material_bonus(MaterialId(999)), 0);
    }

    #[test]
    fn only_leather_and_studded_let_you_meditate() {
        // The one fact that makes Meditation a mage's skill: a leather suit costs
        // nothing, studded costs half its rating, and every metal piece — down to
        // the buckler on your arm — costs all of it.
        let med = |graphic: u16| armor_data(Graphic(graphic)).expect("in the table").meditation;
        assert_eq!(med(0x13CC), MedAllowance::All); // leather chest
        assert_eq!(med(0x13DB), MedAllowance::Half); // studded chest
        assert_eq!(med(0x1415), MedAllowance::None); // plate chest
        assert_eq!(med(0x13BF), MedAllowance::None); // chain tunic
        assert_eq!(med(0x1B73), MedAllowance::None); // buckler
        assert_eq!(med(0x144F), MedAllowance::None); // bone chest
    }

    #[test]
    fn no_two_rows_share_a_graphic() {
        for (i, a) in ARMOR.iter().enumerate() {
            for b in &ARMOR[i + 1..] {
                assert_ne!(a.graphic, b.graphic, "duplicate graphic 0x{:04X}", a.graphic.0);
            }
        }
    }

    #[test]
    fn no_two_rows_claim_one_registered_kind() {
        for (index, armor) in ARMOR.iter().enumerate() {
            let Some(kind) = armor.item_kind else {
                continue;
            };
            assert!(
                ARMOR[index + 1..]
                    .iter()
                    .all(|other| other.item_kind != Some(kind)),
                "duplicate armour kind {}",
                kind.0
            );
        }
    }

    #[test]
    fn the_shared_catalogue_filter_matches_the_gameplay_table() {
        for raw in u16::MIN..=u16::MAX {
            let graphic = Graphic(raw);
            assert_eq!(
                openshard_protocol::items::is_classic_armor(graphic),
                armor_data(graphic).is_some(),
                "0x{raw:04X}"
            );
        }
    }

    #[test]
    fn the_hit_bands_match_their_coverage() {
        // The ladder and the scalars are the same fact told twice; a chest is hit
        // 35% of the time because it covers 35% of a body.
        let mut counts: [(Layer, u32); 6] = [
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
            assert_eq!(count, layer_coverage(layer), "layer {:#04X}", layer.0);
        }
    }
}
