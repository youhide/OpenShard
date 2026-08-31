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
use openshard_protocol::item_kind::{ItemSelector, MaterialRule};
use openshard_skills::skill_value;
use openshard_state::components::{ItemKind, Material};
use openshard_state::{WorldState, item_definition, kind_from_drawn, material_definition};
use std::collections::HashMap;

use crate::recipe::Recipe;
use crate::system::{CraftSystemDef, Text};
use openshard_protocol::wire::{Graphic, Hue};
use openshard_state::Drawn;

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
    /// Each resolved ingredient. `semantic` is present when the registry names
    /// this legacy projection; the exact graphic/hue remains only for migration
    /// compatibility and classic presentation.
    pub lines: Vec<MaterialLine>,
    /// How many items this craft will make. One, unless the recipe consumes
    /// everything in the pack — then it is as many as the scarcest line allows.
    pub max_amount: u16,
    /// The hue the finished item takes, where the recipe does not fix one: the
    /// colour of the material it was made from, which is what makes a valorite
    /// blade valorite-coloured.
    pub res_hue: Hue,
}

/// One craft input after material-axis selection.
#[derive(Clone, Copy, Debug)]
pub struct MaterialLine {
    pub graphic: Graphic,
    pub hue: Option<Hue>,
    pub amount: u16,
    pub semantic: Option<(
        openshard_protocol::item_kind::ItemKindId,
        Option<openshard_protocol::item_kind::MaterialId>,
    )>,
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

/// One read of a crafter's backpack, reused by catalogue rows.
///
/// The ordinary craft path asks one recipe question and [`check`] is the right
/// shape for it. The all-recipes catalogue asks the same question hundreds of
/// times. Reading the backpack for every ingredient made that request
/// `recipes × backpack children`; this snapshot makes it one exact indexed
/// backpack read followed by small hash lookups while preserving the exact
/// typed/legacy matching rules used by a real craft.
pub(crate) struct MaterialStock {
    has_pack: bool,
    /// Every drawn item, including typed instances. Used by legacy recipes.
    drawn: HashMap<Drawn, u32>,
    /// Drawn instances without an `ItemKind`. These alone are the compatibility
    /// half of a typed selector.
    legacy: HashMap<Drawn, u32>,
    identity: HashMap<
        (
            openshard_protocol::item_kind::ItemKindId,
            Option<openshard_protocol::item_kind::MaterialId>,
        ),
        u32,
    >,
}

impl MaterialStock {
    pub(crate) fn capture(state: &WorldState, crafter: EntityId) -> Self {
        let Some(pack) = state
            .registry
            .serial_of(crafter)
            .and_then(|serial| openshard_items::backpack_of(state, serial))
        else {
            return Self {
                has_pack: false,
                drawn: HashMap::new(),
                legacy: HashMap::new(),
                identity: HashMap::new(),
            };
        };

        let mut stock = Self {
            has_pack: true,
            drawn: HashMap::new(),
            legacy: HashMap::new(),
            identity: HashMap::new(),
        };
        for (item, _) in openshard_state::contained_items(state, pack) {
            let Some(&drawn) = state.registry.get::<Drawn>(item) else {
                continue;
            };
            let amount = u32::from(openshard_items::amount_of(state, item));
            *stock.drawn.entry(drawn).or_default() += amount;
            match state.registry.get::<ItemKind>(item) {
                Some(&ItemKind(kind)) => {
                    let material = state.registry.get::<Material>(item).map(|material| material.0);
                    *stock.identity.entry((kind, material)).or_default() += amount;
                }
                None => *stock.legacy.entry(drawn).or_default() += amount,
            }
        }
        stock
    }

    fn held(
        &self,
        semantic: Option<(
            openshard_protocol::item_kind::ItemKindId,
            Option<openshard_protocol::item_kind::MaterialId>,
        )>,
        legacy: Drawn,
    ) -> u32 {
        match semantic {
            Some(identity) => {
                self.identity.get(&identity).copied().unwrap_or_default()
                    + self.legacy.get(&legacy).copied().unwrap_or_default()
            }
            None => self.drawn.get(&legacy).copied().unwrap_or_default(),
        }
    }
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

    check_with(
        state,
        crafter,
        system,
        recipe,
        sub_res,
        |semantic, legacy| match semantic {
            Some((kind, material)) => {
                openshard_items::carried_amount_of_identity_or_legacy(state, pack, kind, material, legacy)
            }
            None => openshard_items::carried_amount_of_hue(state, pack, legacy.id, Some(legacy.hue)),
        },
    )
}

/// The catalogue's dry run against its one captured backpack view.
pub(crate) fn check_stock(
    state: &WorldState,
    crafter: EntityId,
    system: &CraftSystemDef,
    recipe: &Recipe,
    sub_res: usize,
    stock: &MaterialStock,
) -> Result<Materials, Refusal> {
    if !stock.has_pack {
        return Err(Refusal::NoPack);
    }
    check_with(state, crafter, system, recipe, sub_res, |semantic, legacy| {
        stock.held(semantic, legacy)
    })
}

fn check_with(
    state: &WorldState,
    crafter: EntityId,
    system: &CraftSystemDef,
    recipe: &Recipe,
    sub_res: usize,
    held: impl Fn(
        Option<(
            openshard_protocol::item_kind::ItemKindId,
            Option<openshard_protocol::item_kind::MaterialId>,
        )>,
        Drawn,
    ) -> u32,
) -> Result<Materials, Refusal> {
    // Which material the axis is set to, and whether the crafter can work it.
    // ServUO checks this against the **base** skill, not the stat-lent value: no
    // amount of Strength teaches a smith what to do with valorite.
    let mut res_hue = Hue(0);
    let mut axis_hue = None;
    let mut axis_material = None;
    let mut axis_identity = None;
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
            axis_material = Some(entry.material);
            axis_identity = Some((axis.item_kind, entry.material));
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
        // A migrated row resolves its declared selector before it ever consults
        // a classic presentation pair. The old rows below retain the audited
        // `kind_from_drawn` bridge while their data is being moved one by one.
        let semantic = match res.selector {
            Some(selector) => resolve_selector(selector, res.from_axis, axis_material),
            None if res.from_axis => axis_identity.map(|(kind, material)| (kind, Some(material))),
            None => hue.and_then(|hue| kind_from_drawn(Drawn { id: res.graphic, hue })),
        };
        let held = held(
            semantic,
            Drawn {
                id: res.graphic,
                hue: hue.expect("a craft ingredient has a resolved hue"),
            },
        );
        if res.amount == 0 {
            continue;
        }
        if held < u32::from(res.amount) {
            return Err(Refusal::NotEnough(res.message));
        }
        let whole = u16::try_from(held / u32::from(res.amount)).unwrap_or(u16::MAX);
        affordable = affordable.min(whole);
        lines.push(MaterialLine {
            graphic: res.graphic,
            hue,
            amount: res.amount,
            semantic,
        });
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

/// Resolve the exact identity a selector-based recipe line consumes today.
///
/// The first migrated smithing row selects a material through the existing
/// gump axis, so `KindWithMaterial { Any }` means *the selected material of
/// this axis*, not an arbitrary pile in the pack. More general tag and
/// cross-input selectors are kept in the protocol vocabulary but are rejected
/// here until their evaluator can count several candidate kinds atomically.
fn resolve_selector(
    selector: ItemSelector,
    from_axis: bool,
    axis_material: Option<openshard_protocol::item_kind::MaterialId>,
) -> Option<(
    openshard_protocol::item_kind::ItemKindId,
    Option<openshard_protocol::item_kind::MaterialId>,
)> {
    match selector {
        ItemSelector::Exact(kind) if !from_axis => Some((kind, None)),
        ItemSelector::KindWithMaterial { kind, material } => match material {
            MaterialRule::Any if from_axis => {
                let family = item_definition(kind)?.material_family?;
                let selected = axis_material?;
                (material_definition(selected)?.family == family).then_some((kind, Some(selected)))
            }
            MaterialRule::Exact(material) => Some((kind, Some(material))),
            MaterialRule::InFamily(family) => {
                let material = axis_material?;
                (material_definition(material)?.family == family).then_some((kind, Some(material)))
            }
            // `SameAsInput` needs the already-resolved line it names; a tag
            // needs a candidate set. Neither may quietly fall back to art.
            MaterialRule::Any | MaterialRule::SameAsInput(_) => None,
        },
        ItemSelector::Tag(_) => None,
        ItemSelector::Exact(_) => None,
    }
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
    for line in &materials.lines {
        let wanted = match share {
            Share::All => u32::from(line.amount) * u32::from(materials.max_amount),
            // Half is **not** multiplied out by the batch size: a failed run of a
            // hundred boards costs half of *one* craft's logs, not fifty crafts'.
            // ServUO floors it at one, so a bad roll is never free.
            Share::Half => u32::from(line.amount / 2).max(1),
        };
        let Ok(wanted) = u16::try_from(wanted) else {
            whole = false;
            continue;
        };
        if wanted == 0 {
            continue;
        }
        let took = match line.semantic {
            Some((kind, material)) => openshard_items::take_from_backpack_identity_or_legacy(
                state,
                pack,
                kind,
                material,
                Drawn {
                    id: line.graphic,
                    hue: line.hue.expect("a semantic ingredient has a resolved hue"),
                },
                wanted,
            ),
            None => openshard_items::take_from_backpack_of_hue(state, pack, line.graphic, line.hue, wanted),
        };
        if took == 0 {
            whole = false;
        }
    }
    whole
}

#[cfg(test)]
mod tests {
    use super::*;
    use openshard_protocol::item_kind::{ItemKindId, MaterialId};

    #[test]
    fn a_material_axis_resolves_using_its_explicit_material_id() {
        let any_material = |kind| ItemSelector::KindWithMaterial {
            kind,
            material: MaterialRule::Any,
        };
        assert_eq!(
            resolve_selector(any_material(ItemKindId(1)), true, Some(MaterialId(1))),
            Some((ItemKindId(1), Some(MaterialId(1)))),
            "ingots use regular iron"
        );
        assert_eq!(
            resolve_selector(any_material(ItemKindId(3)), true, Some(MaterialId(20))),
            Some((ItemKindId(3), Some(MaterialId(20)))),
            "logs use regular wood, not the globally first iron hue"
        );
    }
}
