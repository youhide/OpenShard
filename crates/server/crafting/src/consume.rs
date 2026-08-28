//! The materials: whether they are there, and taking them.
//!
//! ServUO's `CraftItem.ConsumeRes`, whose `ConsumeType.None` pass is a **dry
//! run** — it answers "could this be made" without taking anything, and the real
//! pass runs later against the same recipe. That split is kept here as
//! [`check`] and [`take`], and it is load-bearing rather than tidy: a craft is
//! checked when it is begun, checked again when it finishes seconds later, and
//! only *then* consumed, because a player can hand their ingots to a friend
//! while the hammer is in the air.

use openshard_entities::EntityId;
use openshard_skills::skill_value;
use openshard_state::WorldState;

use crate::recipe::Recipe;
use crate::system::{CraftSystemDef, Text};
use openshard_protocol::wire::{Graphic, Hue};

/// Why a craft cannot go ahead for want of materials.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Refusal {
    /// No backpack at all — "You don't have a backpack".
    NoPack,
    /// Not enough of something, said in that resource line's own words.
    NotEnough(Text),
    /// The material was picked but the crafter cannot work it — "You have no
    /// idea how to work this metal."
    CannotWork(Text),
}

/// What one craft will actually eat, resolved against the crafter's pack.
#[derive(Clone, Debug)]
pub struct Materials {
    /// Each line as `(graphic, hue, per-craft amount)`, with the material axis
    /// already substituted into whichever line takes it.
    pub lines: Vec<(Graphic, Option<Hue>, u16)>,
    /// How many items this craft will make. One, unless the recipe consumes
    /// everything in the pack — then it is as many as the scarcest line allows.
    pub max_amount: u16,
    /// The hue the finished item takes, where the recipe does not fix one: the
    /// colour of the material it was made from, which is what makes a valorite
    /// blade valorite-coloured.
    pub res_hue: Hue,
}

/// How much of the materials a resolved craft spends.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Share {
    /// All of it — a success, or an ordinary failure.
    All,
    /// Half — ServUO's `ConsumeType.Half`, which a failed `use_all_res` craft
    /// pays instead of losing a whole pack of logs to one bad roll.
    Half,
}

/// The dry run: what this craft would take, or why it cannot be made.
///
/// `sub_res` indexes the system's material axis; it is ignored by a system that
/// has none.
pub fn check(
    state: &WorldState,
    crafter: EntityId,
    system: &CraftSystemDef,
    recipe: &Recipe,
    sub_res: usize,
) -> Result<Materials, Refusal> {
    let Some(pack) = state
        .registry
        .serial_of(crafter)
        .filter(|serial| openshard_items::backpack_of(state, *serial).is_some())
    else {
        return Err(Refusal::NoPack);
    };

    // Which material the axis is set to, and whether the crafter can work it.
    // ServUO checks this against the **base** skill, not the stat-lent value: no
    // amount of Strength teaches a smith what to do with valorite.
    let mut res_hue = Hue(0);
    let mut axis_hue = None;
    // The selected material belongs only to recipes whose primary resource
    // comes from the axis. A fletcher who has oak selected still makes ordinary
    // arrows from shafts and feathers, and does not need the skill to work oak
    // merely to assemble ammunition.
    let uses_axis = recipe.resources.iter().any(|res| res.from_axis);
    if let Some(axis) = system.sub_res.filter(|_| uses_axis) {
        let entry = axis.entries.get(sub_res).or_else(|| axis.entries.first());
        if let Some(entry) = entry {
            if i32::from(skill_value(state, crafter, system.skill)) < entry.req_skill {
                return Err(Refusal::CannotWork(entry.message));
            }
            res_hue = entry.hue;
            axis_hue = Some(entry.hue);
        }
    }

    let mut lines = Vec::with_capacity(recipe.resources.len());
    // How many whole crafts the pack can pay for, for a `use_all_res` recipe.
    let mut affordable = u16::MAX;
    for res in recipe.resources {
        // A line marked `from_axis` is the one the material selection substitutes
        // into; every other line is its own graphic at its own hue.
        let hue = if res.from_axis {
            axis_hue.or(Some(res.hue))
        } else {
            Some(res.hue)
        };
        let held = openshard_items::carried_amount_of_hue(state, pack, res.graphic, hue);
        if res.amount == 0 {
            continue;
        }
        if held < u32::from(res.amount) {
            return Err(Refusal::NotEnough(res.message));
        }
        let whole = u16::try_from(held / u32::from(res.amount)).unwrap_or(u16::MAX);
        affordable = affordable.min(whole);
        lines.push((res.graphic, hue, res.amount));
    }

    // A recipe with no materials at all can always be made once; `affordable`
    // would otherwise still be `u16::MAX` and claim a pack full of nothing.
    let max_amount = if recipe.use_all_res && !lines.is_empty() {
        affordable.max(1)
    } else {
        1
    };
    Ok(Materials {
        lines,
        max_amount,
        res_hue,
    })
}

/// Take what [`check`] resolved. Returns whether every line came out whole.
///
/// Each line is all-or-nothing through `items`' own door, so a craft that finds
/// itself short between the check and the take removes nothing on that line
/// rather than eating part of it.
pub fn take(state: &mut WorldState, crafter: EntityId, materials: &Materials, share: Share) -> bool {
    let Some(pack) = state.registry.serial_of(crafter) else {
        return false;
    };
    let mut whole = true;
    for (graphic, hue, per_craft) in &materials.lines {
        let wanted = match share {
            Share::All => u32::from(*per_craft) * u32::from(materials.max_amount),
            // Half is **not** multiplied out by the batch size: a failed run of a
            // hundred boards costs half of *one* craft's logs, not fifty crafts'.
            // ServUO floors it at one, so a bad roll is never free.
            Share::Half => u32::from(*per_craft / 2).max(1),
        };
        let Ok(wanted) = u16::try_from(wanted) else {
            whole = false;
            continue;
        };
        if wanted == 0 {
            continue;
        }
        if openshard_items::take_from_backpack_of_hue(state, pack, *graphic, *hue, wanted) == 0 {
            whole = false;
        }
    }
    whole
}
