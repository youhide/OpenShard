//! What the weapon a mobile wields does for it: the rules over
//! [`openshard_state::weapon`]'s data.
//!
//! The table itself — every weapon's speed, damage, kind and miss sound keyed by
//! graphic — is data, and lives in `state` beside the skill and title tables,
//! because `skills` reads the same rows to answer an Arms Lore question. What lives
//! here is the part that only a fight cares about: which item a mobile is actually
//! holding, and which skill that makes it fight with.
//!
//! Both are **read-site derivations**: recomputed each swing, with no mirror on the
//! mobile, so a weapon coming off reverts its bearer to wrestling with nothing to
//! undo.

use openshard_entities::EntityId;
use openshard_protocol::wire::Graphic;
use openshard_state::components::{Drawn, Weapon};
use openshard_state::weapon::{LAYER_ONE_HANDED, WeaponData, WeaponSkill, weapon_data};
use openshard_state::weapon::{LAYER_TWO_HANDED, WeaponKind};
use openshard_state::{Skill, WorldState};

/// The skill ids the combat rolls read (`Skills` is keyed by these `u8`s).
///
/// Off [`openshard_state::Skill`] rather than written out, because five of the
/// eight were **wrong** while they were hand-written constants. See
/// [`WeaponSkill::skill_id`] for the whole story; these are the four the damage
/// scaling reads directly rather than through a weapon row.
pub const ANATOMY_SKILL: Skill = Skill::Anatomy;
/// Fencing.
pub const FENCING_SKILL: Skill = Skill::Fencing;
/// Mace fighting.
pub const MACING_SKILL: Skill = Skill::Macing;
/// Tactics — the damage-scaling skill both eras read.
pub const TACTICS_SKILL: Skill = Skill::Tactics;
/// Archery.
pub const ARCHERY_SKILL: Skill = Skill::Archery;
/// Wrestling — the bare-hands weapon skill, and the defender's fallback.
pub const WRESTLING_SKILL: Skill = Skill::Wrestling;
/// Swordsmanship.
pub const SWORDS_SKILL: Skill = Skill::Swords;
/// Lumberjacking — lends an axe a damage bonus.
pub const LUMBERJACKING_SKILL: Skill = Skill::Lumberjacking;

/// The weapon skill a mobile fights with: its wielded weapon's, or Wrestling for
/// bare hands (or anything not in the table). The skill the to-hit roll and the
/// swing gain both key on.
#[must_use]
pub fn combat_skill_id(state: &WorldState, mobile: EntityId) -> Skill {
    equipped_weapon(state, mobile).map_or(WRESTLING_SKILL, |weapon| weapon.skill.skill())
}

/// The weapon `mobile` wields, if any — the item on a weapon layer. The
/// one-handed slot takes priority when both are occupied, so a weapon and a
/// shield work naturally and a shard-issued secondary item cannot make combat
/// depend on registry iteration order. Its stats are the core table's for the
/// item's graphic, unless the item carries a [`Weapon`] override (a shard's
/// magic sword), which replaces speed and damage while keeping the graphic's
/// skill. Read fresh each swing (no mirror on the mobile), so a weapon coming
/// off reverts the bearer to wrestling with nothing to undo.
#[must_use]
pub fn equipped_weapon(state: &WorldState, mobile: EntityId) -> Option<WeaponData> {
    let item = equipped_weapon_item(state, mobile)?;
    let base = state
        .registry
        .get::<Drawn>(item)
        .and_then(|graphic| weapon_data(graphic.id))
        .copied();
    match state.registry.get::<Weapon>(item) {
        // An override stands the item's stats up, keeping the base graphic's skill
        // (a magic longsword is still a Swords weapon); same numbers either era.
        Some(&Weapon { speed, min, max }) => Some(WeaponData {
            graphic: Graphic(0),
            skill: base.map_or(WeaponSkill::Wrestling, |weapon| weapon.skill),
            kind: base.map_or(WeaponKind::Bashing, |weapon| weapon.kind),
            old_speed: speed,
            old_min: min,
            old_max: max,
            aos_speed: speed,
            aos_min: min,
            aos_max: max,
            // ML speed in hundredths-of-a-second is a different unit from the swing
            // base; lacking a better value, mirror the base's, and inherit the
            // graphic's miss sound and axe flag.
            ml_speed: base.map_or(speed, |weapon| weapon.ml_speed),
            miss_sound: base.map_or(0, |weapon| weapon.miss_sound),
            is_axe: base.is_some_and(|weapon| weapon.is_axe),
            hands: base.and_then(|weapon| weapon.hands),
        }),
        None => base,
    }
}

/// The item a mobile currently wields, if either weapon layer holds one.
///
/// This is the entity counterpart to [`equipped_weapon`]. Custom properties
/// belong to the item instance, while the ordinary weapon table answers the
/// graphic's class, so combat needs both answers without mirroring either onto
/// the mobile.
#[must_use]
pub fn equipped_weapon_item(state: &WorldState, mobile: EntityId) -> Option<EntityId> {
    let serial = state.registry.serial_of(mobile)?;
    [LAYER_ONE_HANDED, LAYER_TWO_HANDED]
        .into_iter()
        .find_map(|layer| {
            openshard_state::equipped_items(state, serial)
                .find(|(_, worn)| worn.layer == layer)
                .map(|(entity, _)| entity)
        })
}

#[cfg(test)]
mod tests {
    use openshard_config::CombatEra;
    use openshard_protocol::wire::Graphic;
    use openshard_state::weapon::{swing_base, weapon_data};

    #[test]
    fn the_se_and_ml_formulas_match_their_arithmetic() {
        // Era 3 (SE), longsword aos_speed 30, dex 100, scale 80000:
        // 80000/((100+100)·30) - 2 = 13 - 2 = 11 ticks → 11·10/4 = 27 tenths → 54.
        assert_eq!(crate::swing_ticks(100, 30, 3, 80000), 54);
        // Era 4 (ML), longsword ml_speed 350 (3.50s), dex 100 (scale ignored):
        // 350·4/100 - 100/30 = 14 - 3 = 11 ticks → 27 tenths → 54.
        assert_eq!(crate::swing_ticks(100, 350, 4, 0), 54);
        // Era 0 floors at 5 tenths where pre-AoS (era 1) would go faster.
        assert_eq!(crate::swing_ticks(255, 255, 0, 15000), 10); // 5 tenths ·2
        assert!(crate::swing_ticks(255, 255, 1, 15000) < 10);
        // And the speed those formulas are fed comes from the era's own column.
        let sword = weapon_data(Graphic(0x0F61)).expect("longsword");
        assert_eq!(swing_base(sword, CombatEra::new(3)), 30);
        assert_eq!(swing_base(sword, CombatEra::new(4)), 350);
    }
}
