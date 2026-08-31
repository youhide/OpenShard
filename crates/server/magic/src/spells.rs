//! The Magery spellbook, in the core.
//!
//! All 64 first-through-eighth-circle spells, ported from ServUO's `SpellInfo`
//! and the classic reagent lists: each spell's circle (which sets its mana, cast
//! delay and difficulty), the reagents it consumes, what it targets, and the
//! effect it applies. [`SpellCast`](crate::SpellCast) is still emitted for
//! anything that wants to watch a cast.
//!
//! Effects that need systems the engine does not have yet — poison, timed buffs,
//! persistent fields, summons with a lifetime, travel — are tagged
//! [`SpellEffect::Unimplemented`]: the spell still *casts* (mana, reagents,
//! skill, delay and target all resolve) and then nothing happens, until the
//! subsystem lands.

use openshard_protocol::casting::SpellId;
use openshard_state::{
    DamageType,
    FieldKind,
    Skill,
};

/// A reagent's item graphic — the eight classic Magery reagents.
const BLACK_PEARL: Graphic = Graphic(0x0F7A);
const BLOOD_MOSS: Graphic = Graphic(0x0F7B);
const GARLIC: Graphic = Graphic(0x0F84);
const GINSENG: Graphic = Graphic(0x0F85);
const MANDRAKE_ROOT: Graphic = Graphic(0x0F86);
const NIGHTSHADE: Graphic = Graphic(0x0F88);
const SULFUROUS_ASH: Graphic = Graphic(0x0F8C);
const SPIDERS_SILK: Graphic = Graphic(0x0F8D);

/// The mana a spell of each circle (1..8) costs — ServUO's mana table.
const CIRCLE_MANA: [u16; 8] = [4, 6, 9, 11, 14, 20, 40, 50];

/// A Magery spell circle, which is always in the inclusive range 1 through 8.
///
/// Its wire representation is still the familiar `u8`; this type only makes
/// an invalid circle impossible to attach to a [`SpellInfo`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[repr(transparent)]
pub struct SpellCircle(u8);

impl SpellCircle {
    /// The first valid Magery circle.
    pub const MIN: u8 = 1;
    /// The last valid Magery circle.
    pub const MAX: u8 = 8;

    /// Makes a circle when `value` belongs to the Magery spellbook.
    #[must_use]
    pub const fn new(value: u8) -> Option<Self> {
        if value >= Self::MIN && value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    /// Returns the circle's stable numeric value.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// What a spell asks the caster to aim at.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SpellTarget {
    /// No target: it works on the caster or the ground around them at once.
    SelfCast,
    /// A mobile — a creature or player.
    Mobile,
    /// A spot on the ground.
    Location,
    /// An object you can hold — the travel family, which aims at a recall rune
    /// or a runebook rather than at a place.
    ///
    /// The distinction is not cosmetic: this raises the *object* cursor
    /// (`0x6C` type 0), so the client itself refuses bare ground, and the
    /// server re-checks that what came back is in reach before believing it.
    Item,
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
    /// A timed stat modifier — the Bless/Curse family. Its kind is constrained
    /// to the valid stat-effect tags; magnitude and duration scale from the
    /// caster's Magery when it lands.
    StatMod(StatEffectKind),
    /// Bring the targeted ghost back to life — Resurrection. The core runs it off
    /// the ghost slice: lifts the `Ghost` state, restores the living body, and
    /// hands back a fraction of the target's hit points. A no-op on the living.
    Resurrect,
    /// A timed behaviour buff — the non-stat magical family
    /// ([`BehaviourBuffs`](openshard_state::BehaviourBuffs)). Its kind, magnitude and duration
    /// scale from the caster's Magery when it lands.
    BehaviourBuff(openshard_state::BehaviourBuffKind),
    /// A persistent field — a row of ground tiles laid at the aimed spot that pulse
    /// harm (Fire, Poison) or bar the way (Energy, Stone) until their tick comes.
    Field(FieldKind),
    /// Paralyze — freezes the target mobile in place for a Magery-scaled span; a
    /// blow lifts it. See [`Frozen`](openshard_state::Frozen).
    Paralyze,
    /// Write the caster's own position onto the aimed recall rune.
    ///
    /// The rune must be in the caster's *backpack*, not merely within reach:
    /// ServUO says so with cliloc 1062422, and a rune lying on the floor of a
    /// shop is somebody else's.
    Mark,
    /// Take the caster to where the aimed rune (or runebook) points.
    Recall,
    /// Open a pair of gates: one where the caster stands and one at the rune's
    /// destination, each leading to the other, both closing together.
    GateTravel,
    /// The engine does not run this one yet.
    ///
    /// It still *casts* — mana, reagents, the skill roll, the words and the
    /// gesture all happen — and then nothing occurs. That was the seam a script
    /// pack filled; with the pack gone it is simply a spell that is not built,
    /// and the name says so rather than pointing at a layer that no longer
    /// exists. Summons, fields and the rest of the unbuilt list are here.
    Unimplemented,
}

/// How far an area spell reaches from its centre, in tiles.
pub const AREA_RADIUS: u32 = 2;

/// One spell's fixed data.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SpellInfo {
    /// Its name, for logs and messages.
    pub name:     &'static str,
    /// Circle 1..=8 — sets mana, cast delay and difficulty.
    pub circle:   SpellCircle,
    /// The reagents it consumes, by item graphic (one of each).
    pub reagents: &'static [Graphic],
    /// What it aims at.
    pub target:   SpellTarget,
    /// The core's default effect.
    pub effect:   SpellEffect,
}

use DamageType::{
    Cold,
    Energy,
    Fire,
    Physical,
};
use SpellEffect::{
    AreaCure,
    AreaDamage,
    BehaviourBuff,
    Cure,
    Damage,
    Field,
    Heal,
    Paralyze,
    Poison,
    StatMod,
    Teleport,
    Unimplemented,
};
use SpellTarget::{
    Item,
    Location,
    Mobile,
    SelfCast,
};
use openshard_protocol::wire::Graphic;
use openshard_state::components::StatEffectKind;

/// One table entry, kept terse so all 64 read at a glance.
const fn spell(
    name: &'static str,
    circle: u8,
    reagents: &'static [Graphic],
    target: SpellTarget,
    effect: SpellEffect,
) -> SpellInfo {
    SpellInfo {
        name,
        circle: match SpellCircle::new(circle) {
            Some(circle) => circle,
            None => panic!("spell circle must be in 1..=8"),
        },
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
        StatMod(StatEffectKind::CLUMSY),
    ),
    spell(
        "Create Food",
        1,
        &[GARLIC, GINSENG, MANDRAKE_ROOT],
        SelfCast,
        Unimplemented,
    ),
    spell(
        "Feeblemind",
        1,
        &[GINSENG, NIGHTSHADE],
        Mobile,
        StatMod(StatEffectKind::FEEBLEMIND),
    ),
    spell("Heal", 1, &[GARLIC, GINSENG, SPIDERS_SILK], Mobile, Heal(15)),
    spell("Magic Arrow", 1, &[SULFUROUS_ASH], Mobile, Damage(Fire, 6)),
    spell(
        "Night Sight",
        1,
        &[SULFUROUS_ASH, SPIDERS_SILK],
        Mobile,
        BehaviourBuff(openshard_state::BehaviourBuffKind::NIGHT_SIGHT),
    ),
    spell(
        "Reactive Armor",
        1,
        &[GARLIC, SPIDERS_SILK, SULFUROUS_ASH],
        SelfCast,
        BehaviourBuff(openshard_state::BehaviourBuffKind::REACTIVE_ARMOR),
    ),
    spell(
        "Weaken",
        1,
        &[GARLIC, NIGHTSHADE],
        Mobile,
        StatMod(StatEffectKind::WEAKEN),
    ),
    // -- Second circle -------------------------------------------------------
    spell(
        "Agility",
        2,
        &[BLOOD_MOSS, MANDRAKE_ROOT],
        Mobile,
        StatMod(StatEffectKind::AGILITY),
    ),
    spell(
        "Cunning",
        2,
        &[GINSENG, MANDRAKE_ROOT],
        Mobile,
        StatMod(StatEffectKind::CUNNING),
    ),
    spell("Cure", 2, &[GARLIC, GINSENG], Mobile, Cure),
    spell("Harm", 2, &[NIGHTSHADE, SPIDERS_SILK], Mobile, Damage(Cold, 8)),
    spell(
        "Magic Trap",
        2,
        &[SULFUROUS_ASH, SPIDERS_SILK],
        Location,
        Unimplemented,
    ),
    spell(
        "Magic Untrap",
        2,
        &[BLOOD_MOSS, SULFUROUS_ASH],
        Location,
        Unimplemented,
    ),
    spell(
        "Protection",
        2,
        &[GARLIC, GINSENG, SULFUROUS_ASH, SPIDERS_SILK],
        SelfCast,
        BehaviourBuff(openshard_state::BehaviourBuffKind::PROTECTION),
    ),
    spell(
        "Strength",
        2,
        &[MANDRAKE_ROOT, NIGHTSHADE],
        Mobile,
        StatMod(StatEffectKind::STRENGTH),
    ),
    // -- Third circle --------------------------------------------------------
    spell(
        "Bless",
        3,
        &[GARLIC, MANDRAKE_ROOT],
        Mobile,
        StatMod(StatEffectKind::BLESS),
    ),
    spell("Fireball", 3, &[BLACK_PEARL], Mobile, Damage(Fire, 12)),
    spell(
        "Magic Lock",
        3,
        &[BLOOD_MOSS, GARLIC, SULFUROUS_ASH],
        Location,
        Unimplemented,
    ),
    spell("Poison", 3, &[NIGHTSHADE], Mobile, Poison),
    spell(
        "Telekinesis",
        3,
        &[BLOOD_MOSS, MANDRAKE_ROOT],
        Location,
        Unimplemented,
    ),
    spell("Teleport", 3, &[BLOOD_MOSS, MANDRAKE_ROOT], Location, Teleport),
    spell("Unlock", 3, &[BLOOD_MOSS, SULFUROUS_ASH], Location, Unimplemented),
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
        Unimplemented,
    ),
    spell(
        "Curse",
        4,
        &[GARLIC, NIGHTSHADE, SPIDERS_SILK],
        Mobile,
        StatMod(StatEffectKind::CURSE),
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
        Unimplemented,
    ),
    spell(
        "Recall",
        4,
        &[BLOOD_MOSS, BLACK_PEARL, MANDRAKE_ROOT],
        Item,
        SpellEffect::Recall,
    ),
    // -- Fifth circle --------------------------------------------------------
    spell(
        "Blade Spirits",
        5,
        &[BLACK_PEARL, MANDRAKE_ROOT, NIGHTSHADE],
        Location,
        Unimplemented,
    ),
    spell(
        "Dispel Field",
        5,
        &[BLACK_PEARL, GARLIC, SULFUROUS_ASH, SPIDERS_SILK],
        Location,
        Unimplemented,
    ),
    spell(
        "Incognito",
        5,
        &[BLOOD_MOSS, GARLIC, NIGHTSHADE],
        SelfCast,
        Unimplemented,
    ),
    spell(
        "Magic Reflection",
        5,
        &[GARLIC, MANDRAKE_ROOT, SPIDERS_SILK],
        SelfCast,
        BehaviourBuff(openshard_state::BehaviourBuffKind::MAGIC_REFLECT),
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
        Unimplemented,
    ),
    // -- Sixth circle --------------------------------------------------------
    spell(
        "Dispel",
        6,
        &[GARLIC, MANDRAKE_ROOT, SPIDERS_SILK],
        Mobile,
        Unimplemented,
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
        Unimplemented,
    ),
    spell(
        "Mark",
        6,
        &[BLOOD_MOSS, BLACK_PEARL, MANDRAKE_ROOT],
        Item,
        SpellEffect::Mark,
    ),
    spell(
        "Mass Curse",
        6,
        &[GARLIC, MANDRAKE_ROOT, NIGHTSHADE, SPIDERS_SILK],
        Location,
        Unimplemented,
    ),
    spell(
        "Paralyze Field",
        6,
        &[BLOOD_MOSS, GARLIC, SULFUROUS_ASH, SPIDERS_SILK],
        Location,
        Field(FieldKind::Paralyze),
    ),
    spell("Reveal", 6, &[BLOOD_MOSS, SULFUROUS_ASH], Location, Unimplemented),
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
        // Black pearl, not blood moss: ServUO's `GateTravel.cs` and the classic
        // reagent list agree, and the row had blood moss — which made the one
        // spell in the family that opens a gate cost the wrong reagent.
        &[BLACK_PEARL, MANDRAKE_ROOT, SULFUROUS_ASH],
        Item,
        SpellEffect::GateTravel,
    ),
    spell(
        "Mana Vampire",
        7,
        &[BLOOD_MOSS, BLACK_PEARL, MANDRAKE_ROOT, SPIDERS_SILK],
        Mobile,
        Unimplemented,
    ),
    spell(
        "Mass Dispel",
        7,
        &[BLACK_PEARL, GARLIC, MANDRAKE_ROOT, SPIDERS_SILK],
        Location,
        Unimplemented,
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
        Unimplemented,
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
        Unimplemented,
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
        Unimplemented,
    ),
    spell(
        "Summon Daemon",
        8,
        &[BLOOD_MOSS, MANDRAKE_ROOT, SPIDERS_SILK],
        SelfCast,
        Unimplemented,
    ),
    spell(
        "Earth Elemental",
        8,
        &[BLOOD_MOSS, MANDRAKE_ROOT, SPIDERS_SILK],
        SelfCast,
        Unimplemented,
    ),
    spell(
        "Fire Elemental",
        8,
        &[BLOOD_MOSS, MANDRAKE_ROOT, SPIDERS_SILK, SULFUROUS_ASH],
        SelfCast,
        Unimplemented,
    ),
    spell(
        "Water Elemental",
        8,
        &[BLOOD_MOSS, MANDRAKE_ROOT, SPIDERS_SILK],
        SelfCast,
        Unimplemented,
    ),
];

/// The spell at a zero-based spellbook id, or `None` past the eighth circle.
#[must_use]
pub fn info(spell: SpellId) -> Option<&'static SpellInfo> {
    MAGERY.get(usize::from(spell.0))
}

/// The skill every Magery cast rolls and trains.
///
/// One name for it, here with the spell table, because it was a private `const
/// MAGERY_SKILL: u8 = 25` in *two* modules of `world` and a bare `get(25)` in a
/// third — three copies of a number that belongs to whichever crate owns casting.
pub const MAGERY_SKILL: Skill = Skill::Magery;

/// The mana a spell costs, from its circle.
#[must_use]
pub fn mana(info: &SpellInfo) -> u16 {
    CIRCLE_MANA[usize::from(info.circle.get() - SpellCircle::MIN)]
}

/// The Magery band a spell is cast against, `(min, max)` in tenths — higher
/// circles are harder to hold. Fed to the same band roll a mined ore uses.
#[must_use]
pub fn cast_skills(info: &SpellInfo) -> (i32, i32) {
    // ServUO's `MagerySpell.GetCastSkills`: the band's centre climbs 100/7 skill
    // points a circle, so the first circle sits at 0.0 and the eighth at 100.0,
    // and the band is twenty points either side of it. In tenths.
    let centre = i32::from(info.circle.get() - SpellCircle::MIN) * 1000 / 7;
    (centre - CAST_CHANCE_OFFSET, centre + CAST_CHANCE_OFFSET)
}

/// How far either side of a circle's centre the casting band reaches, in tenths —
/// ServUO's `ChanceOffset`, 20.0 skill points. A first-circle spell therefore has
/// a band starting *below zero*, which is the point: everyone casts it, and a
/// beginner still learns from doing so.
const CAST_CHANCE_OFFSET: i32 = 200;

/// How long the cast takes, in ticks, before it resolves — the delay the
/// "servuo" cast style waits out. Scales with the circle: half a second at the
/// first, a shade over two at the eighth.
#[must_use]
pub fn cast_delay_ticks(info: &SpellInfo, ticks_per_second: u64) -> u64 {
    (u64::from(info.circle.get()) + 1) * ticks_per_second / 4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_circle_holds_eight_spells_in_order() {
        assert_eq!(MAGERY.len(), 64);
        for (id, spell) in MAGERY.iter().enumerate() {
            assert_eq!(
                usize::from(spell.circle.get()),
                id / 8 + 1,
                "{} is in the wrong circle",
                spell.name
            );
            assert!(!spell.reagents.is_empty(), "{} has no reagents", spell.name);
        }
    }

    #[test]
    fn spell_circle_accepts_only_the_eight_spellbook_circles() {
        assert_eq!(SpellCircle::new(SpellCircle::MIN).unwrap().get(), 1);
        assert_eq!(SpellCircle::new(SpellCircle::MAX).unwrap().get(), 8);
        assert!(SpellCircle::new(0).is_none());
        assert!(SpellCircle::new(9).is_none());
    }

    #[test]
    fn the_classic_ids_name_the_classic_spells() {
        assert_eq!(info(SpellId(4)).unwrap().name, "Magic Arrow");
        assert_eq!(info(SpellId(17)).unwrap().name, "Fireball");
        assert_eq!(info(SpellId(50)).unwrap().name, "Flamestrike");
        assert_eq!(info(SpellId(21)).unwrap().name, "Teleport");
        // The field spells, whose ids the field tests cast by.
        assert_eq!(info(SpellId(23)).unwrap().name, "Wall of Stone");
        assert_eq!(info(SpellId(27)).unwrap().name, "Fire Field");
        assert_eq!(info(SpellId(38)).unwrap().name, "Poison Field");
        assert_eq!(info(SpellId(49)).unwrap().name, "Energy Field");
        // Paralysis, whose ids the paralyze tests cast by.
        assert_eq!(info(SpellId(37)).unwrap().name, "Paralyze");
        assert_eq!(info(SpellId(46)).unwrap().name, "Paralyze Field");
        assert!(info(SpellId(64)).is_none(), "there is no 65th spell");
    }

    /// The travel family's rows, pinned against ServUO's own `SpellInfo`.
    ///
    /// Worth a test of its own because a wrong reagent is invisible from every
    /// other direction: the spell still casts, still costs, still works — it
    /// just charges for something the player never needed to buy, and the only
    /// symptom is a mage who cannot open a gate with a pack the reference says
    /// is enough. Gate Travel's row *was* wrong (blood moss for black pearl) and
    /// nothing in the suite pointed at the table when it was.
    #[test]
    fn the_travel_spells_cost_what_the_reference_says() {
        let recall = info(SpellId(31)).unwrap();
        assert_eq!(recall.name, "Recall");
        assert_eq!(recall.reagents, &[BLOOD_MOSS, BLACK_PEARL, MANDRAKE_ROOT]);

        let mark = info(SpellId(44)).unwrap();
        assert_eq!(mark.name, "Mark");
        assert_eq!(mark.reagents, &[BLOOD_MOSS, BLACK_PEARL, MANDRAKE_ROOT]);

        let gate = info(SpellId(51)).unwrap();
        assert_eq!(gate.name, "Gate Travel");
        assert_eq!(
            gate.reagents,
            &[BLACK_PEARL, MANDRAKE_ROOT, SULFUROUS_ASH],
            "black pearl, not blood moss — ServUO's GateTravel.cs"
        );

        // And all three aim at an object, which is what raises the cursor the
        // client refuses to answer with bare ground.
        for spell in [recall, mark, gate] {
            assert_eq!(spell.target, SpellTarget::Item, "{} aims at an item", spell.name);
        }
    }

    #[test]
    fn mana_and_delay_climb_with_the_circle() {
        assert_eq!(
            mana(info(SpellId(4)).unwrap()),
            4,
            "a first-circle spell is cheap"
        );
        assert_eq!(
            mana(info(SpellId(50)).unwrap()),
            40,
            "a seventh-circle one is not"
        );
        assert!(
            cast_delay_ticks(info(SpellId(50)).unwrap(), 20)
                > cast_delay_ticks(info(SpellId(4)).unwrap(), 20)
        );
    }
}
