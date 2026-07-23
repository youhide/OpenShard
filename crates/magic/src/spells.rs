//! The Magery spellbook, in the core.
//!
//! All 64 first-through-eighth-circle spells, ported from ServUO's `SpellInfo`
//! and the classic reagent lists: each spell's circle (which sets its mana, cast
//! delay and difficulty), the reagents it consumes, what it targets, and the
//! *default* effect the core applies. The effect is the core's to run, and a
//! scriptpack may override any spell by reacting to [`SpellCast`](crate::SpellCast)
//! — the same "default in core, customise in the pack" split skills has.
//!
//! Effects that need systems the engine does not have yet — poison, timed
//! buffs, persistent fields, summons with a lifetime, travel — are tagged
//! [`SpellEffect::Scripted`]: the spell still *casts* (mana, reagents, skill,
//! delay, target all resolve), but its effect is left to the pack until the
//! subsystem lands.

use openshard_state::{DamageType, FieldKind};

/// A reagent's item graphic — the eight classic Magery reagents.
const BLACK_PEARL: u16 = 0x0F7A;
const BLOOD_MOSS: u16 = 0x0F7B;
const GARLIC: u16 = 0x0F84;
const GINSENG: u16 = 0x0F85;
const MANDRAKE_ROOT: u16 = 0x0F86;
const NIGHTSHADE: u16 = 0x0F88;
const SULFUROUS_ASH: u16 = 0x0F8C;
const SPIDERS_SILK: u16 = 0x0F8D;

/// The mana a spell of each circle (1..8) costs — ServUO's mana table.
const CIRCLE_MANA: [u16; 8] = [4, 6, 9, 11, 14, 20, 40, 50];

/// What a spell asks the caster to aim at.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SpellTarget {
    /// No target: it works on the caster or the ground around them at once.
    SelfCast,
    /// A mobile — a creature or player.
    Mobile,
    /// A spot on the ground.
    Location,
}

/// The default effect the core runs when a spell lands.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SpellEffect {
    /// Typed damage to the target, `base` before the target's resistance.
    Damage(DamageType, u16),
    /// An area of typed damage centred on the target (or the caster for a
    /// self-cast), every mobile within [`AREA_RADIUS`] taking `base`.
    AreaDamage(DamageType, u16),
    /// Restore hit points to the target.
    Heal(u16),
    /// Poison the target — the level is scaled from the caster's Magery.
    Poison,
    /// Cure the target's poison.
    Cure,
    /// Cure the poison of every mobile around the aimed spot.
    AreaCure,
    /// Move the caster to the targeted spot.
    Teleport,
    /// A timed stat modifier — the Bless/Curse family. The `u8` is the effect
    /// kind ([`openshard_state::effect`]); its magnitude and duration scale from
    /// the caster's Magery when it lands.
    StatMod(u8),
    /// Bring the targeted ghost back to life — Resurrection. The core runs it off
    /// the ghost slice: lifts the `Ghost` state, restores the living body, and
    /// hands back a fraction of the target's hit points. A no-op on the living.
    Resurrect,
    /// A timed behaviour buff — the non-stat magical family
    /// ([`BehaviourBuffs`](openshard_state::BehaviourBuffs)). The `u8` is the
    /// effect kind (`NIGHT_SIGHT`..`MAGIC_REFLECT`); its magnitude and duration
    /// scale from the caster's Magery when it lands.
    BehaviourBuff(u8),
    /// A persistent field — a row of ground tiles laid at the aimed spot that pulse
    /// harm (Fire, Poison) or bar the way (Energy, Stone) until their tick comes.
    Field(FieldKind),
    /// Paralyze — freezes the target mobile in place for a Magery-scaled span; a
    /// blow lifts it. See [`Frozen`](openshard_state::Frozen).
    Paralyze,
    /// The core does not run this one yet — the pack owns it (fields, summons,
    /// travel, and the rest).
    Scripted,
}

/// How far an area spell reaches from its centre, in tiles.
pub const AREA_RADIUS: u32 = 2;

/// One spell's fixed data.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SpellInfo {
    /// Its name, for logs and messages.
    pub name: &'static str,
    /// Circle 1..8 — sets mana, cast delay and difficulty.
    pub circle: u8,
    /// The reagents it consumes, by item graphic (one of each).
    pub reagents: &'static [u16],
    /// What it aims at.
    pub target: SpellTarget,
    /// The core's default effect.
    pub effect: SpellEffect,
}

use openshard_state::effect;
use DamageType::{Cold, Energy, Fire, Physical};
use SpellEffect::{
    AreaCure, AreaDamage, BehaviourBuff, Cure, Damage, Field, Heal, Paralyze, Poison, Scripted,
    StatMod, Teleport,
};
use SpellTarget::{Location, Mobile, SelfCast};

/// One table entry, kept terse so all 64 read at a glance.
const fn spell(
    name: &'static str,
    circle: u8,
    reagents: &'static [u16],
    target: SpellTarget,
    effect: SpellEffect,
) -> SpellInfo {
    SpellInfo {
        name,
        circle,
        reagents,
        target,
        effect,
    }
}

/// The 64 Magery spells, indexed by their zero-based spellbook id (the `0xBF`
/// cast request's value, already one-based-decremented). Order is the classic
/// spellbook: eight per circle, Clumsy first.
pub static MAGERY: [SpellInfo; 64] = [
    // -- First circle --------------------------------------------------------
    spell(
        "Clumsy",
        1,
        &[BLOOD_MOSS, NIGHTSHADE],
        Mobile,
        StatMod(effect::CLUMSY),
    ),
    spell(
        "Create Food",
        1,
        &[GARLIC, GINSENG, MANDRAKE_ROOT],
        SelfCast,
        Scripted,
    ),
    spell(
        "Feeblemind",
        1,
        &[GINSENG, NIGHTSHADE],
        Mobile,
        StatMod(effect::FEEBLEMIND),
    ),
    spell(
        "Heal",
        1,
        &[GARLIC, GINSENG, SPIDERS_SILK],
        Mobile,
        Heal(15),
    ),
    spell("Magic Arrow", 1, &[SULFUROUS_ASH], Mobile, Damage(Fire, 6)),
    spell(
        "Night Sight",
        1,
        &[SULFUROUS_ASH, SPIDERS_SILK],
        Mobile,
        BehaviourBuff(effect::NIGHT_SIGHT),
    ),
    spell(
        "Reactive Armor",
        1,
        &[GARLIC, SPIDERS_SILK, SULFUROUS_ASH],
        SelfCast,
        BehaviourBuff(effect::REACTIVE_ARMOR),
    ),
    spell(
        "Weaken",
        1,
        &[GARLIC, NIGHTSHADE],
        Mobile,
        StatMod(effect::WEAKEN),
    ),
    // -- Second circle -------------------------------------------------------
    spell(
        "Agility",
        2,
        &[BLOOD_MOSS, MANDRAKE_ROOT],
        Mobile,
        StatMod(effect::AGILITY),
    ),
    spell(
        "Cunning",
        2,
        &[GINSENG, MANDRAKE_ROOT],
        Mobile,
        StatMod(effect::CUNNING),
    ),
    spell("Cure", 2, &[GARLIC, GINSENG], Mobile, Cure),
    spell(
        "Harm",
        2,
        &[NIGHTSHADE, SPIDERS_SILK],
        Mobile,
        Damage(Cold, 8),
    ),
    spell(
        "Magic Trap",
        2,
        &[SULFUROUS_ASH, SPIDERS_SILK],
        Location,
        Scripted,
    ),
    spell(
        "Magic Untrap",
        2,
        &[BLOOD_MOSS, SULFUROUS_ASH],
        Location,
        Scripted,
    ),
    spell(
        "Protection",
        2,
        &[GARLIC, GINSENG, SULFUROUS_ASH, SPIDERS_SILK],
        SelfCast,
        BehaviourBuff(effect::PROTECTION),
    ),
    spell(
        "Strength",
        2,
        &[MANDRAKE_ROOT, NIGHTSHADE],
        Mobile,
        StatMod(effect::STRENGTH),
    ),
    // -- Third circle --------------------------------------------------------
    spell(
        "Bless",
        3,
        &[GARLIC, MANDRAKE_ROOT],
        Mobile,
        StatMod(effect::BLESS),
    ),
    spell("Fireball", 3, &[BLACK_PEARL], Mobile, Damage(Fire, 12)),
    spell(
        "Magic Lock",
        3,
        &[BLOOD_MOSS, GARLIC, SULFUROUS_ASH],
        Location,
        Scripted,
    ),
    spell("Poison", 3, &[NIGHTSHADE], Mobile, Poison),
    spell(
        "Telekinesis",
        3,
        &[BLOOD_MOSS, MANDRAKE_ROOT],
        Location,
        Scripted,
    ),
    spell(
        "Teleport",
        3,
        &[BLOOD_MOSS, MANDRAKE_ROOT],
        Location,
        Teleport,
    ),
    spell(
        "Unlock",
        3,
        &[BLOOD_MOSS, SULFUROUS_ASH],
        Location,
        Scripted,
    ),
    spell(
        "Wall of Stone",
        3,
        &[BLOOD_MOSS, GARLIC],
        Location,
        Field(FieldKind::Stone),
    ),
    // -- Fourth circle -------------------------------------------------------
    spell(
        "Arch Cure",
        4,
        &[GARLIC, GINSENG, MANDRAKE_ROOT],
        Location,
        AreaCure,
    ),
    spell(
        "Arch Protection",
        4,
        &[GARLIC, GINSENG, MANDRAKE_ROOT, SULFUROUS_ASH, SPIDERS_SILK],
        Location,
        Scripted,
    ),
    spell(
        "Curse",
        4,
        &[GARLIC, NIGHTSHADE, SPIDERS_SILK],
        Mobile,
        StatMod(effect::CURSE),
    ),
    spell(
        "Fire Field",
        4,
        &[BLACK_PEARL, SULFUROUS_ASH, SPIDERS_SILK],
        Location,
        Field(FieldKind::Fire),
    ),
    spell(
        "Greater Heal",
        4,
        &[GARLIC, GINSENG, MANDRAKE_ROOT, SPIDERS_SILK],
        Mobile,
        Heal(35),
    ),
    spell(
        "Lightning",
        4,
        &[BLACK_PEARL, MANDRAKE_ROOT, SULFUROUS_ASH],
        Mobile,
        Damage(Energy, 14),
    ),
    spell(
        "Mana Drain",
        4,
        &[BLOOD_MOSS, MANDRAKE_ROOT, SULFUROUS_ASH, SPIDERS_SILK],
        Mobile,
        Scripted,
    ),
    spell(
        "Recall",
        4,
        &[BLOOD_MOSS, BLACK_PEARL, MANDRAKE_ROOT],
        SelfCast,
        Scripted,
    ),
    // -- Fifth circle --------------------------------------------------------
    spell(
        "Blade Spirits",
        5,
        &[BLACK_PEARL, MANDRAKE_ROOT, NIGHTSHADE],
        Location,
        Scripted,
    ),
    spell(
        "Dispel Field",
        5,
        &[BLACK_PEARL, GARLIC, SULFUROUS_ASH, SPIDERS_SILK],
        Location,
        Scripted,
    ),
    spell(
        "Incognito",
        5,
        &[BLOOD_MOSS, GARLIC, NIGHTSHADE],
        SelfCast,
        Scripted,
    ),
    spell(
        "Magic Reflection",
        5,
        &[GARLIC, MANDRAKE_ROOT, SPIDERS_SILK],
        SelfCast,
        BehaviourBuff(effect::MAGIC_REFLECT),
    ),
    spell(
        "Mind Blast",
        5,
        &[BLOOD_MOSS, MANDRAKE_ROOT, NIGHTSHADE, SPIDERS_SILK],
        Mobile,
        Damage(Cold, 14),
    ),
    spell(
        "Paralyze",
        5,
        &[GARLIC, MANDRAKE_ROOT, SPIDERS_SILK],
        Mobile,
        Paralyze,
    ),
    spell(
        "Poison Field",
        5,
        &[BLACK_PEARL, NIGHTSHADE, SPIDERS_SILK],
        Location,
        Field(FieldKind::Poison),
    ),
    spell(
        "Summon Creature",
        5,
        &[BLOOD_MOSS, MANDRAKE_ROOT, SPIDERS_SILK],
        Location,
        Scripted,
    ),
    // -- Sixth circle --------------------------------------------------------
    spell(
        "Dispel",
        6,
        &[GARLIC, MANDRAKE_ROOT, SPIDERS_SILK],
        Mobile,
        Scripted,
    ),
    spell(
        "Energy Bolt",
        6,
        &[BLACK_PEARL, NIGHTSHADE],
        Mobile,
        Damage(Energy, 20),
    ),
    spell(
        "Explosion",
        6,
        &[BLOOD_MOSS, MANDRAKE_ROOT],
        Mobile,
        Damage(Fire, 20),
    ),
    spell(
        "Invisibility",
        6,
        &[BLOOD_MOSS, NIGHTSHADE],
        Mobile,
        Scripted,
    ),
    spell(
        "Mark",
        6,
        &[BLOOD_MOSS, BLACK_PEARL, MANDRAKE_ROOT],
        SelfCast,
        Scripted,
    ),
    spell(
        "Mass Curse",
        6,
        &[GARLIC, MANDRAKE_ROOT, NIGHTSHADE, SPIDERS_SILK],
        Location,
        Scripted,
    ),
    spell(
        "Paralyze Field",
        6,
        &[BLOOD_MOSS, GARLIC, SULFUROUS_ASH, SPIDERS_SILK],
        Location,
        Field(FieldKind::Paralyze),
    ),
    spell(
        "Reveal",
        6,
        &[BLOOD_MOSS, SULFUROUS_ASH],
        Location,
        Scripted,
    ),
    // -- Seventh circle ------------------------------------------------------
    spell(
        "Chain Lightning",
        7,
        &[BLOOD_MOSS, BLACK_PEARL, MANDRAKE_ROOT, SULFUROUS_ASH],
        Location,
        AreaDamage(Energy, 22),
    ),
    spell(
        "Energy Field",
        7,
        &[BLOOD_MOSS, MANDRAKE_ROOT, SULFUROUS_ASH, SPIDERS_SILK],
        Location,
        Field(FieldKind::Energy),
    ),
    spell(
        "Flamestrike",
        7,
        &[SULFUROUS_ASH, SPIDERS_SILK],
        Mobile,
        Damage(Fire, 28),
    ),
    spell(
        "Gate Travel",
        7,
        &[BLOOD_MOSS, MANDRAKE_ROOT, SULFUROUS_ASH],
        SelfCast,
        Scripted,
    ),
    spell(
        "Mana Vampire",
        7,
        &[BLOOD_MOSS, BLACK_PEARL, MANDRAKE_ROOT, SPIDERS_SILK],
        Mobile,
        Scripted,
    ),
    spell(
        "Mass Dispel",
        7,
        &[BLACK_PEARL, GARLIC, MANDRAKE_ROOT, SPIDERS_SILK],
        Location,
        Scripted,
    ),
    spell(
        "Meteor Swarm",
        7,
        &[BLOOD_MOSS, MANDRAKE_ROOT, SULFUROUS_ASH, SPIDERS_SILK],
        Location,
        AreaDamage(Fire, 24),
    ),
    spell(
        "Polymorph",
        7,
        &[BLOOD_MOSS, MANDRAKE_ROOT, SPIDERS_SILK],
        SelfCast,
        Scripted,
    ),
    // -- Eighth circle -------------------------------------------------------
    spell(
        "Earthquake",
        8,
        &[BLOOD_MOSS, GARLIC, MANDRAKE_ROOT, SPIDERS_SILK],
        SelfCast,
        AreaDamage(Physical, 30),
    ),
    spell(
        "Energy Vortex",
        8,
        &[BLOOD_MOSS, MANDRAKE_ROOT, NIGHTSHADE, SPIDERS_SILK],
        Location,
        Scripted,
    ),
    spell(
        "Resurrection",
        8,
        &[BLOOD_MOSS, GARLIC, GINSENG],
        Mobile,
        SpellEffect::Resurrect,
    ),
    spell(
        "Air Elemental",
        8,
        &[BLOOD_MOSS, MANDRAKE_ROOT, SPIDERS_SILK],
        SelfCast,
        Scripted,
    ),
    spell(
        "Summon Daemon",
        8,
        &[BLOOD_MOSS, MANDRAKE_ROOT, SPIDERS_SILK],
        SelfCast,
        Scripted,
    ),
    spell(
        "Earth Elemental",
        8,
        &[BLOOD_MOSS, MANDRAKE_ROOT, SPIDERS_SILK],
        SelfCast,
        Scripted,
    ),
    spell(
        "Fire Elemental",
        8,
        &[BLOOD_MOSS, MANDRAKE_ROOT, SPIDERS_SILK, SULFUROUS_ASH],
        SelfCast,
        Scripted,
    ),
    spell(
        "Water Elemental",
        8,
        &[BLOOD_MOSS, MANDRAKE_ROOT, SPIDERS_SILK],
        SelfCast,
        Scripted,
    ),
];

/// The spell at a zero-based spellbook id, or `None` past the eighth circle.
#[must_use]
pub fn info(spell: u16) -> Option<&'static SpellInfo> {
    MAGERY.get(spell as usize)
}

/// The mana a spell costs, from its circle.
#[must_use]
pub fn mana(info: &SpellInfo) -> u16 {
    CIRCLE_MANA[(info.circle.clamp(1, 8) - 1) as usize]
}

/// The casting difficulty, 0..100 — higher circles are harder to hold. Fed to
/// the same skill roll a mined ore uses.
#[must_use]
pub fn difficulty(info: &SpellInfo) -> u16 {
    u16::from(info.circle.saturating_sub(1)) * 10
}

/// How long the cast takes, in ticks, before it resolves — the delay the
/// "servuo" cast style waits out. Scales with the circle: half a second at the
/// first, a shade over two at the eighth.
#[must_use]
pub fn cast_delay_ticks(info: &SpellInfo, ticks_per_second: u64) -> u64 {
    (u64::from(info.circle) + 1) * ticks_per_second / 4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_circle_holds_eight_spells_in_order() {
        assert_eq!(MAGERY.len(), 64);
        for (id, spell) in MAGERY.iter().enumerate() {
            assert_eq!(
                spell.circle as usize,
                id / 8 + 1,
                "{} is in the wrong circle",
                spell.name
            );
            assert!(!spell.reagents.is_empty(), "{} has no reagents", spell.name);
        }
    }

    #[test]
    fn the_classic_ids_name_the_classic_spells() {
        assert_eq!(info(4).unwrap().name, "Magic Arrow");
        assert_eq!(info(17).unwrap().name, "Fireball");
        assert_eq!(info(50).unwrap().name, "Flamestrike");
        assert_eq!(info(21).unwrap().name, "Teleport");
        // The field spells, whose ids the field tests cast by.
        assert_eq!(info(23).unwrap().name, "Wall of Stone");
        assert_eq!(info(27).unwrap().name, "Fire Field");
        assert_eq!(info(38).unwrap().name, "Poison Field");
        assert_eq!(info(49).unwrap().name, "Energy Field");
        // Paralysis, whose ids the paralyze tests cast by.
        assert_eq!(info(37).unwrap().name, "Paralyze");
        assert_eq!(info(46).unwrap().name, "Paralyze Field");
        assert!(info(64).is_none(), "there is no 65th spell");
    }

    #[test]
    fn mana_and_delay_climb_with_the_circle() {
        assert_eq!(mana(info(4).unwrap()), 4, "a first-circle spell is cheap");
        assert_eq!(mana(info(50).unwrap()), 40, "a seventh-circle one is not");
        assert!(cast_delay_ticks(info(50).unwrap(), 20) > cast_delay_ticks(info(4).unwrap(), 20));
    }
}
