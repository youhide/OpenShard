//! What each Magery summon calls up — the core's table, keyed by
//! [`SummonKind`](crate::components::SummonKind).
//!
//! ServUO spells one summon out per class under `Scripts/Mobiles/Summons`
//! (`SummonedAirElemental` and its siblings, `BladeSpirits`, `EnergyVortex`,
//! `SummonedDaemon`); those are per-class constants, so they live here in the shape
//! [`crate::tame`] and [`crate::weapon`] established — data the engine owns, read
//! by whoever does the spawning.
//!
//! **Pre-AoS throughout**, era 1 as everywhere else: where the reference branches
//! on `Core.AOS` or `Core.SE` the classic side is taken, which is what makes a
//! daemon cost five follower slots rather than four and a blade spirit one rather
//! than two.
//!
//! What is *not* here is a spawn position or a `SpawnSpec`: making a creature is
//! `npc`'s, and this crate must not know how one is built.

use openshard_protocol::wire::Graphic;
use openshard_protocol::world::{
    Aggression,
    FollowerSlots,
    PhysicalResistance,
    Sight,
};

use crate::components::SummonKind;
use crate::skill::Skill;

/// How long a summon stands before it goes.
///
/// Two rules and no more, because the reference has two: the elementals, the
/// daemon and Summon Creature hold for a span the caster's Magery buys, while the
/// two that are laid on a tile hold for a roll that ignores skill entirely.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SummonLifetime {
    /// Four seconds a point of Magery — ServUO's `(2 * Magery.Fixed) / 5` seconds,
    /// which at grandmaster is four hundred. Skill buys the creature's *time*, and
    /// this is the only place it does: a summon's stats are the same for a novice.
    Magery,
    /// Eighty seconds plus a roll of forty — ServUO's pre-AoS
    /// `Utility.Random(80, 40)`, so 80 through 119 inclusive, with no skill in it.
    /// Blade Spirits and Energy Vortex.
    Rolled,
}

/// The floor of the rolled lifetime, in seconds.
pub const ROLLED_LIFETIME_FROM: u64 = 80;
/// How many seconds wide the rolled lifetime's window is.
pub const ROLLED_LIFETIME_SPAN: u32 = 40;

/// How many seconds of life a Magery value buys under [`SummonLifetime::Magery`].
///
/// ServUO's `(2 * Skills.Magery.Fixed) / 5`, and `Fixed` is the skill in tenths —
/// the unit this engine keeps skills in too — so a grandmaster's `1000` becomes
/// four hundred seconds. Kept in the reference's own shape rather than reduced to
/// "two fifths of a second a tenth": it is the same number and a far worse thing to
/// check a line of `EarthElemental.cs` against.
#[must_use]
pub const fn magery_lifetime_seconds(magery_tenths: u16) -> u64 {
    2 * magery_tenths as u64 / 5
}

/// One summoned creature: the body it wears, what it can do, and what it costs.
///
/// Deliberately flat rather than a `SpawnSpec`: this crate does not know what a
/// spawn is, and the fields here are exactly the ones ServUO's constructor sets.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SummonData {
    /// Its body — `BaseCreature.Body`.
    pub body:        Graphic,
    /// Hit points, which are also its maximum: `SetHits`, or the creature's
    /// strength where the reference sets none (a `BaseCreature` with no
    /// `HitsMaxSeed` takes `Str`, and every summon in the list has 200).
    pub hits:        u16,
    /// The blow it lands, the midpoint of ServUO's `SetDamage(min, max)` — this
    /// engine's `MeleeDamage` is one number, so the range is averaged rather than
    /// rolled per swing.
    pub damage:      u16,
    /// Its physical resistance, the midpoint of `SetResistance(Physical, lo, hi)`.
    pub resistance:  PhysicalResistance,
    /// How far it notices things.
    pub sight:       Sight,
    /// Its trained skills, in tenths — only the three this engine reads (a blow's
    /// chance and scaling from Tactics and Wrestling, a spell shrugged off with
    /// Resisting Spells). The reference's Meditation, Eval Int and Magery are left
    /// out because nothing here would consult them: a creature does not cast.
    pub skills:      &'static [(Skill, u16)],
    /// How much of its master's following it takes up — ServUO's `ControlSlots`,
    /// which is also the number its spell's `CheckCast` demands be free.
    pub slots:       FollowerSlots,
    /// How long it stands.
    pub lifetime:    SummonLifetime,
    /// Whether it is laid on the *aimed* tile rather than beside its caster.
    ///
    /// True for the two that take a target (`BladeSpiritsSpell.Target` spawns at
    /// the point and refuses a blocked one); false for the six that take none, whose
    /// `SpellHelper.Summon` calls `FindValidSpawnLocation(.., surroundingsOnly:
    /// true)` and so never lands on the caster's own tile. The spell table's target
    /// column says the same thing from the other side, and a test in `magic` holds
    /// the two together.
    pub at_the_mark: bool,
}

/// What `kind` summons.
///
/// Total rather than fallible: every [`SummonKind`] names a creature the reference
/// spells out, and a summon with no data would be a spell that costs mana and does
/// nothing — the state the whole slice exists to leave behind.
#[must_use]
pub fn summoned(kind: SummonKind) -> SummonData {
    match kind {
        // The beast is chosen per cast (see `SUMMONABLE_BEASTS`), so the body here
        // is only the fallback shape; the stats are one modest block for all of
        // them — see the list's own note.
        SummonKind::Creature => {
            SummonData {
                body:        SUMMONABLE_BEASTS[0],
                hits:        60,
                damage:      5,
                resistance:  PhysicalResistance::new(20),
                sight:       Sight(8),
                skills:      &[
                    (Skill::Tactics, 500),
                    (Skill::Wrestling, 500),
                    (Skill::MagicResist, 300),
                ],
                // ServUO leaves the creature's own `ControlSlots` commented out and
                // then demands two free in `CheckCast`. Two is the number the
                // player feels, so two is the number both halves use here: a gate
                // that asks for more room than the thing it admits then takes is a
                // follower cap that cannot be reasoned about.
                slots:       FollowerSlots::new(2),
                lifetime:    SummonLifetime::Magery,
                at_the_mark: false,
            }
        }
        SummonKind::BladeSpirits => {
            SummonData {
                body:        Graphic(0x023E),
                hits:        80,
                damage:      12,                          // SetDamage(10, 14)
                resistance:  PhysicalResistance::new(35), // 30..40
                sight:       Sight(10),
                skills:      &[
                    (Skill::Tactics, 900),
                    (Skill::Wrestling, 900),
                    (Skill::MagicResist, 700),
                ],
                slots:       FollowerSlots::new(1),
                lifetime:    SummonLifetime::Rolled,
                at_the_mark: true,
            }
        }
        SummonKind::EnergyVortex => {
            SummonData {
                body:        Graphic(0x00A4),
                hits:        70,
                damage:      15,                          // SetDamage(14, 17)
                resistance:  PhysicalResistance::new(65), // 60..70
                sight:       Sight(10),
                skills:      &[
                    (Skill::Tactics, 1000),
                    (Skill::Wrestling, 1200),
                    (Skill::MagicResist, 999),
                ],
                slots:       FollowerSlots::new(1),
                lifetime:    SummonLifetime::Rolled,
                at_the_mark: true,
            }
        }
        SummonKind::AirElemental => {
            SummonData {
                body:        Graphic(0x000D),
                hits:        150,
                damage:      7,                           // SetDamage(6, 9)
                resistance:  PhysicalResistance::new(45), // 40..50
                sight:       Sight(10),
                skills:      &[
                    (Skill::Tactics, 1000),
                    (Skill::Wrestling, 800),
                    (Skill::MagicResist, 600),
                ],
                slots:       FollowerSlots::new(2),
                lifetime:    SummonLifetime::Magery,
                at_the_mark: false,
            }
        }
        SummonKind::EarthElemental => {
            SummonData {
                body:        Graphic(0x000E),
                hits:        180,
                damage:      17,                          // SetDamage(14, 21)
                resistance:  PhysicalResistance::new(70), // 65..75
                sight:       Sight(10),
                skills:      &[
                    (Skill::Tactics, 1000),
                    (Skill::Wrestling, 900),
                    (Skill::MagicResist, 650),
                ],
                slots:       FollowerSlots::new(2),
                lifetime:    SummonLifetime::Magery,
                at_the_mark: false,
            }
        }
        SummonKind::FireElemental => {
            SummonData {
                // No `SetHits` in the reference: hit points follow `SetStr(200)`.
                body:        Graphic(0x000F),
                hits:        200,
                damage:      11,                          // SetDamage(9, 14)
                resistance:  PhysicalResistance::new(55), // 50..60
                sight:       Sight(10),
                skills:      &[
                    (Skill::Tactics, 1000),
                    (Skill::Wrestling, 920),
                    (Skill::MagicResist, 850),
                ],
                slots:       FollowerSlots::new(4),
                lifetime:    SummonLifetime::Magery,
                at_the_mark: false,
            }
        }
        SummonKind::WaterElemental => {
            SummonData {
                body:        Graphic(0x0010),
                hits:        165,
                damage:      14,                          // SetDamage(12, 16)
                resistance:  PhysicalResistance::new(55), // 50..60
                sight:       Sight(10),
                skills:      &[
                    (Skill::Tactics, 1000),
                    (Skill::Wrestling, 850),
                    (Skill::MagicResist, 750),
                ],
                slots:       FollowerSlots::new(3),
                lifetime:    SummonLifetime::Magery,
                at_the_mark: false,
            }
        }
        SummonKind::Daemon => {
            SummonData {
                // Pre-AoS body 9; the reference's `Core.AOS ? 10 : 9`. No `SetHits`
                // here either, so `SetStr(200)` is the total.
                body:        Graphic(0x0009),
                hits:        200,
                damage:      17,                          // SetDamage(14, 21)
                resistance:  PhysicalResistance::new(50), // 45..55
                sight:       Sight(10),
                skills:      &[
                    (Skill::Tactics, 1000),
                    (Skill::Wrestling, 985),
                    (Skill::MagicResist, 950),
                ],
                slots:       FollowerSlots::new(5),
                lifetime:    SummonLifetime::Magery,
                at_the_mark: false,
            }
        }
    }
}

/// The beasts Summon Creature can call, drawn at random.
///
/// ServUO's `SummonCreatureSpell.m_Types` — a list its own comment says came from
/// an hour of summoning on OSI — narrowed to the bodies this engine can name
/// (`creature_name`), because a summon nobody can identify on single click is worse
/// than a shorter list.
///
/// **They share one stat block**, which the reference does not: there a rabbit and
/// a grizzly are different creatures. This engine has no per-body stat table to
/// draw them from, and inventing eighteen of them is a bestiary and not a spell —
/// so the block above is one modest woodland animal and the variety is skin. The
/// roadmap records it.
pub static SUMMONABLE_BEASTS: &[Graphic] = &[
    Graphic(0x00D5), // a polar bear
    Graphic(0x00D4), // a grizzly bear
    Graphic(0x00A7), // a brown bear
    Graphic(0x00C8), // a horse
    Graphic(0x00DD), // a walrus
    Graphic(0x0019), // a grey wolf
    Graphic(0x00CB), // a pig
    Graphic(0x00ED), // a hind
    Graphic(0x00CD), // a rabbit
];

/// How a summon behaves once it stands.
///
/// Every Magery summon here is *controlled* — ServUO's `SetControlMaster` — so it
/// answers its master's orders and picks no fights of its own. Blade Spirits and
/// Energy Vortex are the reference's exception (it summons those two with
/// `controlled: false`, which is why they famously turn on the mage who called
/// them); reproducing that wants a hostility model this engine does not have, since
/// its own aggression only ever acquires *players*. Recorded in the roadmap.
pub const SUMMON_AGGRESSION: Aggression = Aggression::Defensive;

#[cfg(test)]
mod tests {
    use super::*;

    /// Every summon fits inside the follower cap on its own, and the daemon fills
    /// it exactly.
    ///
    /// Worth pinning because the number is read twice — the refusal before the cast
    /// and the slot the creature then takes — and a kind whose cost exceeded the cap
    /// would be a spell that can never be cast by anyone, refused for a reason the
    /// player cannot act on.
    ///
    /// The cap is written out rather than imported: it is `skills::MAX_FOLLOWERS`,
    /// and that crate is downstream of this one.
    #[test]
    fn no_summon_costs_more_than_a_whole_following() {
        const FOLLOWERS_MAX: u8 = 5;
        for kind in EVERY_KIND {
            let slots = summoned(kind).slots.get();
            assert!(slots >= 1, "{kind:?} would be free");
            assert!(slots <= FOLLOWERS_MAX, "{kind:?} could never be summoned");
        }
        assert_eq!(
            summoned(SummonKind::Daemon).slots.get(),
            5,
            "a daemon is the whole of a mage's following, pre-AoS"
        );
    }

    /// The Magery span, against the reference's own arithmetic.
    ///
    /// Worth its own test because the formula is the only thing a caster's skill
    /// buys a summon, and getting the divisor wrong halves every elemental's life
    /// while leaving every other symptom identical.
    #[test]
    fn skill_buys_a_summon_four_hundred_seconds_at_grandmaster() {
        assert_eq!(magery_lifetime_seconds(1000), 400, "(2 * 1000) / 5");
        assert_eq!(magery_lifetime_seconds(500), 200);
        assert_eq!(
            magery_lifetime_seconds(0),
            0,
            "and nothing at all with no skill — the caller floors it"
        );
    }

    /// Only the two that take a target land on it.
    #[test]
    fn just_the_aimed_pair_is_laid_on_the_mark() {
        let aimed: Vec<SummonKind> = EVERY_KIND
            .into_iter()
            .filter(|&kind| summoned(kind).at_the_mark)
            .collect();
        assert_eq!(aimed, [SummonKind::BladeSpirits, SummonKind::EnergyVortex]);
    }

    /// A summon that could not fight would be a spell with no effect.
    #[test]
    fn every_summon_can_take_and_land_a_blow() {
        for kind in EVERY_KIND {
            let data = summoned(kind);
            assert!(data.hits > 0, "{kind:?} would be born dead");
            assert!(data.damage > 0, "{kind:?} could not hurt anything");
            assert!(
                data.skills
                    .iter()
                    .any(|&(skill, value)| skill == Skill::Wrestling && value > 0),
                "{kind:?} has no Wrestling, so its blows would never be rolled"
            );
        }
    }

    /// The list is walked with the world's rng, which indexes it — an empty one
    /// would divide by zero, and a body the engine cannot name reads as "a
    /// creature" on single click.
    #[test]
    fn the_summonable_beasts_are_a_named_list() {
        assert!(!SUMMONABLE_BEASTS.is_empty());
        for &body in SUMMONABLE_BEASTS {
            assert!(
                crate::components::creature_name(body).is_some(),
                "0x{:04X} has no name to show",
                body.0
            );
        }
    }

    /// Named rather than derived, so a ninth kind is a deliberate edit here too.
    const EVERY_KIND: [SummonKind; 8] = [
        SummonKind::Creature,
        SummonKind::BladeSpirits,
        SummonKind::EnergyVortex,
        SummonKind::AirElemental,
        SummonKind::EarthElemental,
        SummonKind::FireElemental,
        SummonKind::WaterElemental,
        SummonKind::Daemon,
    ];
}
