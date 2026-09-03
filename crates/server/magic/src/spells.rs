//! The Magery spellbook, in the core.
//!
//! All 64 first-through-eighth-circle spells, ported from ServUO's `SpellInfo`
//! and the classic reagent lists: each spell's circle (which sets its mana, cast
//! delay and difficulty), the reagents it consumes, what it targets, the effect
//! it applies, and — since the art stopped being guessed from the effect — its
//! power words, its casting gesture and the exact sound and picture it lands
//! with. [`SpellCast`](crate::SpellCast) is still emitted for anything that wants
//! to watch a cast.
//!
//! Effects that need systems the engine does not have yet — a body swap,
//! the lock-and-trap family — are tagged
//! [`SpellEffect::Unimplemented`]: the spell still *casts* (its words, its
//! gesture, mana, reagents, skill, delay and target all resolve) and then
//! nothing happens, until the subsystem lands. Such a row is
//! [`SpellArt::Silent`] too, and grows its art when it grows its effect.

use openshard_protocol::casting::SpellId;
use openshard_state::{
    DamageType,
    FieldKind,
    Skill,
    SummonKind,
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
    /// A creature called up for a while — the summoning family. It stands as the
    /// caster's follower, counts against the follower cap, and goes on its own
    /// timer. What each kind *is* — its body, its blow, its cost in slots and how
    /// long it holds — is [`openshard_state::summon`].
    Summon(SummonKind),
    /// Paralyze — freezes the target mobile in place for a Magery-scaled span; a
    /// blow lifts it. See [`Frozen`](openshard_state::Frozen).
    Paralyze,
    /// Unmake the aimed creature, if it was ever really there — Dispel.
    ///
    /// The only question it asks is whether the target carries
    /// [`Summoned`](openshard_state::components::Summoned); the roll that follows is
    /// [`crate::dispel_chance`], off the creature's own row. Nothing that was not
    /// summoned can be dispelled, however magical it looks.
    Dispel,
    /// Unmake every summon standing near the aimed spot, each rolling its own
    /// chance — Mass Dispel, within [`crate::MASS_DISPEL_RANGE`].
    MassDispel,
    /// Take away the aimed magical field — Dispel Field, which also closes a gate a
    /// Gate Travel laid.
    ///
    /// It aims at the tile *entity*, not at the ground under it, which is why its
    /// row targets an object: a field is a row of drawn items, and the one the
    /// player clicked is the one that goes.
    DispelField,
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
    /// exists. Polymorph, the lock-and-trap family and the rest of the unbuilt
    /// list are here.
    Unimplemented,
}

/// How far an area spell reaches from its centre, in tiles.
pub const AREA_RADIUS: u32 = 2;

/// The gesture a caster throws while the spell is held.
///
/// ServUO carries a per-spell `SpellInfo.Action` in the 203..=269 range, which
/// reads like twenty different animations. It is not: the client's own
/// `Anim2.def` replaces every id from 203 to 245 with group `{16}` and every one
/// from 260 to 269 with `{17}`, so the whole range is a two-valued choice. That
/// choice is what this carries, and
/// [`WorldState::animate`](openshard_state::WorldState::animate) turns it into
/// whatever the caster's body can actually do.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum CastGesture {
    /// One arm thrown at the mark — the client's `CastDirected`. All but nine
    /// of the sixty-four.
    Directed,
    /// Both arms raised to reach for something — the client's `CastArea`: the
    /// seven summons, Gate Travel and Mass Dispel.
    Area,
}

/// What a spell draws where it lands, beside the sound.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SpellVisual {
    /// A projectile thrown from the caster to the mark, bursting on arrival.
    Bolt(Graphic),
    /// A fixed animation on the mark itself, so it rides a mobile that moves.
    OnTarget(Graphic),
    /// A fixed animation planted at the aimed spot, which no mobile carries.
    AtSpot(Graphic),
    /// The client's own lightning strike over the mark. It has no art id of its
    /// own: the graphic lives in the client, and the wire says only
    /// [`EffectKind::Lightning`](openshard_protocol::feedback::EffectKind).
    Lightning,
    /// Nothing to see. What the spell *makes* is its own visual — a field's row
    /// of tiles, a summoned creature standing there — so drawing anything else
    /// would be a second, contradictory picture of the same event. The sound
    /// still plays: the reference gives these spells one and no particle.
    Unseen,
}

/// A spell's exact sound and picture, ServUO's per-spell art.
///
/// # One art per cast
///
/// ServUO gives an *area* spell two: a sound at the aimed spot and a particle on
/// every mobile it catches. This carries the landing art — the spot sound where
/// the reference has one, and the per-victim visual collapsed onto the spot, so
/// an area spell is still seen. Art played once per victim is deferred, and the
/// roadmap records it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SpellArt {
    /// A sound where it lands, and what is drawn with it.
    Landing {
        /// Played at the mark, or at the aimed spot when there is no mark.
        sound:  SoundId,
        /// Drawn with it.
        visual: SpellVisual,
    },
    /// None of its own.
    ///
    /// Four different reasons, and the row's effect says which: a travel spell
    /// voices itself at *both* ends of the journey and so cannot be voiced once
    /// here (Recall, Gate Travel, and Mark, whose sound belongs beside the rune
    /// it writes); a dispel has one picture for the thing that goes and another
    /// for the thing that holds, so it is voiced by its outcome and not by its
    /// landing; the reference gives the spell no art at all (Earthquake); or
    /// the engine does not run the spell yet, and a spell with no effect has no
    /// landing to voice. A row that grows an effect grows its art with it.
    Silent,
}

/// One spell's fixed data.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SpellInfo {
    /// Its name, for logs and messages.
    pub name:     &'static str,
    /// Its power words, said over the caster's head as the cast begins —
    /// ServUO's `SpellInfo.Mantra`, the Britannian the spell is spoken in.
    pub mantra:   &'static str,
    /// Circle 1..=8 — sets mana, cast delay and difficulty.
    pub circle:   SpellCircle,
    /// The reagents it consumes, by item graphic (one of each).
    pub reagents: &'static [Graphic],
    /// What it aims at.
    pub target:   SpellTarget,
    /// The gesture the caster throws while holding it.
    pub gesture:  CastGesture,
    /// The core's default effect.
    pub effect:   SpellEffect,
    /// The sound and picture it lands with.
    pub art:      SpellArt,
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
    Dispel,
    DispelField,
    Field,
    Heal,
    MassDispel,
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
use SpellVisual::{
    AtSpot,
    Bolt,
    Lightning,
    OnTarget,
    Unseen,
};
use openshard_protocol::wire::{
    Graphic,
    SoundId,
};
use openshard_state::components::StatEffectKind;

use crate::spells::CastGesture::{
    Area,
    Directed,
};

/// One table entry, kept terse so all 64 read at a glance.
#[allow(
    clippy::too_many_arguments,
    reason = "eight columns of one table row; a struct literal per spell is what this exists to avoid"
)]
const fn spell(
    name: &'static str,
    mantra: &'static str,
    circle: u8,
    reagents: &'static [Graphic],
    target: SpellTarget,
    gesture: CastGesture,
    effect: SpellEffect,
    art: SpellArt,
) -> SpellInfo {
    SpellInfo {
        name,
        mantra,
        circle: match SpellCircle::new(circle) {
            Some(circle) => circle,
            None => panic!("spell circle must be in 1..=8"),
        },
        reagents,
        target,
        gesture,
        effect,
        art,
    }
}

/// A spell's landing art, written the way the reference writes it: the sound id
/// and the graphic as bare client numbers, so a row can be read straight against
/// ServUO's own `PlaySound`/`FixedParticles` call.
const fn art(sound: u16, visual: SpellVisual) -> SpellArt {
    SpellArt::Landing {
        sound: SoundId(sound),
        visual,
    }
}

/// The art of a spell that has none of its own — see [`SpellArt::Silent`].
const SILENT: SpellArt = SpellArt::Silent;

/// The 64 Magery spells, indexed by their zero-based spellbook id (the `0xBF`
/// cast request's value, already one-based-decremented). Order is the classic
/// spellbook: eight per circle, Clumsy first.
pub static MAGERY: [SpellInfo; 64] = [
    // -- First circle --------------------------------------------------------
    spell(
        "Clumsy",
        "Uus Jux",
        1,
        &[BLOOD_MOSS, NIGHTSHADE],
        Mobile,
        Directed,
        StatMod(StatEffectKind::CLUMSY),
        art(0x01DF, OnTarget(Graphic(0x3779))),
    ),
    spell(
        "Create Food",
        "In Mani Ylem",
        1,
        &[GARLIC, GINSENG, MANDRAKE_ROOT],
        SelfCast,
        Directed,
        Unimplemented,
        SILENT,
    ),
    spell(
        "Feeblemind",
        "Rel Wis",
        1,
        &[GINSENG, NIGHTSHADE],
        Mobile,
        Directed,
        StatMod(StatEffectKind::FEEBLEMIND),
        art(0x01DF, OnTarget(Graphic(0x3779))),
    ),
    spell(
        "Heal",
        "In Mani",
        1,
        &[GARLIC, GINSENG, SPIDERS_SILK],
        Mobile,
        Directed,
        Heal(15),
        art(0x01F2, OnTarget(Graphic(0x376A))),
    ),
    spell(
        "Magic Arrow",
        "In Por Ylem",
        1,
        &[SULFUROUS_ASH],
        Mobile,
        Directed,
        Damage(Fire, 6),
        art(0x01E5, Bolt(Graphic(0x36E4))),
    ),
    spell(
        "Night Sight",
        "In Lor",
        1,
        &[SULFUROUS_ASH, SPIDERS_SILK],
        Mobile,
        Directed,
        BehaviourBuff(openshard_state::BehaviourBuffKind::NIGHT_SIGHT),
        art(0x01E3, OnTarget(Graphic(0x376A))),
    ),
    spell(
        "Reactive Armor",
        "Flam Sanct",
        1,
        &[GARLIC, SPIDERS_SILK, SULFUROUS_ASH],
        SelfCast,
        Directed,
        BehaviourBuff(openshard_state::BehaviourBuffKind::REACTIVE_ARMOR),
        art(0x01F2, OnTarget(Graphic(0x376A))),
    ),
    spell(
        "Weaken",
        "Des Mani",
        1,
        &[GARLIC, NIGHTSHADE],
        Mobile,
        Directed,
        StatMod(StatEffectKind::WEAKEN),
        art(0x01DF, OnTarget(Graphic(0x3779))),
    ),
    // -- Second circle -------------------------------------------------------
    spell(
        "Agility",
        "Ex Uus",
        2,
        &[BLOOD_MOSS, MANDRAKE_ROOT],
        Mobile,
        Directed,
        StatMod(StatEffectKind::AGILITY),
        art(0x01E7, OnTarget(Graphic(0x375A))),
    ),
    spell(
        "Cunning",
        "Uus Wis",
        2,
        &[MANDRAKE_ROOT, NIGHTSHADE],
        Mobile,
        Directed,
        StatMod(StatEffectKind::CUNNING),
        art(0x01EB, OnTarget(Graphic(0x375A))),
    ),
    spell(
        "Cure",
        "An Nox",
        2,
        &[GARLIC, GINSENG],
        Mobile,
        Directed,
        Cure,
        art(0x01E0, OnTarget(Graphic(0x373A))),
    ),
    spell(
        "Harm",
        "An Mani",
        2,
        &[NIGHTSHADE, SPIDERS_SILK],
        Mobile,
        Directed,
        Damage(Cold, 8),
        // The pre-AoS art of the two ServUO carries (`Core.AOS` swaps in sound
        // 0x0FC and a hued 0x374A); this shard is classic era 1 everywhere else,
        // so it takes the classic branch here too.
        art(0x01F1, OnTarget(Graphic(0x374A))),
    ),
    spell(
        "Magic Trap",
        "In Jux",
        2,
        &[GARLIC, SULFUROUS_ASH, SPIDERS_SILK],
        Location,
        Directed,
        Unimplemented,
        SILENT,
    ),
    spell(
        "Magic Untrap",
        "An Jux",
        2,
        &[BLOOD_MOSS, SULFUROUS_ASH],
        Location,
        Directed,
        Unimplemented,
        SILENT,
    ),
    spell(
        "Protection",
        "Uus Sanct",
        2,
        &[GARLIC, GINSENG, SULFUROUS_ASH],
        SelfCast,
        Directed,
        BehaviourBuff(openshard_state::BehaviourBuffKind::PROTECTION),
        art(0x01ED, OnTarget(Graphic(0x375A))),
    ),
    spell(
        "Strength",
        "Uus Mani",
        2,
        &[MANDRAKE_ROOT, NIGHTSHADE],
        Mobile,
        Directed,
        StatMod(StatEffectKind::STRENGTH),
        art(0x01EE, OnTarget(Graphic(0x375A))),
    ),
    // -- Third circle --------------------------------------------------------
    spell(
        "Bless",
        "Rel Sanct",
        3,
        &[GARLIC, MANDRAKE_ROOT],
        Mobile,
        Directed,
        StatMod(StatEffectKind::BLESS),
        art(0x01EA, OnTarget(Graphic(0x373A))),
    ),
    spell(
        "Fireball",
        "Vas Flam",
        3,
        &[BLACK_PEARL],
        Mobile,
        Directed,
        Damage(Fire, 12),
        // Classic era 1 again: ServUO's `Core.AOS ? 0x15E : 0x44B`.
        art(0x044B, Bolt(Graphic(0x36D4))),
    ),
    spell(
        "Magic Lock",
        "An Por",
        3,
        &[BLOOD_MOSS, GARLIC, SULFUROUS_ASH],
        Location,
        Directed,
        Unimplemented,
        SILENT,
    ),
    spell(
        "Poison",
        "In Nox",
        3,
        &[NIGHTSHADE],
        Mobile,
        Directed,
        Poison,
        art(0x0205, OnTarget(Graphic(0x374A))),
    ),
    spell(
        "Telekinesis",
        "Ort Por Ylem",
        3,
        &[BLOOD_MOSS, MANDRAKE_ROOT],
        Location,
        Directed,
        Unimplemented,
        SILENT,
    ),
    spell(
        "Teleport",
        "Rel Por",
        3,
        &[BLOOD_MOSS, MANDRAKE_ROOT],
        Location,
        Directed,
        Teleport,
        art(0x01FE, AtSpot(Graphic(0x3728))),
    ),
    spell(
        "Unlock",
        "Ex Por",
        3,
        &[BLOOD_MOSS, SULFUROUS_ASH],
        Location,
        Directed,
        Unimplemented,
        SILENT,
    ),
    spell(
        "Wall of Stone",
        "In Sanct Ylem",
        3,
        &[BLOOD_MOSS, GARLIC],
        Location,
        Directed,
        Field(FieldKind::Stone),
        art(0x01F6, Unseen),
    ),
    // -- Fourth circle -------------------------------------------------------
    spell(
        "Arch Cure",
        "Vas An Nox",
        4,
        &[GARLIC, GINSENG, MANDRAKE_ROOT],
        Location,
        Directed,
        AreaCure,
        // ServUO sounds 0x299 at the spot and sparkles each mobile it cures; the
        // sparkle is planted at the spot instead, one art for one cast.
        art(0x0299, AtSpot(Graphic(0x373A))),
    ),
    spell(
        "Arch Protection",
        "Vas Uus Sanct",
        4,
        &[GARLIC, GINSENG, MANDRAKE_ROOT, SULFUROUS_ASH],
        Location,
        Directed,
        Unimplemented,
        SILENT,
    ),
    spell(
        "Curse",
        "Des Sanct",
        4,
        &[GARLIC, NIGHTSHADE, SULFUROUS_ASH],
        Mobile,
        Directed,
        StatMod(StatEffectKind::CURSE),
        art(0x01E1, OnTarget(Graphic(0x374A))),
    ),
    spell(
        "Fire Field",
        "In Flam Grav",
        4,
        &[BLACK_PEARL, SULFUROUS_ASH, SPIDERS_SILK],
        Location,
        Directed,
        Field(FieldKind::Fire),
        art(0x020C, Unseen),
    ),
    spell(
        "Greater Heal",
        "In Vas Mani",
        4,
        &[GARLIC, GINSENG, MANDRAKE_ROOT, SPIDERS_SILK],
        Mobile,
        Directed,
        Heal(35),
        art(0x0202, OnTarget(Graphic(0x376A))),
    ),
    spell(
        "Lightning",
        "Por Ort Grav",
        4,
        &[MANDRAKE_ROOT, SULFUROUS_ASH],
        Mobile,
        Directed,
        Damage(Energy, 14),
        // 0x29 is the thunderclap ServUO's `Effects.SendBoltEffect` plays with
        // the strike; the strike itself has no art id.
        art(0x0029, Lightning),
    ),
    spell(
        "Mana Drain",
        "Ort Rel",
        4,
        &[BLACK_PEARL, MANDRAKE_ROOT, SPIDERS_SILK],
        Mobile,
        Directed,
        Unimplemented,
        SILENT,
    ),
    spell(
        "Recall",
        "Kal Ort Por",
        4,
        &[BLOOD_MOSS, BLACK_PEARL, MANDRAKE_ROOT],
        Item,
        Directed,
        SpellEffect::Recall,
        SILENT,
    ),
    // -- Fifth circle --------------------------------------------------------
    spell(
        "Blade Spirits",
        "In Jux Hur Ylem",
        5,
        &[BLACK_PEARL, MANDRAKE_ROOT, NIGHTSHADE],
        Location,
        Area,
        SpellEffect::Summon(SummonKind::BladeSpirits),
        art(0x0212, Unseen),
    ),
    spell(
        "Dispel Field",
        "An Grav",
        5,
        &[BLACK_PEARL, GARLIC, SULFUROUS_ASH, SPIDERS_SILK],
        // The object cursor, not the ground one: ServUO's target is built with
        // `allowGround: false` and answers with the field *item* it was clicked on.
        // Aiming at a tile would mean guessing which of the things standing on it
        // the caster meant.
        Item,
        Directed,
        DispelField,
        // Voiced where the tile goes, not here: the sparkle and pop are ServUO's
        // own, played after the field is found dispellable, so a Dispel Field aimed
        // at a rock is as quiet as a refused Mark.
        SILENT,
    ),
    spell(
        "Incognito",
        "Kal In Ex",
        5,
        &[BLOOD_MOSS, GARLIC, NIGHTSHADE],
        SelfCast,
        Directed,
        Unimplemented,
        SILENT,
    ),
    spell(
        "Magic Reflection",
        "In Jux Sanct",
        5,
        &[GARLIC, MANDRAKE_ROOT, SPIDERS_SILK],
        SelfCast,
        Directed,
        BehaviourBuff(openshard_state::BehaviourBuffKind::MAGIC_REFLECT),
        art(0x01E9, OnTarget(Graphic(0x375A))),
    ),
    spell(
        "Mind Blast",
        "Por Corp Wis",
        5,
        &[BLACK_PEARL, MANDRAKE_ROOT, NIGHTSHADE, SULFUROUS_ASH],
        Mobile,
        Directed,
        Damage(Cold, 14),
        art(0x0213, OnTarget(Graphic(0x374A))),
    ),
    spell(
        "Paralyze",
        "An Ex Por",
        5,
        &[GARLIC, MANDRAKE_ROOT, SPIDERS_SILK],
        Mobile,
        Directed,
        Paralyze,
        art(0x0204, OnTarget(Graphic(0x376A))),
    ),
    spell(
        "Poison Field",
        "In Nox Grav",
        5,
        &[BLACK_PEARL, NIGHTSHADE, SPIDERS_SILK],
        Location,
        Directed,
        Field(FieldKind::Poison),
        art(0x020B, Unseen),
    ),
    spell(
        "Summon Creature",
        "Kal Xen",
        5,
        &[BLOOD_MOSS, MANDRAKE_ROOT, SPIDERS_SILK],
        // No cursor: ServUO's `SpellInfo` for it passes `allowTarg: false` and
        // `OnCast` summons beside the caster without asking where. It read
        // `Location` here while the row did nothing, and a target cursor for a
        // spell that ignores the answer is a lie the moment the row runs.
        SelfCast,
        // The one summon ServUO gives the directed gesture: its `SpellInfo` names
        // group 16 outright rather than an id in the 260s.
        Directed,
        SpellEffect::Summon(SummonKind::Creature),
        art(0x0215, Unseen),
    ),
    // -- Sixth circle --------------------------------------------------------
    spell(
        "Dispel",
        "An Ort",
        6,
        // Sulfurous ash, not spider's silk: ServUO's `SpellInfo` and the classic
        // list agree, and the row had the wrong third reagent — the same kind of
        // invisible error Gate Travel's blood moss was.
        &[GARLIC, MANDRAKE_ROOT, SULFUROUS_ASH],
        Mobile,
        Directed,
        Dispel,
        // Voiced by the outcome, which is two different pictures: a summon that goes
        // leaves `npc::unsummon`'s puff, and one that holds flashes and stays.
        SILENT,
    ),
    spell(
        "Energy Bolt",
        "Corp Por",
        6,
        &[BLACK_PEARL, NIGHTSHADE],
        Mobile,
        Directed,
        Damage(Energy, 20),
        art(0x020A, Bolt(Graphic(0x379F))),
    ),
    spell(
        "Explosion",
        "Vas Ort Flam",
        6,
        &[BLOOD_MOSS, MANDRAKE_ROOT],
        Mobile,
        Directed,
        Damage(Fire, 20),
        art(0x0307, OnTarget(Graphic(0x36BD))),
    ),
    spell(
        "Invisibility",
        "An Lor Xen",
        6,
        &[BLOOD_MOSS, NIGHTSHADE],
        Mobile,
        Directed,
        Unimplemented,
        SILENT,
    ),
    spell(
        "Mark",
        "Kal Por Ylem",
        6,
        &[BLOOD_MOSS, BLACK_PEARL, MANDRAKE_ROOT],
        Item,
        Directed,
        SpellEffect::Mark,
        // Voiced beside the rune it writes, not here: ServUO plays 0x1FA on the
        // caster *after* the mark takes, so a refused mark stays quiet, and the
        // aimed rune is in a pack and has no world position to sound at.
        SILENT,
    ),
    spell(
        "Mass Curse",
        "Vas Des Sanct",
        6,
        &[GARLIC, MANDRAKE_ROOT, NIGHTSHADE, SULFUROUS_ASH],
        Location,
        Directed,
        Unimplemented,
        SILENT,
    ),
    spell(
        "Paralyze Field",
        "In Ex Grav",
        6,
        &[BLACK_PEARL, GINSENG, SPIDERS_SILK],
        Location,
        Directed,
        Field(FieldKind::Paralyze),
        art(0x020B, Unseen),
    ),
    spell(
        "Reveal",
        "Wis Quas",
        6,
        &[BLOOD_MOSS, SULFUROUS_ASH],
        Location,
        Directed,
        Unimplemented,
        SILENT,
    ),
    // -- Seventh circle ------------------------------------------------------
    spell(
        "Chain Lightning",
        "Vas Ort Grav",
        7,
        &[BLOOD_MOSS, BLACK_PEARL, MANDRAKE_ROOT, SULFUROUS_ASH],
        Location,
        Directed,
        AreaDamage(Energy, 22),
        // ServUO strikes every mobile the blast catches; one strike stands at the
        // aimed spot until per-victim art lands.
        art(0x0029, Lightning),
    ),
    spell(
        "Energy Field",
        "In Sanct Grav",
        7,
        &[BLACK_PEARL, MANDRAKE_ROOT, SULFUROUS_ASH, SPIDERS_SILK],
        Location,
        Directed,
        Field(FieldKind::Energy),
        art(0x020B, Unseen),
    ),
    spell(
        "Flamestrike",
        "Kal Vas Flam",
        7,
        &[SULFUROUS_ASH, SPIDERS_SILK],
        Mobile,
        Directed,
        Damage(Fire, 28),
        art(0x0208, OnTarget(Graphic(0x3709))),
    ),
    spell(
        "Gate Travel",
        "Vas Rel Por",
        7,
        // Black pearl, not blood moss: ServUO's `GateTravel.cs` and the classic
        // reagent list agree, and the row had blood moss — which made the one
        // spell in the family that opens a gate cost the wrong reagent.
        &[BLACK_PEARL, MANDRAKE_ROOT, SULFUROUS_ASH],
        Item,
        Area,
        SpellEffect::GateTravel,
        SILENT,
    ),
    spell(
        "Mana Vampire",
        "Ort Sanct",
        7,
        &[BLOOD_MOSS, BLACK_PEARL, MANDRAKE_ROOT, SPIDERS_SILK],
        Mobile,
        Directed,
        Unimplemented,
        SILENT,
    ),
    spell(
        "Mass Dispel",
        "Vas An Ort",
        7,
        // Sulfurous ash for spider's silk here too — the same wrong reagent as
        // Dispel's, and from the same place.
        &[BLACK_PEARL, GARLIC, MANDRAKE_ROOT, SULFUROUS_ASH],
        Location,
        Area,
        MassDispel,
        // Per victim, and each of them the outcome's — see Dispel above.
        SILENT,
    ),
    spell(
        "Meteor Swarm",
        "Flam Kal Des Ylem",
        7,
        &[BLOOD_MOSS, MANDRAKE_ROOT, SULFUROUS_ASH, SPIDERS_SILK],
        Location,
        Directed,
        AreaDamage(Fire, 24),
        // ServUO throws one of these at each mobile the swarm catches; one
        // fireball flies to the aimed spot until per-victim art lands.
        art(0x0160, Bolt(Graphic(0x36D4))),
    ),
    spell(
        "Polymorph",
        "Vas Ylem Rel",
        7,
        &[BLOOD_MOSS, MANDRAKE_ROOT, SPIDERS_SILK],
        SelfCast,
        Directed,
        Unimplemented,
        SILENT,
    ),
    // -- Eighth circle -------------------------------------------------------
    spell(
        "Earthquake",
        "In Vas Por",
        8,
        &[BLOOD_MOSS, GINSENG, MANDRAKE_ROOT, SULFUROUS_ASH],
        SelfCast,
        Directed,
        AreaDamage(Physical, 30),
        // The reference gives it neither sound nor particle, at the spot or on a
        // victim: what an earthquake sounds like is everyone it hurts.
        SILENT,
    ),
    spell(
        "Energy Vortex",
        "Vas Corp Por",
        8,
        &[BLACK_PEARL, BLOOD_MOSS, MANDRAKE_ROOT, NIGHTSHADE],
        Location,
        Area,
        SpellEffect::Summon(SummonKind::EnergyVortex),
        art(0x0212, Unseen),
    ),
    spell(
        "Resurrection",
        "An Corp",
        8,
        &[BLOOD_MOSS, GARLIC, GINSENG],
        Mobile,
        Directed,
        SpellEffect::Resurrect,
        art(0x0214, OnTarget(Graphic(0x376A))),
    ),
    spell(
        "Air Elemental",
        "Kal Vas Xen Hur",
        8,
        &[BLOOD_MOSS, MANDRAKE_ROOT, SPIDERS_SILK],
        SelfCast,
        Area,
        SpellEffect::Summon(SummonKind::AirElemental),
        art(0x0217, Unseen),
    ),
    spell(
        "Summon Daemon",
        "Kal Vas Xen Corp",
        8,
        &[BLOOD_MOSS, MANDRAKE_ROOT, SULFUROUS_ASH, SPIDERS_SILK],
        SelfCast,
        Area,
        SpellEffect::Summon(SummonKind::Daemon),
        art(0x0216, Unseen),
    ),
    spell(
        "Earth Elemental",
        "Kal Vas Xen Ylem",
        8,
        &[BLOOD_MOSS, MANDRAKE_ROOT, SPIDERS_SILK],
        SelfCast,
        Area,
        SpellEffect::Summon(SummonKind::EarthElemental),
        art(0x0217, Unseen),
    ),
    spell(
        "Fire Elemental",
        "Kal Vas Xen Flam",
        8,
        &[BLOOD_MOSS, MANDRAKE_ROOT, SPIDERS_SILK, SULFUROUS_ASH],
        SelfCast,
        Area,
        SpellEffect::Summon(SummonKind::FireElemental),
        art(0x0217, Unseen),
    ),
    spell(
        "Water Elemental",
        "Kal Vas Xen An Flam",
        8,
        &[BLOOD_MOSS, MANDRAKE_ROOT, SPIDERS_SILK],
        SelfCast,
        Area,
        SpellEffect::Summon(SummonKind::WaterElemental),
        art(0x0217, Unseen),
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
    cast_band(circle_index(info))
}

/// The band a spell cast *off a scroll* is rolled against.
///
/// ServUO's `MagerySpell.GetCastSkills` subtracts two circles when the cast came
/// from a scroll, and that relief is the whole reason a scroll is worth buying:
/// an eighth-circle scroll is rolled as if it were a sixth-circle spell, so a
/// mage who cannot yet hold the circle can still get the spell off.
///
/// The index goes *negative* for the first two circles, which is why this
/// arithmetic is signed. A band below zero is one nobody fails, and that is the
/// intended answer: a Magic Arrow scroll always works.
#[must_use]
pub fn scroll_cast_skills(info: &SpellInfo) -> (i32, i32) {
    cast_band(circle_index(info) - SCROLL_CIRCLE_RELIEF)
}

/// How many circles easier a scroll makes its spell — ServUO's `circle -= 2`.
const SCROLL_CIRCLE_RELIEF: i32 = 2;

/// A circle's `0`-based place in the eight, which is what the band measures
/// from: the first circle is `0` and the eighth `7`.
fn circle_index(info: &SpellInfo) -> i32 {
    i32::from(info.circle.get() - SpellCircle::MIN)
}

/// The band around a circle index's centre, in tenths — ServUO's
/// `MagerySpell.GetCastSkills`: the centre climbs 100/7 skill points a circle,
/// so the first circle sits at 0.0 and the eighth at 100.0, and the band is
/// twenty points either side of it.
fn cast_band(circle: i32) -> (i32, i32) {
    let centre = circle * 1000 / 7;
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

    /// A scroll is worth buying because it is rolled as a spell two circles
    /// lower: ServUO's `MagerySpell.GetCastSkills` with `circle -= 2`. Stated as
    /// an identity between two bands rather than as literal numbers, so it stays
    /// true if the band arithmetic itself is ever retuned.
    #[test]
    fn a_scroll_is_rolled_two_circles_easier() {
        let eighth = info(SpellId(63)).unwrap(); // Earthquake, eighth circle
        let sixth = info(SpellId(47)).unwrap(); // Mark, sixth
        assert_eq!(eighth.circle.get(), 8);
        assert_eq!(sixth.circle.get(), 6);
        assert_eq!(
            scroll_cast_skills(eighth),
            cast_skills(sixth),
            "an eighth-circle scroll is rolled as a sixth-circle spell"
        );
        // And the relief runs off the bottom of the table rather than clamping at
        // the first circle: a Clumsy scroll's whole band is below zero, which is a
        // roll nobody fails.
        let first = info(SpellId(0)).unwrap();
        assert_eq!(first.circle.get(), 1);
        assert!(
            scroll_cast_skills(first).1 < 0,
            "a first-circle scroll can still be failed"
        );
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

    /// The three dispels, and the three different things they aim at.
    ///
    /// The target column is the half of each row a player feels first — it decides
    /// which cursor the client raises — and all three are different for reasons that
    /// are invisible from the Rust: Dispel Field answers with the field *item*
    /// (ServUO builds its target with `allowGround: false`), Dispel with a mobile,
    /// and Mass Dispel with a spot on the ground it then sweeps around.
    ///
    /// The reagents are pinned for [`the_travel_spells_cost_what_the_reference_says`]'s
    /// reason, and two of these three rows were wrong in exactly that invisible way:
    /// both named spider's silk where the reference has sulfurous ash.
    #[test]
    fn the_dispel_family_aims_at_three_different_things() {
        let field = info(SpellId(33)).unwrap();
        assert_eq!(field.name, "Dispel Field");
        assert_eq!(field.effect, SpellEffect::DispelField);
        assert_eq!(field.target, SpellTarget::Item);
        assert_eq!(
            field.reagents,
            &[BLACK_PEARL, GARLIC, SULFUROUS_ASH, SPIDERS_SILK]
        );

        let one = info(SpellId(40)).unwrap();
        assert_eq!(one.name, "Dispel");
        assert_eq!(one.effect, SpellEffect::Dispel);
        assert_eq!(one.target, SpellTarget::Mobile);
        assert_eq!(
            one.reagents,
            &[GARLIC, MANDRAKE_ROOT, SULFUROUS_ASH],
            "sulfurous ash, not spider's silk — ServUO's Dispel.cs"
        );

        let many = info(SpellId(53)).unwrap();
        assert_eq!(many.name, "Mass Dispel");
        assert_eq!(many.effect, SpellEffect::MassDispel);
        assert_eq!(many.target, SpellTarget::Location);
        assert_eq!(
            many.reagents,
            &[BLACK_PEARL, GARLIC, MANDRAKE_ROOT, SULFUROUS_ASH],
            "sulfurous ash here too — ServUO's MassDispel.cs"
        );
    }

    /// Every spell has words. There is no such thing as a Magery spell cast
    /// silently: ServUO gives all sixty-four a `Mantra`, and a row that grew a
    /// blank one would cast with an empty line over the caster's head rather than
    /// with no line at all.
    #[test]
    fn every_spell_has_power_words() {
        for spell in &MAGERY {
            assert!(!spell.mantra.is_empty(), "{} has no mantra", spell.name);
            assert!(
                spell.mantra.chars().all(|c| c.is_ascii_alphabetic() || c == ' '),
                "{}'s mantra is Britannian, not punctuation: {:?}",
                spell.name,
                spell.mantra
            );
        }
        assert_eq!(info(SpellId(17)).unwrap().mantra, "Vas Flam");
        assert_eq!(info(SpellId(50)).unwrap().mantra, "Kal Vas Flam");
    }

    /// The nine spells that raise both arms, and no others.
    ///
    /// Worth pinning because the source of the split is invisible from the Rust:
    /// ServUO names a per-spell action id in the 203..=269 range, and only the
    /// client's `Anim2.def` says that 203..=245 all mean `{16}` while 260..=269
    /// mean `{17}`. Seven of the nine call a creature up; the other two are Gate
    /// Travel and Mass Dispel, which reach out with the same two-armed gesture.
    #[test]
    fn only_the_summoning_family_and_its_two_kin_raise_both_arms() {
        let area: Vec<&str> = MAGERY
            .iter()
            .filter(|spell| spell.gesture == CastGesture::Area)
            .map(|spell| spell.name)
            .collect();
        assert_eq!(
            area,
            [
                "Blade Spirits",
                "Gate Travel",
                "Mass Dispel",
                "Energy Vortex",
                "Air Elemental",
                "Summon Daemon",
                "Earth Elemental",
                "Fire Elemental",
                "Water Elemental",
            ]
        );
        // Summon Creature is the exception in the reference itself: its
        // `SpellInfo` names group 16 outright rather than an id in the 260s.
        assert_eq!(info(SpellId(39)).unwrap().gesture, CastGesture::Directed);
    }

    /// A spell the core runs is a spell the player can see and hear.
    ///
    /// The invariant the art table exists for: art is no longer derived from the
    /// effect, so nothing but this stops a built spell from landing in silence.
    /// The three exemptions are named rather than filtered by shape, so adding a
    /// fourth is a deliberate edit.
    #[test]
    fn every_spell_the_core_runs_has_art_or_a_reason_not_to() {
        for spell in &MAGERY {
            let voiced_elsewhere = matches!(
                spell.effect,
                SpellEffect::Recall
                    | SpellEffect::GateTravel
                    | SpellEffect::Mark
                    // Two outcomes and two pictures: the puff a summon leaves, or
                    // the flash of one that shrugged the dispel off.
                    | SpellEffect::Dispel
                    | SpellEffect::MassDispel
                    | SpellEffect::DispelField
            );
            // Earthquake: ServUO gives it neither sound nor particle.
            let silent_in_the_reference = spell.name == "Earthquake";
            let built = spell.effect != SpellEffect::Unimplemented;
            if built && !voiced_elsewhere && !silent_in_the_reference {
                assert!(
                    spell.art != SpellArt::Silent,
                    "{} runs in the core and would land in silence",
                    spell.name
                );
            }
            // And the converse: a spell that does nothing announces nothing.
            if !built {
                assert_eq!(
                    spell.art,
                    SpellArt::Silent,
                    "{} has no effect, so it has no landing to voice",
                    spell.name
                );
            }
        }
    }

    /// A field's tiles are its picture, and nothing else is drawn over them.
    #[test]
    fn a_field_is_heard_and_not_seen() {
        for spell in MAGERY
            .iter()
            .filter(|s| matches!(s.effect, SpellEffect::Field(_)))
        {
            assert!(
                matches!(
                    spell.art,
                    SpellArt::Landing {
                        visual: SpellVisual::Unseen,
                        ..
                    }
                ),
                "{} would draw a second picture over its own tiles",
                spell.name
            );
        }
    }

    /// A summon that is laid on a tile asks for one, and one that is not does not.
    ///
    /// The same fact is written in two tables — the spell's target column here, and
    /// [`openshard_state::summon::SummonData::at_the_mark`] there — because each is
    /// the natural home for one half: the client is told which cursor to raise from
    /// the spell, and the spawn point is chosen from the creature. Two copies of one
    /// fact is exactly the shape that drifts, and the drift would be silent in the
    /// worst way: a spell that raises a cursor and then ignores the answer, or one
    /// that summons at the caster's feet while the player is still pointing at a
    /// spot across the room.
    #[test]
    fn a_summon_asks_for_the_tile_it_is_laid_on_and_no_other() {
        for spell in &MAGERY {
            let SpellEffect::Summon(kind) = spell.effect else {
                continue;
            };
            assert_eq!(
                openshard_state::summon::summoned(kind).at_the_mark,
                spell.target == SpellTarget::Location,
                "{} disagrees with its creature about where it lands",
                spell.name
            );
        }
        // And the family is all eight, so a ninth row cannot join it unnoticed.
        assert_eq!(
            MAGERY
                .iter()
                .filter(|spell| matches!(spell.effect, SpellEffect::Summon(_)))
                .count(),
            8
        );
    }

    /// The art rows, spot-checked against ServUO's own calls.
    ///
    /// The gap this closed: the visual used to be keyed on the coarse effect, so
    /// every fire spell threw the same bolt and every stat buff the same sparkle.
    /// Flamestrike and Clumsy are the two that moved furthest.
    #[test]
    fn the_art_is_the_spells_own_and_not_its_archetypes() {
        // `m.FixedParticles(0x3709, 10, 30, 5052, EffectLayer.LeftFoot); m.PlaySound(0x208);`
        assert_eq!(
            info(SpellId(50)).unwrap().art,
            art(0x0208, OnTarget(Graphic(0x3709))),
            "Flamestrike burns at the feet; it does not throw a fireball"
        );
        // `m.FixedParticles(0x3779, 10, 15, 5002, EffectLayer.Head); m.PlaySound(0x1DF);`
        assert_eq!(
            info(SpellId(0)).unwrap().art,
            art(0x01DF, OnTarget(Graphic(0x3779))),
            "a curse is not drawn in a blessing's sparkle"
        );
        assert_ne!(
            info(SpellId(0)).unwrap().art,
            info(SpellId(16)).unwrap().art,
            "Clumsy and Bless were one row of art between them"
        );
        assert_ne!(
            info(SpellId(4)).unwrap().art,
            info(SpellId(17)).unwrap().art,
            "Magic Arrow and Fireball were one row of art between them"
        );
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
