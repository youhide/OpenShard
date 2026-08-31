//! Weapon properties — the speed, damage and kind a weapon class carries.
//!
//! These numbers are **not** in `tiledata.mul`: the client and both reference
//! emulators keep them per weapon *class*, not per tile. So they live here, a core
//! table keyed by item graphic for legacy art and by `ItemKindId` for registered
//! kinds, ported from ServUO's `BaseWeapon` subclasses — the same "data keyed by
//! graphic, default in core" shape as
//! [`creature_name`](crate::components::creature_name).
//!
//! Data, in `state`, for the same reason [`crate::title`] is: two crates read it.
//! `combat` turns a row into a swing pace and a damage roll; `skills` reads the
//! same row to tell an Arms Lore student what they are holding. The *rules* —
//! which era's column applies, what the wielder's dexterity does to it, what a
//! blow loses to armour — stay in `combat`, and nothing here knows a fight is
//! happening.
//!
//! Two number sets per weapon, because the engine runs two eras: ServUO's `Old*`
//! (pre-AoS, combat era 1) and `Aos*` (era 2), plus ML's own speed column.
//! [`by_era`] and [`swing_base`] pick between them.
//!
//! Whether a weapon takes both hands is mostly **not** here, because it is in
//! `tiledata.mul` — the quality byte, which ServUO reads straight into `Layer`
//! (`BaseWeapon`: `Layer = (Layer)ItemData.Quality`) — so it comes from the
//! client's own table through `Terrain::item_layer`. Six of ServUO's classic
//! weapons override that byte in code, and only those six carry a [`WeaponData::hands`];
//! see [`weapon_layer`].

use openshard_config::CombatEra;
use openshard_protocol::item_kind::ItemKindId;
use openshard_protocol::wire::{
    Graphic,
    Layer,
    SoundId,
};
use openshard_protocol::world::RangedRange;

use crate::Skill;

/// The paperdoll layer a one-handed weapon sits on (UO layer 1).
pub const LAYER_ONE_HANDED: Layer = Layer(1);
/// The paperdoll layer a two-handed weapon or shield sits on (UO layer 2).
pub const LAYER_TWO_HANDED: Layer = Layer(2);

/// Which combat skill a weapon trains and hits with — ServUO's `DefSkill`.
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
    /// The skill this weapon trains and rolls its to-hit against.
    ///
    /// The mapping is kept in [`Skill`] rather than written out as raw ids,
    /// because five of the eight were **wrong** while they were hand-written
    /// constants. The ids belong to the client, so they come from the client's
    /// own table, and callers keep the domain type until a wire boundary.
    #[must_use]
    pub const fn skill(self) -> Skill {
        match self {
            Self::Swords => Skill::Swords,
            Self::Macing => Skill::Macing,
            Self::Fencing => Skill::Fencing,
            Self::Archery => Skill::Archery,
            Self::Wrestling => Skill::Wrestling,
        }
    }
}

/// How a weapon wounds — ServUO's `WeaponType`, verbatim minus `Fists`.
///
/// It is not derivable from the skill column: a war axe is an axe that *bashes*, a
/// dagger a knife that *pierces*, and Arms Lore reads five different blocks of
/// clilocs off exactly this distinction. So it is a column, taken from each
/// class's `Type` getter (or its base class's, where the subclass does not
/// override).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeaponKind {
    /// Swords and knives: it cuts.
    Slashing,
    /// Spears, forks, krysses, daggers: it pierces.
    Piercing,
    /// Maces, hammers, the war axe: it bashes.
    Bashing,
    /// `BaseAxe` — its own kind in ServUO, and its own Arms Lore line.
    Axe,
    /// `BasePoleArm` — bardiche and halberd.
    Polearm,
    /// `BaseStaff` — quarter staff, black staff, gnarled staff, crook.
    Staff,
    /// Bows and crossbows: it is fired.
    Ranged,
}

/// The human animation group a weapon uses for a swing.
///
/// These are ServUO's `WeaponAnimation` values, which are also the group ids in
/// the classic human animation table.  The modern `0xE2` packet uses a compact
/// sub-action instead; [`WeaponAnimation::sub_action`] names that translation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum WeaponAnimation {
    SlashOneHanded = 9,
    PierceOneHanded = 10,
    BashOneHanded = 11,
    BashTwoHanded = 12,
    SlashTwoHanded = 13,
    PierceTwoHanded = 14,
    ShootBow = 18,
    ShootCrossbow = 19,
    Wrestle = 31,
}

impl WeaponAnimation {
    /// The classic human animation group (`0x6E` action id).
    #[must_use]
    pub const fn group(self) -> u16 {
        self as u16
    }

    /// ServUO's `GetNewAnimationAction`, used as the `0xE2` attack sub-action.
    #[must_use]
    pub const fn sub_action(self) -> u16 {
        match self {
            Self::Wrestle => 0,
            Self::ShootBow => 1,
            Self::ShootCrossbow => 2,
            Self::BashOneHanded => 3,
            Self::SlashOneHanded => 4,
            Self::PierceOneHanded => 5,
            Self::BashTwoHanded => 6,
            Self::SlashTwoHanded => 7,
            Self::PierceTwoHanded => 8,
        }
    }

    /// Frames in this human attack group.
    ///
    /// This is the same mapping the client uses for modern action sub-types.
    /// Keeping it beside the weapon motion lets the server open the animation
    /// window exactly far enough ahead of the authoritative impact tick.
    #[must_use]
    pub const fn frame_count(self) -> u16 {
        match self {
            Self::BashOneHanded | Self::BashTwoHanded => 5,
            Self::SlashTwoHanded => 6,
            Self::SlashOneHanded
            | Self::PierceOneHanded
            | Self::PierceTwoHanded
            | Self::ShootBow
            | Self::ShootCrossbow
            | Self::Wrestle => 7,
        }
    }
}

/// One weapon's combat numbers, keyed by its item [`Drawn`](crate::Drawn) id.
#[derive(Debug, Clone, Copy)]
pub struct WeaponData {
    /// The durable item kind for a registered weapon. Most legacy rows have no
    /// registry entry yet and remain reachable through [`weapon_data`].
    pub item_kind:  Option<ItemKindId>,
    /// The item graphic this row describes.
    pub graphic:    Graphic,
    /// The skill it trains and strikes with.
    pub skill:      WeaponSkill,
    /// How it wounds — which family of Arms Lore lines describes it.
    pub kind:       WeaponKind,
    /// Pre-AoS (era 1) speed constant — the `base` in Sphere's swing formula.
    pub old_speed:  u16,
    /// Pre-AoS minimum damage, before resistance.
    pub old_min:    u16,
    /// Pre-AoS maximum damage, before resistance.
    pub old_max:    u16,
    /// AoS (era 2) speed constant.
    pub aos_speed:  u16,
    /// AoS minimum damage.
    pub aos_min:    u16,
    /// AoS maximum damage.
    pub aos_max:    u16,
    /// ML (era 4) swing speed, in hundredths of a second (ServUO's `MlSpeed`).
    pub ml_speed:   u16,
    /// The sound a whiff makes — ServUO's `DefMissSound` for this weapon class.
    pub miss_sound: SoundId,
    /// Whether Lumberjacking lends this weapon a damage bonus (an axe).
    pub is_axe:     bool,
    /// The ammunition this weapon fires, one graphic per shot. `None` for every
    /// melee row — a melee weapon has no ammunition concept at all, which is the
    /// case `Option` exists for, not "unknown."
    pub ammo:       Option<Graphic>,
    /// The graphic the shot itself is drawn with while it crosses the gap —
    /// ServUO's per-weapon `EffectID` (the bow fires `0x0F42`, both crossbows fire
    /// `0x1BFE`). `None` for melee, matching [`ammo`](Self::ammo).
    pub effect_art: Option<Graphic>,
    /// How far this weapon reaches — ServUO's `DefMaxRange` (the bow's ten tiles,
    /// eight for both crossbows: a bow outranges a crossbow, so this is not one
    /// shared constant). `None` for melee, matching [`ammo`](Self::ammo).
    pub range:      Option<RangedRange>,
    /// The paperdoll layer this weapon class insists on, where it does not trust
    /// `tiledata.mul` — `None` for the great majority, which take the client's
    /// byte.
    ///
    /// Six of ServUO's classic weapons set `Layer` in their own constructor rather
    /// than inheriting `(Layer)ItemData.Quality`, and every one of them is a case
    /// where the file is simply wrong: a real `tiledata.mul` files the bow, the
    /// crossbow, the heavy crossbow, the battle axe and the war hammer as
    /// one-handed. Everyone knows you do not fire a bow one-handed, so ServUO says
    /// so in code and so does this. Read through [`weapon_layer`], never directly.
    pub hands:      Option<Layer>,
}

/// Pick the era-appropriate damage value: the AoS family (eras 2 AoS, 3 SE, 4 ML)
/// uses the `aos` numbers, the pre-AoS family (eras 0 custom, 1 pre-AoS) the `old`.
#[must_use]
pub const fn by_era(old: u16, aos: u16, era: CombatEra) -> u16 {
    if era.value() >= 2 { aos } else { old }
}

/// The swing-speed base a weapon lends under each era's formula: `ml_speed` for ML
/// (era 4), `aos_speed` for AoS/SE (2, 3), `old_speed` for the pre-AoS family
/// (0, 1).
#[must_use]
pub const fn swing_base(weapon: &WeaponData, era: CombatEra) -> u16 {
    match era.value() {
        4 => weapon.ml_speed,
        2 | 3 => weapon.aos_speed,
        _ => weapon.old_speed,
    }
}

/// The weapon row for an item graphic, or `None` for anything not a known weapon
/// (a torch, a spellbook, a shield, bare hands).
#[must_use]
pub fn weapon_data(graphic: Graphic) -> Option<&'static WeaponData> {
    WEAPONS.iter().find(|w| w.graphic == graphic)
}

/// The weapon row for a registered item kind.
///
/// A present kind which is not in this semantic column is deliberately not
/// reinterpreted from its displayed art.
#[must_use]
pub fn weapon_data_for_kind(kind: ItemKindId) -> Option<&'static WeaponData> {
    WEAPONS.iter().find(|weapon| weapon.item_kind == Some(kind))
}

/// Which human swing a known weapon uses while worn on `layer`.
///
/// The family follows ServUO's weapon base classes.  The worn layer settles the
/// one/two-handed split for bashing and piercing weapons; axes and polearms use
/// their characteristic wide two-handed slash even where an axe is physically
/// held in one hand.  Pickaxes are ServUO's explicit one-handed exception.
#[must_use]
pub const fn weapon_animation(weapon: &WeaponData, layer: Layer) -> WeaponAnimation {
    match weapon.kind {
        WeaponKind::Slashing => WeaponAnimation::SlashOneHanded,
        WeaponKind::Piercing => {
            if layer.0 == LAYER_TWO_HANDED.0 {
                WeaponAnimation::PierceTwoHanded
            } else {
                WeaponAnimation::PierceOneHanded
            }
        }
        WeaponKind::Bashing => {
            if layer.0 == LAYER_TWO_HANDED.0 {
                WeaponAnimation::BashTwoHanded
            } else {
                WeaponAnimation::BashOneHanded
            }
        }
        WeaponKind::Axe => {
            if weapon.graphic.0 == 0x0E86 {
                WeaponAnimation::SlashOneHanded // Pickaxe
            } else {
                WeaponAnimation::SlashTwoHanded
            }
        }
        WeaponKind::Polearm => WeaponAnimation::SlashTwoHanded,
        WeaponKind::Staff => WeaponAnimation::BashTwoHanded,
        WeaponKind::Ranged => {
            if weapon.graphic.0 == 0x13B2 {
                WeaponAnimation::ShootBow
            } else {
                WeaponAnimation::ShootCrossbow
            }
        }
    }
}

/// Which hand (or hands) a weapon is held in: the class's own answer where it has
/// one, else `tiledata_layer` — what `Terrain::item_layer` read out of the client's
/// file. Compare against [`LAYER_TWO_HANDED`].
///
/// A shard with no client files passes `0` here and gets `0` back for every weapon
/// but the six, which is the honest answer: without tiledata the engine does not
/// know, and it says so rather than guessing one-handed.
#[must_use]
pub const fn weapon_layer(weapon: &WeaponData, tiledata_layer: Layer) -> Layer {
    match weapon.hands {
        Some(layer) => layer,
        None => tiledata_layer,
    }
}

/// The classic pre-AoS weapon set, ported from
/// `ServUO/Scripts/Items/Equipment/Weapons/*.cs` — each row's graphic from the
/// constructor's `: base(0x…)`, its `Old*`/`Aos*` from the subclass getters, its
/// skill from the base class's `DefSkill` (BaseSword/Axe/Knife/PoleArm → Swords,
/// BaseBashing/Staff → Macing, BaseSpear → Fencing, BaseRanged → Archery), and its
/// kind from the class's own `Type`. Kryss is Fencing here: ServUO files it under
/// `BaseSword`, but classic UO trains it with Fencing, and the
/// numbers-taken/arithmetic-audited rule favours the client's truth.
// Columns after the AoS block: `ml` ML-speed (hundredths of a second), `miss` the
// weapon-class miss sound, `axe` the Lumberjacking flag. A trailing `.hands` is set
// only on the six rows whose class overrides tiledata's layer byte.
#[rustfmt::skip]
static WEAPONS: &[WeaponData] = &[
    // ------------------------  skill kind   spd  min  max  aos speeds     ml  miss   axe
    with_item_kind(w(0x0F61, SWORDS, SLASHING, 35, 5, 33, 30, 14, 18, 350, SoundId(0x23A), false), ItemKindId(4)), // Longsword
    with_item_kind(w(0x0F5E, SWORDS, SLASHING, 45, 5, 29, 33, 13, 17, 325, SoundId(0x23A), false), ItemKindId(66)), // Broadsword
    with_item_kind(w(0x13FF, SWORDS, SLASHING, 58, 5, 26, 46, 10, 14, 250, SoundId(0x23A), false), ItemKindId(69)), // Katana
    with_item_kind(w(0x13B9, SWORDS, SLASHING, 30, 6, 34, 28, 15, 19, 375, SoundId(0x23A), false), ItemKindId(72)), // Viking sword
    with_item_kind(w(0x1441, SWORDS, SLASHING, 45, 6, 28, 44, 10, 14, 250, SoundId(0x23A), false), ItemKindId(67)), // Cutlass
    with_item_kind(w(0x13B6, SWORDS, SLASHING, 43, 4, 30, 37, 12, 16, 300, SoundId(0x23A), false), ItemKindId(71)), // Scimitar
    // -- Knives (BaseKnife; the dagger is the one that pierces) --------------------
    with_item_kind(w(0x0F52, SWORDS, PIERCING, 55, 3, 15, 56, 10, 12, 200, SoundId(0x238), false), ItemKindId(68)), // Dagger
    w(0x13F6, SWORDS,  SLASHING, 40,  2, 14, 49, 10, 13, 225, SoundId(0x238), false), // Butcher knife
    w(0x0EC3, SWORDS,  SLASHING, 40,  2, 13, 46, 10, 14, 250, SoundId(0x238), false), // Cleaver
    w(0x0EC4, SWORDS,  SLASHING, 40,  1, 10, 49, 10, 13, 225, SoundId(0x238), false), // Skinning knife
    // -- Axes (BaseAxe, Swords skill; the war axe bashes) -------------------------
    w(0x0F43, SWORDS,  AXE,      40,  2, 17, 41, 13, 16, 275, SoundId(0x23A), true ), // Hatchet
    with_item_kind(w(0x0F49, SWORDS, AXE, 37, 6, 33, 37, 14, 17, 300, SoundId(0x23A), true), ItemKindId(73)), // Axe
    with_item_kind(both_hands(w(0x0F47, SWORDS, AXE, 30, 6, 38, 31, 16, 19, 350, SoundId(0x23A), true)), ItemKindId(74)), // Battle axe
    with_item_kind(w(0x0F4B, SWORDS, AXE, 37, 5, 35, 33, 15, 18, 325, SoundId(0x23A), true), ItemKindId(75)), // Double axe
    with_item_kind(w(0x0F45, SWORDS, AXE, 37, 6, 33, 33, 15, 18, 325, SoundId(0x23A), true), ItemKindId(76)), // Executioner's axe
    with_item_kind(w(0x13FB, SWORDS, AXE, 30, 6, 38, 29, 17, 20, 375, SoundId(0x23A), true), ItemKindId(77)), // Large battle axe
    with_item_kind(w(0x1443, SWORDS, AXE, 30, 5, 39, 31, 16, 19, 350, SoundId(0x23A), true), ItemKindId(78)), // Two-handed axe
    with_item_kind(w(0x13B0, SWORDS, BASHING, 40, 9, 27, 33, 12, 16, 300, SoundId(0x239), true), ItemKindId(79)), // War axe
    with_item_kind(w(0x0E86, SWORDS, AXE, 35, 1, 15, 35, 12, 16, 300, SoundId(0x23A), true), ItemKindId(9)), // Pickaxe
    // -- Polearms (BasePoleArm, Swords skill) -------------------------------------
    with_item_kind(w(0x0F4D, SWORDS, POLEARM, 26, 5, 43, 28, 17, 20, 375, SoundId(0x238), false), ItemKindId(80)), // Bardiche
    with_item_kind(w(0x143E, SWORDS, POLEARM, 25, 5, 49, 25, 18, 21, 400, SoundId(0x238), false), ItemKindId(81)), // Halberd
    // -- Maces (BaseBashing) & staves (BaseStaff) ---------------------------------
    w(0x13B4, MACING,  BASHING,  40,  8, 24, 44, 10, 14, 250, SoundId(0x239), false), // Club
    with_item_kind(w(0x0F5C, MACING, BASHING, 30, 8, 32, 40, 11, 15, 275, SoundId(0x239), false), ItemKindId(86)), // Mace
    with_item_kind(w(0x143B, MACING, BASHING, 30, 10, 30, 32, 14, 18, 350, SoundId(0x239), false), ItemKindId(87)), // Maul
    with_item_kind(w(0x1407, MACING, BASHING, 32, 10, 30, 26, 16, 20, 400, SoundId(0x239), false), ItemKindId(88)), // War mace
    with_item_kind(both_hands(w(0x1439, MACING, BASHING, 31, 8, 36, 28, 17, 20, 375, SoundId(0x239), false)), ItemKindId(89)), // War hammer
    with_item_kind(one_hand(w(0x143D, MACING, BASHING, 30, 6, 33, 28, 13, 17, 325, SoundId(0x239), false)), ItemKindId(85)), // Hammer pick
    w(0x0E89, MACING,  STAFF,    48,  8, 28, 48, 11, 14, 225, SoundId(0x239), false), // Quarter staff
    w(0x0DF0, MACING,  STAFF,    35,  8, 33, 39, 13, 16, 275, SoundId(0x239), false), // Black staff
    w(0x13F8, MACING,  STAFF,    33, 10, 30, 33, 15, 18, 325, SoundId(0x239), false), // Gnarled staff
    w(0x0E81, MACING,  STAFF,    30,  3, 12, 40, 13, 16, 275, SoundId(0x239), false), // Shepherd's crook
    // -- Fencing (BaseSpear, and the kryss) ---------------------------------------
    with_item_kind(w(0x1401, FENCING, PIERCING, 53, 3, 28, 53, 10, 12, 200, SoundId(0x238), false), ItemKindId(70)), // Kryss
    with_item_kind(w(0x1405, FENCING, PIERCING, 45, 4, 32, 43, 10, 14, 250, SoundId(0x238), false), ItemKindId(84)), // War fork
    with_item_kind(w(0x0F62, FENCING, PIERCING, 46, 2, 36, 42, 13, 16, 275, SoundId(0x238), false), ItemKindId(83)), // Spear
    with_item_kind(w(0x1403, FENCING, PIERCING, 50, 4, 32, 55, 10, 13, 200, SoundId(0x238), false), ItemKindId(82)), // Short spear
    with_item_kind(w(0x0E87, FENCING, PIERCING, 45, 4, 16, 43, 12, 15, 250, SoundId(0x238), false), ItemKindId(90)), // Pitchfork
    // -- Archery (BaseRanged) -----------------------------------------------------
    // Range is ServUO's `DefMaxRange`: the bow outreaches both crossbows.
    with_item_kind(both_hands(ranged(w(0x13B2, ARCHERY, RANGED, 20, 9, 41, 25, 25, 25, 425, SoundId(0x238), false), ARROW, ARROW_EFFECT, 10)), ItemKindId(91)), // Bow
    with_item_kind(both_hands(ranged(w(0x0F50, ARCHERY, RANGED, 18, 8, 43, 24, 18, 24, 450, SoundId(0x238), false), BOLT, BOLT_EFFECT, 8)), ItemKindId(92)), // Crossbow
    with_item_kind(both_hands(ranged(w(0x13FD, ARCHERY, RANGED, 10, 11, 56, 22, 22, 22, 500, SoundId(0x238), false), BOLT, BOLT_EFFECT, 8)), ItemKindId(93)), // Heavy crossbow
];

/// Arrow — ServUO's `Arrow.cs` (`0x0F3F`), what the bow fires and what a shot
/// spends from the shooter's own pack. Public so `combat` can tell an empty
/// quiver from an empty case of bolts when a shot cannot be fired.
pub const ARROW: Graphic = Graphic(0x0F3F);
/// Bolt — ServUO's `Bolt.cs` (`0x1BFB`), what both crossbows fire.
pub const BOLT: Graphic = Graphic(0x1BFB);
/// The bow's shot in flight — ServUO's `Bow.EffectID`.
const ARROW_EFFECT: Graphic = Graphic(0x0F42);
/// Both crossbows' shot in flight — ServUO's `Crossbow`/`HeavyCrossbow.EffectID`.
const BOLT_EFFECT: Graphic = Graphic(0x1BFE);

// Short names for the two enum columns, so the table above stays one weapon per
// readable line. They exist for the table and nothing else.
const SWORDS: WeaponSkill = WeaponSkill::Swords;
const MACING: WeaponSkill = WeaponSkill::Macing;
const FENCING: WeaponSkill = WeaponSkill::Fencing;
const ARCHERY: WeaponSkill = WeaponSkill::Archery;
const SLASHING: WeaponKind = WeaponKind::Slashing;
const PIERCING: WeaponKind = WeaponKind::Piercing;
const BASHING: WeaponKind = WeaponKind::Bashing;
const AXE: WeaponKind = WeaponKind::Axe;
const POLEARM: WeaponKind = WeaponKind::Polearm;
const STAFF: WeaponKind = WeaponKind::Staff;
const RANGED: WeaponKind = WeaponKind::Ranged;

/// Mark a row two-handed whatever `tiledata.mul` says about it — ServUO's
/// `Layer = Layer.TwoHanded` in the class's own constructor.
const fn both_hands(mut weapon: WeaponData) -> WeaponData {
    weapon.hands = Some(LAYER_TWO_HANDED);
    weapon
}

/// Mark a row one-handed whatever tiledata says. Only the hammer pick, which
/// ServUO pins explicitly and tiledata already agrees with — kept because the
/// class *insists*, and a future tiledata that disagrees should not change it.
const fn one_hand(mut weapon: WeaponData) -> WeaponData {
    weapon.hands = Some(LAYER_ONE_HANDED);
    weapon
}

/// Attach the ammunition, flight graphic and reach to a `Ranged`-kind row — the
/// three facts that only apply to a fired weapon, kept out of `w()`'s already-long
/// argument list the way `both_hands`/`one_hand` keep the hand override out of it.
const fn ranged(mut weapon: WeaponData, ammo: Graphic, effect_art: Graphic, range: u8) -> WeaponData {
    weapon.ammo = Some(ammo);
    weapon.effect_art = Some(effect_art);
    weapon.range = RangedRange::new(range);
    weapon
}

/// Attach the registry identity to a legacy combat row. This is deliberately
/// separate from its drawing graphic: semantic readers never round-trip through
/// presentation to obtain combat class.
const fn with_item_kind(mut weapon: WeaponData, item_kind: ItemKindId) -> WeaponData {
    weapon.item_kind = Some(item_kind);
    weapon
}

/// A terse constructor so the table above stays one weapon per readable line.
// Every argument is a distinct weapon field; a struct literal per row would only
// make the table wordier, which is the opposite of the point.
#[allow(clippy::too_many_arguments)]
const fn w(
    graphic: u16,
    skill: WeaponSkill,
    kind: WeaponKind,
    old_speed: u16,
    old_min: u16,
    old_max: u16,
    aos_speed: u16,
    aos_min: u16,
    aos_max: u16,
    ml_speed: u16,
    miss_sound: SoundId,
    is_axe: bool,
) -> WeaponData {
    WeaponData {
        item_kind: None,
        graphic: Graphic(graphic),
        skill,
        kind,
        old_speed,
        old_min,
        old_max,
        aos_speed,
        aos_min,
        aos_max,
        ml_speed,
        miss_sound,
        is_axe,
        hands: None,
        ammo: None,
        effect_art: None,
        range: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_graphic_resolves_and_an_unknown_one_does_not() {
        let sword = weapon_data(Graphic(0x0F61)).expect("longsword is in the table");
        assert_eq!(sword.skill, WeaponSkill::Swords);
        assert_eq!(sword.old_speed, 35);
        assert_eq!((sword.old_min, sword.old_max), (5, 33));
        assert!(weapon_data(Graphic(0x0000)).is_none());
    }

    #[test]
    fn a_registered_longsword_resolves_by_kind() {
        let sword = weapon_data_for_kind(ItemKindId(4)).expect("longsword kind");
        assert_eq!(sword.item_kind, Some(ItemKindId(4)));
        assert_eq!(sword.graphic, Graphic(0x0F61));
        assert!(weapon_data_for_kind(ItemKindId(5)).is_none()); // plate chest
    }

    #[test]
    fn every_registered_weapon_kind_has_its_direct_combat_row() {
        for definition in crate::item_definition::ITEM_DEFINITIONS {
            if !definition
                .tags
                .contains(&openshard_protocol::item_kind::ItemTag::Weapon)
            {
                continue;
            }
            let weapon = weapon_data_for_kind(definition.id)
                .unwrap_or_else(|| panic!("{} has no weapon row", definition.name));
            assert_eq!(weapon.item_kind, Some(definition.id));
        }
    }

    #[test]
    fn axes_swing_the_weapon_instead_of_wrestling() {
        let axe = weapon_data(Graphic(0x0F49)).expect("axe");
        assert_eq!(
            weapon_animation(axe, LAYER_ONE_HANDED),
            WeaponAnimation::SlashTwoHanded
        );
        assert_eq!(weapon_animation(axe, LAYER_ONE_HANDED).group(), 13);
        assert_eq!(weapon_animation(axe, LAYER_ONE_HANDED).sub_action(), 7);

        let war_axe = weapon_data(Graphic(0x13B0)).expect("war axe");
        assert_eq!(
            weapon_animation(war_axe, LAYER_ONE_HANDED),
            WeaponAnimation::BashOneHanded
        );
    }

    #[test]
    fn by_era_splits_the_pre_aos_and_aos_families() {
        assert_eq!(by_era(35, 30, CombatEra::new(0)), 35); // custom → pre-AoS numbers
        assert_eq!(by_era(35, 30, CombatEra::new(1)), 35); // pre-AoS
        assert_eq!(by_era(35, 30, CombatEra::new(2)), 30); // AoS
        assert_eq!(by_era(35, 30, CombatEra::new(3)), 30); // SE → AoS family
        assert_eq!(by_era(35, 30, CombatEra::new(4)), 30); // ML → AoS family
    }

    #[test]
    fn swing_base_picks_the_eras_speed_column() {
        let sword = weapon_data(Graphic(0x0F61)).unwrap(); // old 35, aos 30, ml 350
        assert_eq!(swing_base(sword, CombatEra::new(0)), 35);
        assert_eq!(swing_base(sword, CombatEra::new(1)), 35);
        assert_eq!(swing_base(sword, CombatEra::new(2)), 30);
        assert_eq!(swing_base(sword, CombatEra::new(3)), 30);
        assert_eq!(swing_base(sword, CombatEra::new(4)), 350);
    }

    #[test]
    fn no_two_weapons_share_a_graphic() {
        for (i, a) in WEAPONS.iter().enumerate() {
            for b in &WEAPONS[i + 1..] {
                assert_ne!(a.graphic, b.graphic, "duplicate graphic 0x{:04X}", a.graphic.0);
            }
        }
    }

    #[test]
    fn no_two_weapons_claim_one_registered_kind() {
        for (index, weapon) in WEAPONS.iter().enumerate() {
            let Some(kind) = weapon.item_kind else {
                continue;
            };
            assert!(
                WEAPONS[index + 1..]
                    .iter()
                    .all(|other| other.item_kind != Some(kind)),
                "duplicate weapon kind {}",
                kind.0
            );
        }
    }

    #[test]
    fn the_shared_catalogue_filter_matches_the_gameplay_table() {
        for raw in u16::MIN..=u16::MAX {
            let graphic = Graphic(raw);
            assert_eq!(
                openshard_protocol::items::is_classic_weapon(graphic),
                weapon_data(graphic).is_some(),
                "0x{raw:04X}"
            );
        }
    }

    #[test]
    fn a_weapons_kind_is_its_servuo_class_and_not_its_skill() {
        let kind = |graphic: u16| weapon_data(Graphic(graphic)).expect("in the table").kind;
        assert_eq!(kind(0x0F61), WeaponKind::Slashing); // longsword, BaseSword
        assert_eq!(kind(0x0F49), WeaponKind::Axe); // axe, BaseAxe
        assert_eq!(kind(0x13B0), WeaponKind::Bashing); // war axe: an axe that bashes
        assert_eq!(kind(0x0F52), WeaponKind::Piercing); // dagger: a knife that pierces
        assert_eq!(kind(0x13F6), WeaponKind::Slashing); // butcher knife, BaseKnife
        assert_eq!(kind(0x143E), WeaponKind::Polearm); // halberd
        assert_eq!(kind(0x0E89), WeaponKind::Staff); // quarter staff
        assert_eq!(kind(0x0F5C), WeaponKind::Bashing); // mace
        assert_eq!(kind(0x1401), WeaponKind::Piercing); // kryss
        assert_eq!(kind(0x13B2), WeaponKind::Ranged); // bow
    }

    #[test]
    fn only_the_ranged_rows_carry_ammo_a_flight_graphic_and_a_reach() {
        let row = |graphic: u16| weapon_data(Graphic(graphic)).expect("in the table");
        let bow = row(0x13B2);
        assert_eq!(bow.ammo, Some(Graphic(0x0F3F)), "bow fires arrows");
        assert_eq!(bow.effect_art, Some(Graphic(0x0F42)), "bow's own flight graphic");
        assert_eq!(
            bow.range.map(RangedRange::get),
            Some(10),
            "bow outreaches a crossbow"
        );
        for graphic in [0x0F50, 0x13FD] {
            // Crossbow, heavy crossbow: both fire bolts, not arrows, and reach less far.
            let crossbow = row(graphic);
            assert_eq!(
                crossbow.ammo,
                Some(Graphic(0x1BFB)),
                "0x{graphic:04X} fires bolts"
            );
            assert_eq!(
                crossbow.effect_art,
                Some(Graphic(0x1BFE)),
                "0x{graphic:04X}'s own flight graphic"
            );
            assert_eq!(
                crossbow.range.map(RangedRange::get),
                Some(8),
                "0x{graphic:04X}'s reach"
            );
        }
        // A melee row has no ammunition concept at all.
        let sword = row(0x0F61);
        assert_eq!(sword.ammo, None);
        assert_eq!(sword.effect_art, None);
        assert_eq!(sword.range, None);
    }

    #[test]
    fn the_six_classes_that_distrust_tiledata_win_over_it() {
        let layer = |graphic: u16, tiledata: Layer| {
            weapon_layer(weapon_data(Graphic(graphic)).expect("in the table"), tiledata)
        };
        // A real `tiledata.mul` files all five of these as one-handed. They are not.
        for graphic in [0x13B2, 0x0F50, 0x13FD, 0x0F47, 0x1439] {
            assert_eq!(
                layer(graphic, LAYER_ONE_HANDED),
                LAYER_TWO_HANDED,
                "0x{graphic:04X} overrides tiledata"
            );
        }
        // The hammer pick pins one-handed even if a file claims otherwise.
        assert_eq!(layer(0x143D, LAYER_TWO_HANDED), LAYER_ONE_HANDED);
        // Everything else takes the client's word, whichever way it reads.
        assert_eq!(layer(0x143E, LAYER_TWO_HANDED), LAYER_TWO_HANDED); // halberd
        assert_eq!(layer(0x13FF, LAYER_ONE_HANDED), LAYER_ONE_HANDED); // katana
        assert_eq!(layer(0x13FF, Layer(0)), Layer(0), "no tiledata, no answer");
    }

    #[test]
    fn a_skills_id_comes_from_the_clients_table() {
        assert_eq!(WeaponSkill::Swords.skill(), Skill::Swords);
        assert_eq!(WeaponSkill::Wrestling.skill(), Skill::Wrestling);
        // The five that were wrong as hand-written constants; pinned so a future
        // edit cannot quietly put Fencing back on Cooking's row.
        assert_eq!(WeaponSkill::Swords.skill().id(), 40);
        assert_eq!(WeaponSkill::Macing.skill().id(), 41);
        assert_eq!(WeaponSkill::Fencing.skill().id(), 42);
        assert_eq!(WeaponSkill::Archery.skill().id(), 31);
    }
}
