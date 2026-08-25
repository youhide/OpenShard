//! What a craft *system* is: a skill, a chance floor, a beat, and a workshop.
//!
//! ServUO's `CraftSystem` — the abstract base each `Def*.cs` fills in. The five
//! constants at the top of `DefBlacksmithy` (`base(1, 1, 1.25)`, `GetChanceAtMin`,
//! `ECA`, `CanCraft`, `PlayCraftEffect`) are the whole of it, and they are what
//! this struct holds.
//!
//! Fixed point throughout, like [`openshard_state::harvest`] beside it: skills in
//! tenths, chances in per-mille, every duration a tick count. A craft has to
//! replay.

use openshard_protocol::wire::{ClilocId, SoundId};
use openshard_state::Skill;

use crate::recipe::{Recipe, SubResAxis};

/// A cliloc number, or a literal string where ServUO has no number for it.
///
/// ServUO's `TextDefinition` is an implicit union of `int` and `string`; almost
/// every name, group and error message in the craft tables is the `int` form, so
/// the gump is built out of numbers the client localizes itself and hardly any
/// text travels. The string arm is carried because a handful of rows genuinely
/// use it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Text {
    /// A cliloc number the client looks up.
    Cliloc(ClilocId),
    /// A literal, for the rows that have no number.
    Str(&'static str),
}

/// How a system turns a success chance into an *exceptional* chance — ServUO's
/// `CraftECA`.
///
/// Always a subtraction from the success chance, never an independent roll: a
/// smith who can barely make a thing cannot make a masterpiece of it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Eca {
    /// The default: sixty points off.
    ChanceMinusSixty,
    /// Half, then ten points off — the alchemist's curve.
    FiftyPercentChanceMinusTenPercent,
    /// A sliding offset that narrows as the main skill passes 95.0:
    /// `clamp(0.60 - (skill - 95) * 0.03, 0.45, 0.60)`. Blacksmithy's.
    ChanceMinusSixtyToFourtyFive,
}

/// The workshop a recipe needs standing around it — ServUO's `NeedHeat`,
/// `NeedOven`, `NeedMill`, `NeedWater`, and the forge-and-anvil pair
/// `DefBlacksmithy.CanCraft` demands of every one of its recipes.
///
/// A set of requirements rather than a single enum because a recipe can want two
/// (a smith needs both a forge *and* an anvil), and because the requirement is
/// per *recipe* for four of them and per *system* for the fifth.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct Needs {
    /// A forge — the fire a smith works metal in.
    pub forge: bool,
    /// An anvil to beat it on.
    pub anvil: bool,
    /// Any fire at all, which is a looser test than a forge.
    pub heat: bool,
    /// An oven, for baking.
    pub oven: bool,
    /// A flour mill.
    pub mill: bool,
    /// Water.
    pub water: bool,
}

impl Needs {
    /// Nothing but the tool in hand — the common case.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            forge: false,
            anvil: false,
            heat: false,
            oven: false,
            mill: false,
            water: false,
        }
    }

    /// A smith's shop: the fire and the thing to hammer on.
    #[must_use]
    pub const fn smithy() -> Self {
        Self {
            forge: true,
            anvil: true,
            ..Self::none()
        }
    }

    /// Whether anything is wanted at all, so the common case skips the scan.
    #[must_use]
    pub const fn any(self) -> bool {
        self.forge || self.anvil || self.heat || self.oven || self.mill || self.water
    }

    /// Everything either of these wants — a recipe's needs over its system's.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self {
            forge: self.forge || other.forge,
            anvil: self.anvil || other.anvil,
            heat: self.heat || other.heat,
            oven: self.oven || other.oven,
            mill: self.mill || other.mill,
            water: self.water || other.water,
        }
    }
}

/// One craft system — a skill, its recipes, and the rules they are made under.
///
/// The five that exist are in [`crate::defs`]; a system is named by its index
/// there ([`SystemId`]) so a [`Crafting`](openshard_state::components::Crafting)
/// component can hold one without borrowing.
#[derive(Clone, Copy, Debug)]
pub struct CraftSystemDef {
    /// The skill rolled and trained — `MainSkill`.
    pub skill: Skill,
    /// The title over the gump.
    pub title: Text,
    /// The chance of success at the bottom of a recipe's band, in per-mille.
    /// ServUO's `GetChanceAtMin`: zero for a smith (a recipe you have only just
    /// qualified for always fails at first), five hundred for a tailor.
    pub chance_at_min: u32,
    /// How the exceptional chance falls out of the success chance.
    pub eca: Eca,
    /// Ticks between the beats of one craft — ServUO's `Delay`, 1.25s for all but
    /// one system.
    pub delay_ticks: u64,
    /// The fewest beats a craft takes, and the most; the count is rolled between
    /// them. Both are 1 nearly everywhere, which is what makes a craft feel
    /// instant and a failed one still take a moment.
    pub min_beats: u8,
    /// The other end of the roll.
    pub max_beats: u8,
    /// What the tool makes on each beat — `PlayCraftEffect`. The hammer on the
    /// anvil is `0x2A`.
    pub craft_sound: SoundId,
    /// What has to be standing nearby for *any* of this system's recipes.
    pub needs: Needs,
    /// The cliloc said when [`needs`](Self::needs) is not met — "You must be near
    /// an anvil and a forge to smith items."
    ///
    /// `None` for a system that demands no workshop, which is four of the five:
    /// there is no message because the branch that would say it cannot be
    /// reached. Not `ClilocId(0)` — zero is a number the client would look up,
    /// so "absent" and "message zero" would be the same bits.
    pub needs_message: Option<ClilocId>,
    /// The gump's left-hand column, indexed by [`Recipe::group`].
    pub groups: &'static [Text],
    /// Everything it can make.
    pub recipes: &'static [Recipe],
    /// The material axis, where it has one: the nine metals, the seven woods, the
    /// leather grades. A system without one (Alchemy, Tinkering) has `None` and
    /// its gump draws no material row.
    pub sub_res: Option<SubResAxis>,
}

/// Which of the systems in [`crate::defs`], by index.
///
/// An index rather than a reference so it fits in a component and a save record.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct SystemId(u8);

impl SystemId {
    /// Name a system by its stored byte.
    #[must_use]
    pub const fn new(raw: u8) -> Self {
        Self(raw)
    }

    /// The byte kept in the in-flight crafting component and save record.
    #[must_use]
    pub const fn raw(self) -> u8 {
        self.0
    }

    /// The position in [`crate::defs`] this id names.
    #[must_use]
    pub const fn index(self) -> usize {
        self.0 as usize
    }

    /// Name a system by a collection index, if it fits the stored byte.
    #[must_use]
    pub fn from_index(index: usize) -> Option<Self> {
        u8::try_from(index).ok().map(Self)
    }
}
