//! One recipe, its ingredients, and the material a shard of them can be made of.
//!
//! ServUO's `CraftItem` (the data half — the execution half is [`crate::craft`]),
//! `CraftRes` and `CraftSubRes`.
//!
//! **The material axis is a hue swap here, and in ServUO it is a type swap.** That
//! is not a shortcut: an ingot is an ingot is `0x1BF2`, and valorite differs from
//! iron only by `0x08AB`, which is exactly what [`openshard_state::harvest`]
//! already relies on to tell one ore from another. ServUO needs nine classes
//! because a C# item *is* its class; this engine's items are a graphic and a hue,
//! so the nine rows of `AddSubRes` collapse to nine hues against one graphic and
//! the substitution in [`crate::consume`] is a single field. Boards and leather
//! grades work the same way.

use openshard_protocol::item_kind::{
    ItemKindId,
    ItemSelector,
    MaterialId,
};
use openshard_protocol::wire::{
    Graphic,
    Hue,
};
use openshard_state::{
    AddonKind,
    Skill,
};

use crate::system::{
    Needs,
    Text,
};

/// One ingredient line — ServUO's `CraftRes`.
#[derive(Clone, Copy, Debug)]
pub struct CraftRes {
    /// The semantic thing this line accepts, when its row has been migrated.
    /// The graphic and hue remain as a classic-client/migration projection until
    /// the whole catalogue is typed; they do not decide what a selector means.
    pub selector:  Option<ItemSelector>,
    /// The item art consumed.
    pub graphic:   Graphic,
    /// And its hue. Zero is the plain grade (iron, regular wood); a line marked
    /// [`from_axis`](Self::from_axis) has this overridden by whatever material the
    /// player picked in the gump.
    pub hue:       Hue,
    /// How many, per craft.
    pub amount:    u16,
    /// What it is called, for the detail page.
    pub name:      Text,
    /// What is said when there are not enough — "You do not have sufficient metal
    /// to make that."
    pub message:   Text,
    /// Whether this is the line the system's material axis substitutes into. Only
    /// one line of a recipe ever is.
    pub from_axis: bool,
}

/// One skill a recipe demands, and the band it is rolled over — ServUO's
/// `CraftSkill`. Tenths, as [`openshard_state::skill`] keeps them.
#[derive(Clone, Copy, Debug)]
pub struct CraftSkillReq {
    /// Which skill.
    pub skill: Skill,
    /// The bottom of the band: below this, minus the recipe's offset, the craft is
    /// refused outright rather than rolled.
    pub min:   i32,
    /// The top, at which it always succeeds.
    pub max:   i32,
}

/// One thing a system can make.
#[derive(Clone, Copy, Debug)]
pub struct Recipe {
    /// The semantic thing the recipe produces. `None` keeps an unaudited legacy
    /// row on its graphic/hue adapter until the item catalogue gives it a stable
    /// identity.
    pub kind:               Option<ItemKindId>,
    /// The house addon this generic-looking deed installs, if any.
    pub addon:              Option<AddonKind>,
    /// The item art it produces.
    pub graphic:            Graphic,
    /// Its name, for the gump.
    pub name:               Text,
    /// Which of the system's [`groups`](crate::system::CraftSystemDef::groups) it
    /// files under.
    pub group:              u16,
    /// The skills wanted. The first is the system's main skill and is the one the
    /// success chance is interpolated over; the rest are gates that also train.
    pub skills:             &'static [CraftSkillReq],
    /// What it eats.
    pub resources:          &'static [CraftRes],
    /// How many come out of one paid unit of a recipe.
    ///
    /// **Every shipped row is 1 today**, including fletching: arrows, bolts,
    /// shafts and boards use [`use_all_res`](Self::use_all_res) to make one item
    /// per affordable set of inputs. The column stays because a custom recipe
    /// can make several items from each paid set without widening the craft
    /// path.
    pub amount:             u16,
    /// A fixed hue for the result — ServUO's `SetItemHue`. Zero defers to
    /// [`retain_color`](Self::retain_color), which is what makes a valorite blade
    /// valorite-coloured while special wood still produces ordinary shafts.
    pub hue:                Hue,
    /// Where a typed result gets its material. This is deliberately separate
    /// from its displayed hue: presentation is derived from `kind + material`,
    /// never used to discover either one.
    pub output_material:    OutputMaterial,
    /// Whether a result with no fixed [`hue`](Self::hue) inherits the material
    /// hue. Weapons do; resources such as shafts and kindling deliberately do
    /// not, even when made from one of the special woods.
    pub retain_color:       bool,
    /// Whether the craft consumes every material in the pack and makes as many as
    /// it can — ServUO's `SetUseAllRes`, which is how a hundred logs become a
    /// hundred boards in one click.
    pub use_all_res:        bool,
    /// Tenths knocked off every skill's `min` for the "can you attempt this at
    /// all" gate — ServUO's `SetMinSkillOffset`.
    pub min_skill_offset:   i32,
    /// This recipe's own floor for [`chance::chance`](crate::chance::chance)'s
    /// interpolation, in per-mille, overriding the system's
    /// [`CraftSystemDef::chance_at_min`](crate::system::CraftSystemDef::chance_at_min)
    /// — ServUO's `CraftSystem.GetChanceAtMin(CraftItem)`, which most systems
    /// answer with their own constant but a handful special-case by recipe
    /// (Cooking's `GrapesOfWrath`/`EnchantedApple` start at 50% rather than the
    /// trade's 0%). `None` for every recipe that does not.
    pub min_chance:         Option<u32>,
    /// Mana the crafter pays on top of the materials — ServUO's `SetManaReq`,
    /// which only Inscription's scroll rows set: writing a spell down costs what
    /// the spell's own circle costs to cast.
    ///
    /// Zero for every other row, and zero is "no requirement" rather than "free":
    /// the check is skipped entirely, so a mobile with no [`Mana`] component at
    /// all — a creature, an NPC scribe — is not refused an ordinary craft.
    ///
    /// [`Mana`]: openshard_state::components::Mana
    pub mana:               u16,
    /// Whether an exceptional one carries its maker's name — ServUO's
    /// `CraftItem.IsMarkable`, which is a list of base classes (armour, weapons,
    /// clothing, jewellery, tools, instruments) and so is data here rather than a
    /// rule. A potion has no room for a signature.
    pub markable:           bool,
    /// Never exceptional, whatever the roll — ServUO's `ForceNonExceptional`.
    pub never_exceptional:  bool,
    /// Always exceptional — `ForceExceptional`.
    pub always_exceptional: bool,
    /// What has to be standing nearby, on top of the system's own.
    pub needs:              Needs,
}

/// The material policy of a semantic crafting result.
///
/// `Legacy` is the migration seam for old recipe rows. A typed row must state
/// one of the other variants in data, so adding a kind cannot silently preserve
/// a hue-based material choice by accident.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputMaterial {
    /// No semantic output exists yet; use the recipe's legacy hue behavior.
    Legacy,
    /// The output has a kind but no material (for example an ordinary tool).
    None,
    /// The output is made from one fixed material.
    Fixed(MaterialId),
    /// Take the material carried by a resolved input line.
    InheritInput(u8),
}

/// One material a system's axis offers — ServUO's `CraftSubRes`.
#[derive(Clone, Copy, Debug)]
pub struct SubRes {
    /// The durable material grade selected by this row. Hue is only the
    /// classic-client projection of this id.
    pub material:  MaterialId,
    /// The hue that *is* this material.
    pub hue:       Hue,
    /// Its name, for the material row of the gump.
    pub name:      Text,
    /// The main skill needed to work it, in tenths, checked against the crafter's
    /// **base** — not the stat-lent value. Valorite wants 99.0 Blacksmithy and no
    /// amount of Strength substitutes for it.
    pub req_skill: i32,
    /// What is said when the crafter is not good enough — "You have no idea how to
    /// work this metal."
    pub message:   Text,
}

/// A system's material axis — ServUO's `CraftSubResCol` plus its `SetSubRes`.
#[derive(Clone, Copy, Debug)]
pub struct SubResAxis {
    /// The semantic resource kind selected by this material axis (ingot, board
    /// or leather). Classic art below is only its client projection.
    pub item_kind: ItemKindId,
    /// The resource graphic the axis substitutes a hue into. The recipe line
    /// carrying [`CraftRes::from_axis`] must name this same graphic.
    pub graphic:   Graphic,
    /// The heading over the material row — "Metal", "Wood".
    pub name:      Text,
    /// The grades, in the order the gump lists them; index 0 is the plain one and
    /// is what a fresh crafter is defaulted to.
    pub entries:   &'static [SubRes],
}
