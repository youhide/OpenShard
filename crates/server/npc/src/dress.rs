//! What a townsperson looks like: the body, the skin, the hair and the clothes.
//!
//! # Why this is generated and not data
//!
//! A town whose every soul is the same male body in the same robe does not read
//! as a town, and the fix is not a bigger table in the pack — it is the *roll*
//! ServUO already makes. `BaseVendor.InitBody` and `InitOutfit` dress each vendor
//! from a handful of choices: gender, one of 57 skin hues, one of nine hair
//! styles and seven beards, a shirt/doublet/fancy-shirt, trousers or a kilt or a
//! skirt, shoes of a type its profession picks. The variety is in the dice, so it
//! belongs where the dice are.
//!
//! This is that port, constant for constant (`Utility.RandomSkinHue` and friends
//! in `Server/Utility.cs`, `RaceDefinitions.Human.RandomHair`/`RandomFacialHair`
//! in `Scripts/Misc`). The profession's own additions — a smith's apron, a mage's
//! blue robe — are the *pack's*, layered on top: the same "default in core,
//! customise in the pack" split `magic::spells` and `creature_name` use. All of
//! it spends the world's seeded [`Rng`], so a shard populates the same town twice.
//!
//! # Hair is an item, and that is a hazard
//!
//! UO has no "hair" field on a mobile: hair and a beard are items worn on layers
//! `0x0B` and `0x10`, drawn in the same `0x78` equipment list as a shirt. Which
//! means that without care a player could drag the hair off a shopkeeper's head.
//! [`FIXED_LAYERS`] names the layers nothing may be lifted from — ServUO's
//! `Movable = false` on the same items.

use openshard_protocol::wire::{Graphic, Hue, Layer};
use openshard_state::rng::Rng;

/// Layer `0x03`, UO `Layer.Shoes`.
pub const LAYER_SHOES: Layer = Layer(0x03);
/// Layer `0x04`, UO `Layer.Pants` — trousers.
pub const LAYER_PANTS: Layer = Layer(0x04);
/// Layer `0x05`, UO `Layer.Shirt`.
pub const LAYER_SHIRT: Layer = Layer(0x05);
/// Layer `0x0B`, UO `Layer.Hair`. The value lives in `protocol` rather than
/// here, unlike the rest of this family: the *client* has to ask about this one
/// too — it refuses to draw hair on the dead — and a number two crates decide
/// behaviour from is a number with one definition.
pub const LAYER_HAIR: Layer = Layer::HAIR;
/// Layer `0x10`, UO `Layer.FacialHair` — a beard. [`LAYER_HAIR`]'s twin, and
/// named in `protocol` for its reason.
pub const LAYER_FACIAL_HAIR: Layer = Layer::BEARD;
/// Layer `0x11`, UO `Layer.MiddleTorso` — a doublet, a tunic, an apron.
pub const LAYER_MIDDLE_TORSO: Layer = Layer(0x11);
/// Layer `0x17`, UO `Layer.OuterLegs` — a kilt or a skirt.
pub const LAYER_OUTER_LEGS: Layer = Layer(0x17);

/// The layers a player may never lift something off, however close they stand.
///
/// `items`' — it is the crate the lift path lives in and so the crate that has to
/// enforce it; re-exported here because this is the module that puts things on
/// those layers, and two copies of the list would drift.
pub use openshard_items::FIXED_LAYERS;

/// The male human body, UO `0x0190`.
pub const BODY_MALE: Graphic = Graphic(0x0190);
/// The female human body, UO `0x0191`.
pub const BODY_FEMALE: Graphic = Graphic(0x0191);

/// What a profession puts on its feet — ServUO's `VendorShoeType`, which each
/// vendor class overrides (`Mage` rolls shoes or sandals, a `Ranger` wears thigh
/// boots). `None` is barefoot, which no `BaseVendor` is but a beggar should be.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ShoeType {
    /// Barefoot.
    None,
    /// Plain shoes — `BaseVendor`'s default.
    #[default]
    Shoes,
    /// Boots.
    Boots,
    /// Sandals.
    Sandals,
    /// Thigh boots.
    ThighBoots,
}

impl ShoeType {
    /// From the wire byte the pack sends, matching the declaration order. An
    /// unknown value is the default, so an old pack keeps working.
    #[must_use]
    pub const fn from_bits(bits: u8) -> Self {
        match bits {
            0 => Self::None,
            2 => Self::Boots,
            3 => Self::Sandals,
            4 => Self::ThighBoots,
            _ => Self::Shoes,
        }
    }

    /// The graphic worn, or `None` when barefoot.
    const fn graphic(self) -> Option<Graphic> {
        match self {
            Self::None => None,
            Self::Shoes => Some(Graphic(0x170F)),
            Self::Boots => Some(Graphic(0x170B)),
            Self::Sandals => Some(Graphic(0x170D)),
            Self::ThighBoots => Some(Graphic(0x1711)),
        }
    }
}

/// A generated townsperson: the body it wears and everything on it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Appearance {
    /// The human body graphic, [`BODY_MALE`] or [`BODY_FEMALE`].
    pub body: Graphic,
    /// The skin hue.
    pub hue: Hue,
    /// Whether it came out female — the pack needs it to pick a name list.
    pub female: bool,
    /// Everything worn, `(graphic, layer, hue)`, in the order `spawn` equips it.
    pub equipment: Vec<(Graphic, Layer, Hue)>,
}

/// Dress a townsperson, ServUO's `BaseVendor.InitBody` then `InitOutfit`.
///
/// `female` pins the gender; `None` rolls it (`GetGender` is `Utility.RandomBool`).
/// `shoe` is the profession's [`ShoeType`]. The rolls come off `rng` in ServUO's
/// order, so the sequence is reproducible and a shard replays its town.
///
/// Only the *base* outfit: the profession's own additions go on top, which is why
/// the shirt sits on `0x05` and the doublet on `0x11` — a pack that adds a robe
/// (`0x16`) or an apron never fights this for a layer.
#[must_use]
pub fn dress_townsperson(rng: &mut Rng, shoe: ShoeType, female: Option<bool>) -> Appearance {
    // InitBody: the skin, then the gender and the body that follows from it.
    let hue = random_skin_hue(rng);
    let female = female.unwrap_or_else(|| rng.below(2) == 0);
    let body = if female { BODY_FEMALE } else { BODY_MALE };

    let mut equipment = Vec::with_capacity(5);

    // InitOutfit, in ServUO's order. The torso first: one of three shirts.
    let torso = match rng.below(3) {
        0 => (0x1EFD, LAYER_SHIRT),        // FancyShirt
        1 => (0x1F7B, LAYER_MIDDLE_TORSO), // Doublet
        _ => (0x1517, LAYER_SHIRT),        // Shirt
    };
    equipment.push((Graphic(torso.0), torso.1, random_clothing_hue(rng)));

    // Then the feet.
    if let Some(graphic) = shoe.graphic() {
        equipment.push((graphic, LAYER_SHOES, shoe_hue(rng)));
    }

    // Then hair, and a beard for a man. One hue for both — ServUO passes the same
    // `hairHue` to `AssignRandomHair` and `AssignRandomFacialHair`, which is why a
    // townsperson's beard matches their head.
    let hair_hue = random_hair_hue(rng);
    equipment.push((random_hair(rng, female), LAYER_HAIR, hair_hue));
    if !female {
        equipment.push((random_facial_hair(rng), LAYER_FACIAL_HAIR, hair_hue));
    }

    // And the legs last, which is where the genders differ: a woman may wear
    // short pants, a kilt or a skirt (weighted 1/2/3 out of six), a man trousers.
    let legs = if female {
        match rng.below(6) {
            0 => (0x152E, LAYER_PANTS),          // ShortPants
            1 | 2 => (0x1537, LAYER_OUTER_LEGS), // Kilt
            _ => (0x1516, LAYER_OUTER_LEGS),     // Skirt
        }
    } else if rng.below(2) == 0 {
        (0x1539, LAYER_PANTS) // LongPants
    } else {
        (0x152E, LAYER_PANTS) // ShortPants
    };
    equipment.push((Graphic(legs.0), legs.1, random_clothing_hue(rng)));

    Appearance {
        body,
        hue,
        female,
        equipment,
    }
}

/// `Utility.RandomSkinHue`: one of 57 flesh tones, with the partial-hue bit set —
/// which is what stops the whole body being painted one flat colour.
fn random_skin_hue(rng: &mut Rng) -> Hue {
    Hue((1002 + rng.below(57)) as u16 | 0x8000)
}

/// `Utility.RandomHairHue`: one of 48 hair colours.
fn random_hair_hue(rng: &mut Rng) -> Hue {
    Hue((1102 + rng.below(48)) as u16)
}

/// `Utility.RandomNeutralHue`: the browns and greys leather comes in.
fn random_neutral_hue(rng: &mut Rng) -> Hue {
    Hue((1801 + rng.below(108)) as u16)
}

/// `BaseVendor.GetShoeHue`: mostly a neutral leather, one time in ten black.
fn shoe_hue(rng: &mut Rng) -> Hue {
    if rng.below(10) == 0 {
        Hue(0)
    } else {
        random_neutral_hue(rng)
    }
}

/// `BaseVendor.GetRandomHue`: cloth comes in one of five bands, so a street of
/// shopkeepers is not five hundred shades of the same blue.
fn random_clothing_hue(rng: &mut Rng) -> Hue {
    match rng.below(5) {
        0 => Hue((1301 + rng.below(54)) as u16), // RandomBlueHue
        1 => Hue((1401 + rng.below(54)) as u16), // RandomGreenHue
        2 => Hue((1601 + rng.below(54)) as u16), // RandomRedHue
        3 => Hue((1701 + rng.below(54)) as u16), // RandomYellowHue
        _ => random_neutral_hue(rng),
    }
}

/// `RaceDefinitions.Human.RandomHair`: nine styles, and never baldness. The last
/// case differs by gender — buns for a woman, a receding hairline for a man —
/// because the other body cannot wear it (`ValidateHair` rejects it).
fn random_hair(rng: &mut Rng, female: bool) -> Graphic {
    Graphic(match rng.below(9) {
        0 => 0x203B,           // Short
        1 => 0x203C,           // Long
        2 => 0x203D,           // Pony Tail
        3 => 0x2044,           // Mohawk
        4 => 0x2045,           // Pageboy
        5 => 0x2047,           // Afro
        6 => 0x2049,           // Pig tails
        7 => 0x204A,           // Krisna
        _ if female => 0x2046, // Buns
        _ => 0x2048,           // Receding
    })
}

/// `RaceDefinitions.Human.RandomFacialHair`: `((rand < 4) ? 0x203E : 0x2047) + rand`
/// over seven rolls — the odd-looking arithmetic is ServUO's, and it lands on the
/// four short beards then the three long ones.
fn random_facial_hair(rng: &mut Rng) -> Graphic {
    let rand = rng.below(7) as u16;
    Graphic(if rand < 4 { 0x203E + rand } else { 0x2047 + rand })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn the_same_seed_dresses_the_same_townsperson() {
        // The whole point of spending the world's generator: a shard populated
        // twice from one seed has the same people standing in it.
        let mut a = Rng::new(0xB0DE);
        let mut b = Rng::new(0xB0DE);
        assert_eq!(
            dress_townsperson(&mut a, ShoeType::Shoes, None),
            dress_townsperson(&mut b, ShoeType::Shoes, None)
        );
    }

    #[test]
    fn everyone_gets_a_torso_legs_shoes_and_hair() {
        // The regression that catches "they are all back in the one generic
        // robe": whatever the roll, a townsperson leaves here clothed on every
        // part the base outfit covers.
        for seed in 1..200u64 {
            let mut rng = Rng::new(seed);
            let look = dress_townsperson(&mut rng, ShoeType::Shoes, None);
            let layers: HashSet<Layer> = look.equipment.iter().map(|&(_, l, _)| l).collect();
            assert!(
                layers.contains(&LAYER_SHIRT) || layers.contains(&LAYER_MIDDLE_TORSO),
                "seed {seed}: nothing on the torso"
            );
            assert!(
                layers.contains(&LAYER_PANTS) || layers.contains(&LAYER_OUTER_LEGS),
                "seed {seed}: nothing on the legs"
            );
            assert!(layers.contains(&LAYER_SHOES), "seed {seed}: barefoot");
            assert!(layers.contains(&LAYER_HAIR), "seed {seed}: bald");
        }
    }

    #[test]
    fn no_two_items_land_on_one_layer() {
        // A layer collision is silent: `equip_worn_item` refuses the second item
        // and the townsperson comes out missing a garment for no visible reason.
        for seed in 1..200u64 {
            let mut rng = Rng::new(seed);
            let look = dress_townsperson(&mut rng, ShoeType::ThighBoots, None);
            let mut seen = HashSet::new();
            for &(graphic, layer, _) in &look.equipment {
                assert!(
                    seen.insert(layer),
                    "seed {seed}: {:#06x} collides on layer {:#04x}",
                    graphic.0,
                    layer.0
                );
            }
        }
    }

    #[test]
    fn a_woman_wears_no_beard_and_a_man_may() {
        // ServUO zeroes `FacialHairItemID` on body 0x191 outright; the client
        // draws a beard on a female body as a floating smudge.
        let mut men = 0;
        for seed in 1..100u64 {
            let mut rng = Rng::new(seed);
            let she = dress_townsperson(&mut rng, ShoeType::Shoes, Some(true));
            assert_eq!(she.body, BODY_FEMALE);
            assert!(
                !she.equipment.iter().any(|&(_, l, _)| l == LAYER_FACIAL_HAIR),
                "seed {seed}: a woman grew a beard"
            );

            let mut rng = Rng::new(seed);
            let he = dress_townsperson(&mut rng, ShoeType::Shoes, Some(false));
            assert_eq!(he.body, BODY_MALE);
            if he.equipment.iter().any(|&(_, l, _)| l == LAYER_FACIAL_HAIR) {
                men += 1;
            }
        }
        assert!(men > 0, "no man ever grew a beard");
    }

    #[test]
    fn both_genders_turn_up_when_the_roll_is_free() {
        // A town of only men is the bug this whole module exists to fix, and it
        // would pass every other test here.
        let mut female = 0;
        let mut male = 0;
        for seed in 1..200u64 {
            let mut rng = Rng::new(seed);
            if dress_townsperson(&mut rng, ShoeType::Shoes, None).female {
                female += 1;
            } else {
                male += 1;
            }
        }
        assert!(female > 20 && male > 20, "{female} women, {male} men");
    }

    #[test]
    fn a_skin_hue_is_a_partial_hue() {
        // Without the 0x8000 bit the client paints the whole body — hair, clothes
        // and all — in the flesh tone.
        for seed in 1..100u64 {
            let mut rng = Rng::new(seed);
            let hue = dress_townsperson(&mut rng, ShoeType::Shoes, None).hue;
            assert_ne!(hue.0 & 0x8000, 0, "seed {seed}: skin hue {:#06x} is flat", hue.0);
            assert!((1002..=1058).contains(&(hue.0 & 0x7FFF)));
        }
    }

    #[test]
    fn barefoot_is_possible_and_shoes_are_the_default() {
        let mut rng = Rng::new(7);
        let bare = dress_townsperson(&mut rng, ShoeType::None, None);
        assert!(!bare.equipment.iter().any(|&(_, l, _)| l == LAYER_SHOES));
        assert_eq!(ShoeType::from_bits(9), ShoeType::Shoes);
        assert_eq!(ShoeType::from_bits(0), ShoeType::None);
        assert_eq!(ShoeType::from_bits(4), ShoeType::ThighBoots);
    }
}
