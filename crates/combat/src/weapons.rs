//! Weapon properties — the speed and damage a wielded weapon lends its bearer.
//!
//! These numbers are **not** in `tiledata.mul`: the client and both reference
//! emulators keep them per weapon *class*, not per tile. So they live here, a core
//! table keyed by item graphic, ported from ServUO's `BaseWeapon` subclasses — the
//! same "data keyed by graphic, default in core" shape as
//! [`creature_name`](openshard_state::components::creature_name). A pack may layer a
//! per-item override on top later (a magic sword); this is the base every weapon
//! falls back to.
//!
//! Two number sets per weapon, because the engine runs two eras: ServUO's `Old*`
//! (pre-AoS, combat era 1) and `Aos*` (era 2). [`by_era`] picks between them, and
//! [`super::swing_speed`]/[`super::melee_blow`] read the chosen values — the
//! wielder's dexterity and the target's resistance are applied by combat as before.
//! What is deliberately *not* here yet: the weapon's `skill` is carried but not yet
//! consumed (hit chance and skill-gain are a later slice), archery damage still runs
//! flat through `volleys`, and AoS strength/tactics damage bonuses are unwritten.

use openshard_entities::EntityId;
use openshard_state::components::{Equipped, Graphic, Weapon};
use openshard_state::WorldState;

/// The paperdoll layer a one-handed weapon sits on (UO layer 1).
pub const LAYER_ONE_HANDED: u8 = 1;
/// The paperdoll layer a two-handed weapon or shield sits on (UO layer 2).
pub const LAYER_TWO_HANDED: u8 = 2;

/// Classic UO skill ids the combat rolls read (`Skills` is keyed by these `u8`s).
pub const ANATOMY_SKILL: u8 = 1;
/// Fencing.
pub const FENCING_SKILL: u8 = 13;
/// Mace fighting.
pub const MACING_SKILL: u8 = 15;
/// Tactics — the damage-scaling skill both eras read.
pub const TACTICS_SKILL: u8 = 30;
/// Archery.
pub const ARCHERY_SKILL: u8 = 31;
/// Wrestling — the bare-hands weapon skill, and the defender's fallback.
pub const WRESTLING_SKILL: u8 = 34;
/// Swordsmanship.
pub const SWORDS_SKILL: u8 = 41;

/// Which combat skill a weapon trains and hits with. Carried from ServUO's
/// `DefSkill` for the slice that wires hit chance and gain; unused today, but free
/// to port now while the table is being written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeaponSkill {
    Swords,
    Macing,
    Fencing,
    Archery,
    /// Bare hands — the fallback for a mobile wielding nothing in the table.
    Wrestling,
}

impl WeaponSkill {
    /// The `Skills` id this weapon trains and rolls its to-hit against.
    #[must_use]
    pub const fn skill_id(self) -> u8 {
        match self {
            Self::Swords => SWORDS_SKILL,
            Self::Macing => MACING_SKILL,
            Self::Fencing => FENCING_SKILL,
            Self::Archery => ARCHERY_SKILL,
            Self::Wrestling => WRESTLING_SKILL,
        }
    }
}

/// The weapon skill a mobile fights with: its wielded weapon's, or Wrestling for
/// bare hands (or anything not in the table). The id the to-hit roll and the swing
/// gain both key on.
#[must_use]
pub fn combat_skill_id(state: &WorldState, mobile: EntityId) -> u8 {
    equipped_weapon(state, mobile).map_or(WRESTLING_SKILL, |weapon| weapon.skill.skill_id())
}

/// One weapon's combat numbers, keyed by its item [`Graphic`] id.
#[derive(Debug, Clone, Copy)]
pub struct WeaponData {
    /// The item graphic this row describes.
    pub graphic: u16,
    /// The skill it trains and strikes with.
    pub skill: WeaponSkill,
    /// Pre-AoS (era 1) speed constant — the `base` in Sphere's swing formula.
    pub old_speed: u16,
    /// Pre-AoS minimum damage, before resistance.
    pub old_min: u16,
    /// Pre-AoS maximum damage, before resistance.
    pub old_max: u16,
    /// AoS (era 2) speed constant.
    pub aos_speed: u16,
    /// AoS minimum damage.
    pub aos_min: u16,
    /// AoS maximum damage.
    pub aos_max: u16,
}

/// Pick the era-appropriate value: AoS (era 2) or pre-AoS (era 1 and the fallback).
#[must_use]
pub const fn by_era(old: u16, aos: u16, era: u8) -> u16 {
    if era == 2 {
        aos
    } else {
        old
    }
}

/// The weapon row for an item graphic, or `None` for anything not a known weapon
/// (a torch, a spellbook, a shield, bare hands).
#[must_use]
pub fn weapon_data(graphic: u16) -> Option<&'static WeaponData> {
    WEAPONS.iter().find(|w| w.graphic == graphic)
}

/// The weapon `mobile` wields, if any — the item on a weapon layer. Its stats are
/// the core table's for the item's graphic, unless the item carries a [`Weapon`]
/// override (the pack's magic sword), which replaces speed and damage while
/// keeping the graphic's skill. Read fresh each swing (no mirror on the mobile),
/// so a weapon coming off reverts the bearer to wrestling with nothing to undo.
#[must_use]
pub fn equipped_weapon(state: &WorldState, mobile: EntityId) -> Option<WeaponData> {
    let serial = state.registry.serial_of(mobile)?;
    let item = state
        .registry
        .query::<Equipped>()
        .find(|(_, worn)| {
            worn.mobile == serial
                && (worn.layer == LAYER_ONE_HANDED || worn.layer == LAYER_TWO_HANDED)
        })
        .map(|(entity, _)| entity)?;
    let base = state
        .registry
        .get::<Graphic>(item)
        .and_then(|graphic| weapon_data(graphic.id))
        .copied();
    match state.registry.get::<Weapon>(item) {
        // An override stands the item's stats up, keeping the base graphic's skill
        // (a magic longsword is still a Swords weapon); same numbers either era.
        Some(&Weapon { speed, min, max }) => Some(WeaponData {
            graphic: 0,
            skill: base.map_or(WeaponSkill::Wrestling, |weapon| weapon.skill),
            old_speed: speed,
            old_min: min,
            old_max: max,
            aos_speed: speed,
            aos_min: min,
            aos_max: max,
        }),
        None => base,
    }
}

/// The classic pre-AoS weapon set, ported from
/// `ServUO/Scripts/Items/Equipment/Weapons/*.cs` — each row's graphic from the
/// constructor's `: base(0x…)`, its `Old*`/`Aos*` from the subclass getters, and its
/// skill from the base class's `DefSkill` (BaseSword/Axe/Knife/PoleArm → Swords,
/// BaseBashing/Staff → Macing, BaseSpear → Fencing, BaseRanged → Archery). Kryss is
/// Fencing here: ServUO files it under `BaseSword`, but classic UO trains it with
/// Fencing, and the numbers-taken/arithmetic-audited rule favours the client's truth.
#[rustfmt::skip]
static WEAPONS: &[WeaponData] = &[
    // -- Swords ----------------------------------------------------------------
    w(0x0F61, WeaponSkill::Swords,  35,  5, 33, 30, 14, 18), // Longsword
    w(0x0F5E, WeaponSkill::Swords,  45,  5, 29, 33, 13, 17), // Broadsword
    w(0x13FF, WeaponSkill::Swords,  58,  5, 26, 46, 10, 14), // Katana
    w(0x13B9, WeaponSkill::Swords,  30,  6, 34, 28, 15, 19), // Viking sword
    w(0x1441, WeaponSkill::Swords,  45,  6, 28, 44, 10, 14), // Cutlass
    w(0x13B6, WeaponSkill::Swords,  43,  4, 30, 37, 12, 16), // Scimitar
    w(0x0F52, WeaponSkill::Swords,  55,  3, 15, 56, 10, 12), // Dagger
    w(0x13F6, WeaponSkill::Swords,  40,  2, 14, 49, 10, 13), // Butcher knife
    w(0x0EC3, WeaponSkill::Swords,  40,  2, 13, 46, 10, 14), // Cleaver
    w(0x0EC4, WeaponSkill::Swords,  40,  1, 10, 49, 10, 13), // Skinning knife
    // -- Axes (Swords skill) ---------------------------------------------------
    w(0x0F43, WeaponSkill::Swords,  40,  2, 17, 41, 13, 16), // Hatchet
    w(0x0F49, WeaponSkill::Swords,  37,  6, 33, 37, 14, 17), // Axe
    w(0x0F47, WeaponSkill::Swords,  30,  6, 38, 31, 16, 19), // Battle axe
    w(0x0F4B, WeaponSkill::Swords,  37,  5, 35, 33, 15, 18), // Double axe
    w(0x0F45, WeaponSkill::Swords,  37,  6, 33, 33, 15, 18), // Executioner's axe
    w(0x13FB, WeaponSkill::Swords,  30,  6, 38, 29, 17, 20), // Large battle axe
    w(0x1443, WeaponSkill::Swords,  30,  5, 39, 31, 16, 19), // Two-handed axe
    w(0x13B0, WeaponSkill::Swords,  40,  9, 27, 33, 12, 16), // War axe
    w(0x0E86, WeaponSkill::Swords,  35,  1, 15, 35, 12, 16), // Pickaxe
    // -- Polearms (Swords skill) -----------------------------------------------
    w(0x0F4D, WeaponSkill::Swords,  26,  5, 43, 28, 17, 20), // Bardiche
    w(0x143E, WeaponSkill::Swords,  25,  5, 49, 25, 18, 21), // Halberd
    // -- Maces & staves --------------------------------------------------------
    w(0x13B4, WeaponSkill::Macing,  40,  8, 24, 44, 10, 14), // Club
    w(0x0F5C, WeaponSkill::Macing,  30,  8, 32, 40, 11, 15), // Mace
    w(0x143B, WeaponSkill::Macing,  30, 10, 30, 32, 14, 18), // Maul
    w(0x1407, WeaponSkill::Macing,  32, 10, 30, 26, 16, 20), // War mace
    w(0x1439, WeaponSkill::Macing,  31,  8, 36, 28, 17, 20), // War hammer
    w(0x143D, WeaponSkill::Macing,  30,  6, 33, 28, 13, 17), // Hammer pick
    w(0x0E89, WeaponSkill::Macing,  48,  8, 28, 48, 11, 14), // Quarter staff
    w(0x0DF0, WeaponSkill::Macing,  35,  8, 33, 39, 13, 16), // Black staff
    w(0x13F8, WeaponSkill::Macing,  33, 10, 30, 33, 15, 18), // Gnarled staff
    w(0x0E81, WeaponSkill::Macing,  30,  3, 12, 40, 13, 16), // Shepherd's crook
    // -- Fencing ---------------------------------------------------------------
    w(0x1401, WeaponSkill::Fencing, 53,  3, 28, 53, 10, 12), // Kryss
    w(0x1405, WeaponSkill::Fencing, 45,  4, 32, 43, 10, 14), // War fork
    w(0x0F62, WeaponSkill::Fencing, 46,  2, 36, 42, 13, 16), // Spear
    w(0x1403, WeaponSkill::Fencing, 50,  4, 32, 55, 10, 13), // Short spear
    w(0x0E87, WeaponSkill::Fencing, 45,  4, 16, 43, 12, 15), // Pitchfork
    // -- Archery ---------------------------------------------------------------
    w(0x13B2, WeaponSkill::Archery, 20,  9, 41, 25, 25, 25), // Bow
    w(0x0F50, WeaponSkill::Archery, 18,  8, 43, 24, 18, 24), // Crossbow
    w(0x13FD, WeaponSkill::Archery, 10, 11, 56, 22, 22, 22), // Heavy crossbow
];

/// A terse constructor so the table above stays one weapon per readable line.
// Every argument is a distinct weapon field; a struct literal per row would only
// make the table wordier, which is the opposite of the point.
#[allow(clippy::too_many_arguments)]
const fn w(
    graphic: u16,
    skill: WeaponSkill,
    old_speed: u16,
    old_min: u16,
    old_max: u16,
    aos_speed: u16,
    aos_min: u16,
    aos_max: u16,
) -> WeaponData {
    WeaponData {
        graphic,
        skill,
        old_speed,
        old_min,
        old_max,
        aos_speed,
        aos_min,
        aos_max,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_graphic_resolves_and_an_unknown_one_does_not() {
        let sword = weapon_data(0x0F61).expect("longsword is in the table");
        assert_eq!(sword.skill, WeaponSkill::Swords);
        assert_eq!(sword.old_speed, 35);
        assert_eq!((sword.old_min, sword.old_max), (5, 33));
        assert!(weapon_data(0x0000).is_none());
    }

    #[test]
    fn by_era_picks_aos_only_for_era_two() {
        assert_eq!(by_era(35, 30, 1), 35);
        assert_eq!(by_era(35, 30, 2), 30);
        assert_eq!(by_era(35, 30, 0), 35); // the fallback is pre-AoS
    }

    #[test]
    fn no_two_weapons_share_a_graphic() {
        for (i, a) in WEAPONS.iter().enumerate() {
            for b in &WEAPONS[i + 1..] {
                assert_ne!(
                    a.graphic, b.graphic,
                    "duplicate graphic 0x{:04X}",
                    a.graphic
                );
            }
        }
    }
}
