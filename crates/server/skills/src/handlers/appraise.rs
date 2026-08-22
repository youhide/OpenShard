//! The skills that read an *object*: Arms Lore and Item Identification.
//!
//! The sibling of [`super::lore`], which reads a living body. Same shape — a
//! `roll_skill_band` that both decides and trains, and an answer chosen by
//! arithmetic on a base cliloc — over a different subject, so the two families sit
//! in their own files.
//!
//! Both read the core gear tables in [`openshard_state::weapon`] and
//! [`openshard_state::armor`], which is why those tables live in `state`: `combat`
//! turns a weapon row into a swing, and this turns the same row into a sentence.

use openshard_entities::EntityId;
use openshard_protocol::wire::{ClilocId, Graphic, Layer};
use openshard_state::armor::armor_data;
use openshard_state::components::{Body, Drawn, Name, PoisonCharges, Price};
use openshard_state::weapon::{LAYER_TWO_HANDED, WeaponKind, by_era, weapon_data, weapon_layer};
use openshard_state::{Skill, WorldState};

use crate::check::roll_skill_band;

/// "You are not certain..." — the failure both skills share.
const NOT_CERTAIN: ClilocId = ClilocId(500_353);
/// "This is neither weapon nor armor."
const NEITHER: ClilocId = ClilocId(500_352);

/// The base of Arms Lore's damage sentences for a weapon the table calls an axe, a
/// polearm or a staff — ServUO's final `else` branch. Each block is nine clilocs
/// wide (one per damage band), and within a block the one-handed line comes first.
const ARMS_OTHER: u32 = 1_038_216;
/// "It is a [n]-handed piercing weapon" — dagger, spear, kryss.
const ARMS_PIERCING: u32 = 1_038_218;
/// The slashing block: swords and knives.
const ARMS_SLASHING: u32 = 1_038_220;
/// The bashing block: maces, hammers, the war axe.
const ARMS_BASHING: u32 = 1_038_222;
/// The ranged block. One-handed and two-handed do not differ for a bow, so this
/// block has no `hand` offset — ServUO's only branch without one.
const ARMS_RANGED: u32 = 1_038_224;
/// How far apart two damage bands are inside a block.
const ARMS_DAMAGE_STRIDE: u32 = 9;
/// "This armor offers no defense against attackers." — the base of eight lines
/// climbing to "superbly crafted to provide maximum protection".
const ARMS_ARMOR: u32 = 1_038_295;
/// "It appears to have poison smeared on it." — the same line Taste ID uses, which
/// is ServUO's: a weapon master notices a coated blade without having to lick it.
const SMEARED_WITH_POISON: ClilocId = ClilocId(1_038_284);

/// "You have no idea how much it might be worth." — Item ID's failure.
const ITEM_ID_FAILED: ClilocId = ClilocId(1_041_352);
/// "It appears to be:" — followed by what it is.
const ITEM_ID_IS: ClilocId = ClilocId(1_041_349);
/// "You guess the value of that item at:" — followed by a number.
const ITEM_ID_VALUE: ClilocId = ClilocId(1_041_351);

/// Arms Lore: what a weapon does, or how well a piece of armour protects.
///
/// ServUO's `ArmsLore.InternalTarget.OnTarget`. Its durability lines
/// (`1038285 + hp`) are deliberately absent: an item in this engine has no hit
/// points, so there is no wear to report, and inventing a number would read as a
/// worn sword to anyone who looked. Poison on the blade *is* reported, as ServUO
/// does — the same line Taste Identification gives, and the reason a weapon master
/// does not have to lick a sword to know.
pub(super) fn arms_lore(state: &mut WorldState, actor: EntityId, target: EntityId) {
    let Some(&Drawn { id: graphic, .. }) = state.registry.get::<Drawn>(target) else {
        // A mobile, not a thing you can weigh in your hands.
        state.localized_message(actor, NEITHER, "");
        return;
    };
    let skill = Skill::ArmsLore;
    let era = state.gameplay.combat_era;

    if let Some(weapon) = weapon_data(graphic) {
        // The average of the era's own damage range, banded: under three it reads
        // as no damage at all, and everything from three up is one of six bands
        // five points wide, capped at thirty.
        let min = u32::from(by_era(weapon.old_min, weapon.aos_min, era));
        let max = u32::from(by_era(weapon.old_max, weapon.aos_max, era));
        let average = (min + max) / 2;
        let band = if average < 3 {
            0
        } else {
            average.min(30).div_ceil(5) // ServUO's `Math.Ceiling(min(damage, 30) / 5.0)`
        };
        // Which hand it takes comes from the client's own tiledata, over which six
        // weapon classes insist otherwise. `item_layer` needs the facet's terrain,
        // and a shard with no client files has none — there its weapons all read
        // one-handed, the same bargain its terrain makes by allowing every step.
        let tiledata_layer = tiledata_layer(state, graphic);
        let hand = u32::from(weapon_layer(weapon, tiledata_layer) == LAYER_TWO_HANDED);
        let line = match weapon.kind {
            // A bow's line does not distinguish the hands, so the offset is not
            // added — the one branch in ServUO's ladder that leaves it out.
            WeaponKind::Ranged => ARMS_RANGED + band * ARMS_DAMAGE_STRIDE,
            WeaponKind::Piercing => ARMS_PIERCING + hand + band * ARMS_DAMAGE_STRIDE,
            WeaponKind::Slashing => ARMS_SLASHING + hand + band * ARMS_DAMAGE_STRIDE,
            WeaponKind::Bashing => ARMS_BASHING + hand + band * ARMS_DAMAGE_STRIDE,
            // Axe, polearm and staff share ServUO's catch-all block.
            _ => ARMS_OTHER + hand + band * ARMS_DAMAGE_STRIDE,
        };
        if roll_skill_band(state, actor, skill, crate::SkillBand::new(0, 1000)) {
            state.localized_message(actor, ClilocId(line), "");
            // And whether somebody has been at it with a bottle.
            if state.registry.has::<PoisonCharges>(target) {
                state.localized_message(actor, SMEARED_WITH_POISON, "");
            }
        } else {
            state.localized_message(actor, NOT_CERTAIN, "");
        }
        return;
    }

    if let Some(armor) = armor_data(graphic) {
        // Eight lines over a rating capped at thirty-five, in bands of five.
        let band = u32::from(armor.rating).min(35).div_ceil(5);
        if roll_skill_band(state, actor, skill, crate::SkillBand::new(0, 1000)) {
            state.localized_message(actor, ClilocId(ARMS_ARMOR + band), "");
        } else {
            state.localized_message(actor, NOT_CERTAIN, "");
        }
        return;
    }

    state.localized_message(actor, NEITHER, "");
}

/// Item Identification: what a thing is, and what it is worth.
///
/// ServUO's `ItemIdentification.InternalTarget.OnTarget`. The name is drawn over
/// the object as well as told to the asker, which is what makes the skill read as
/// pointing at something rather than as a line in the journal.
///
/// The value line is sent only for an item that actually has a price — vendor
/// stock the pack priced. ServUO computes one from its own shop tables for every
/// item in the game; here the core knows what a shopkeeper charges and nothing
/// more, and guessing a number for a rock would be worse than saying nothing.
pub(super) fn item_id(state: &mut WorldState, actor: EntityId, target: EntityId) {
    let skill = Skill::ItemId;
    if !roll_skill_band(state, actor, skill, crate::SkillBand::new(0, 1000)) {
        state.private_overhead_cliloc(actor, actor, ITEM_ID_FAILED, "");
        return;
    }
    // A mobile answers with its name and nothing else — there is no appraising a
    // person.
    if state.registry.has::<Body>(target) {
        let name = state.registry.get::<Name>(target).map(|n| n.0.clone());
        state.private_overhead_cliloc(actor, actor, ITEM_ID_IS, "");
        if let Some(name) = name {
            state.private_overhead_text(actor, target, &name);
        }
        return;
    }
    let Some(name) = item_name(state, target) else {
        state.private_overhead_cliloc(actor, actor, ITEM_ID_FAILED, "");
        return;
    };
    state.private_overhead_cliloc(actor, actor, ITEM_ID_IS, "");
    state.private_overhead_text(actor, target, &name);
    if let Some(&Price(price)) = state.registry.get::<Price>(target) {
        state.private_overhead_cliloc(actor, actor, ITEM_ID_VALUE, "");
        state.private_overhead_text(actor, actor, &price.to_string());
    }
}

/// What an item is called: its own label if it carries one (vendor stock, a named
/// corpse), else the tiledata name the client would draw on a single click.
pub(super) fn item_name(state: &WorldState, item: EntityId) -> Option<String> {
    if let Some(name) = state.registry.get::<Name>(item) {
        return Some(name.0.clone());
    }
    let graphic = state.registry.get::<Drawn>(item)?.id;
    // The shard's table, not the facet the looker stands on: what a graphic is
    // called does not change when it is carried across a moongate.
    state
        .tiles
        .as_deref()
        .and_then(|tiles| tiles.item_name(graphic.0))
        .map(str::to_owned)
}

/// The paperdoll layer `graphic` carries in the client's own tiledata, or `0` on a
/// shard with no client files.
///
/// The wrap from tiledata's *quality* byte to a [`Layer`] happens here:
/// `openshard-uofiles` reads the client's files and is below `protocol`, so it
/// hands out the byte and this is where it becomes a layer — see
/// `docs/protocol_newtypes.md` N4.
fn tiledata_layer(state: &WorldState, graphic: Graphic) -> Layer {
    state
        .tiles
        .as_deref()
        .map_or(Layer(0), |tiles| Layer(tiles.static_tile(graphic.0).layer))
}
