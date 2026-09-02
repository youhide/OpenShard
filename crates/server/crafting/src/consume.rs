//! The materials: whether they are there, and taking them.
//!
//! ServUO's `CraftItem.ConsumeRes`, whose `ConsumeType.None` pass is a **dry
//! run** — it answers "could this be made" without taking anything, and the real
//! pass runs later against the same recipe. That split is kept here as
//! [`check`] and [`take`], and it is load-bearing rather than tidy: a craft is
//! checked when it is begun, checked again when it finishes seconds later, and
//! only *then* consumed, because a player can hand their ingots to a friend
//! while the hammer is in the air.

use std::collections::BTreeMap;

use openshard_entities::EntityId;
use openshard_protocol::craft::{
    CraftKey,
    craft_key_for,
};
use openshard_protocol::item_kind::{
    ItemSelector,
    MaterialRule,
};
use openshard_protocol::serial::Serial;
use openshard_protocol::wire::{
    Graphic,
    Hue,
};
use openshard_skills::skill_value;
use openshard_state::components::{
    ItemKind,
    Material,
};
use openshard_state::{
    Drawn,
    ItemLocation,
    SettledItemLocation,
    WorldState,
    item_definition,
    kind_from_drawn,
    material_definition,
};

use crate::recipe::Recipe;
use crate::system::{
    CraftSystemDef,
    Text,
};

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
    /// The recipe or the physical pile fragmentation exceeds the bounded work
    /// one realtime craft is allowed to commit.
    TooComplex,
}

/// The most resource lines one recipe may ask the realtime planner to join.
///
/// The generated recipe build asserts the same ceiling. Raising it is a
/// benchmark decision because every additional line can overlap every earlier
/// selector and therefore has to share the same reservation table.
///
/// It is five because the reference has a row that needs five: a scroll for one
/// of the four-reagent spells — Arch Protection, Greater Heal, Mind Blast and
/// eleven more — plus the blank scroll every scroll is written on. Four held
/// while the seven material trades were the whole catalogue, and none of them
/// asks for more than four.
pub const MAX_CRAFT_RESOURCE_LINES: usize = 5;

/// The most physical piles one atomic craft may change.
///
/// This matches the ordinary recursive container item ceiling. A fragmented
/// payment above it is refused before mutation instead of turning one command
/// into an unbounded tick.
pub const MAX_CRAFT_WITHDRAWALS: usize = openshard_items::MAX_ITEMS;

/// The largest indexed source root admitted to realtime craft preparation.
pub const MAX_CRAFT_SOURCE_ITEMS: usize = openshard_protocol::craft::MAX_CRAFT_SOURCE_ITEMS;

/// A `use_all_res` click is one bounded batch, not an instruction to drain an
/// arbitrarily large workshop.
pub const MAX_CRAFT_BATCH: u16 = openshard_items::MAX_STACK;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ExpectedIdentity {
    Semantic {
        kind:     openshard_protocol::item_kind::ItemKindId,
        material: Option<openshard_protocol::item_kind::MaterialId>,
    },
    Legacy(Drawn),
}

/// One physical pile reserved by an atomic material withdrawal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Withdrawal {
    item:     EntityId,
    serial:   Serial,
    source:   Serial,
    expected: u16,
    take:     u16,
    identity: ExpectedIdentity,
}

/// A deterministic, all-or-nothing payment prepared from current canonical
/// backpack state.
///
/// One item occurs at most once. Overlapping recipe selectors reserve from the
/// same remaining amount and their takes are folded into that item's row.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct WithdrawalPlan {
    source:          Serial,
    source_revision: u64,
    rows:            Vec<Withdrawal>,
}

impl WithdrawalPlan {
    /// Commit only the exact state validated by [`prepare_withdrawal`].
    ///
    /// The tick is the sole owner between prepare and commit. Any mismatch is
    /// therefore a broken caller invariant, not a gameplay refusal halfway
    /// through payment.
    pub fn commit(self, state: &mut WorldState) {
        let began = std::time::Instant::now();
        let withdrawals = self.rows.len();
        assert_eq!(
            state
                .craft_stock_amounts(self.source)
                .map(|(revision, _)| revision),
            Ok(self.source_revision),
            "a prepared craft source keeps its projection revision until commit"
        );
        let batch = state.begin_craft_stock_batch(self.source);
        for row in self.rows {
            assert_eq!(
                state.registry.entity_of(row.serial),
                Some(row.item),
                "a prepared craft pile keeps its serial until commit"
            );
            assert_eq!(
                openshard_items::amount_of(state, row.item),
                row.expected,
                "a prepared craft pile keeps its amount until commit"
            );
            assert!(
                matches!(
                    openshard_state::item_location(state, row.item),
                    Some(ItemLocation::Settled(SettledItemLocation::Contained(held)))
                        if held.container == row.source
                ),
                "a prepared craft pile keeps its source until commit"
            );
            assert!(
                expected_identity_matches(state, row.item, row.identity),
                "a prepared craft pile keeps its identity until commit"
            );
            assert!(
                openshard_items::consume(state, row.serial, row.take),
                "a validated craft withdrawal must consume its reserved pile"
            );
        }
        state.finish_craft_stock_batch(batch);
        tracing::trace!(
            metric = "item_transaction.withdrawal_commit",
            withdrawals,
            elapsed_ns = began.elapsed().as_nanos(),
        );
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether the recipe consumes no material piles.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// What one craft will actually eat, resolved against the crafter's pack.
#[derive(Clone, Debug)]
pub struct Materials {
    /// Each resolved ingredient. `semantic` is present when the registry names
    /// this legacy projection; the exact graphic/hue remains only for migration
    /// compatibility and classic presentation.
    pub lines:      Vec<MaterialLine>,
    /// How many items this craft will make. One, unless the recipe consumes
    /// everything in the pack — then it is as many as the scarcest line allows.
    pub max_amount: u16,
    /// The hue the finished item takes, where the recipe does not fix one: the
    /// colour of the material it was made from, which is what makes a valorite
    /// blade valorite-coloured.
    pub res_hue:    Hue,
}

/// One craft input after material-axis selection.
#[derive(Clone, Copy, Debug)]
pub struct MaterialLine {
    pub key:      CraftKey,
    pub graphic:  Graphic,
    pub hue:      Option<Hue>,
    pub amount:   u16,
    pub message:  Text,
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
        .and_then(|serial| openshard_items::backpack_of(state, serial))
    else {
        return Err(Refusal::NoPack);
    };
    let (_, amounts) = state.craft_stock_amounts(pack).map_err(|_| Refusal::TooComplex)?;

    check_with(state, crafter, system, recipe, sub_res, |semantic, legacy| {
        craft_key_for(semantic, legacy.id, legacy.hue)
            .and_then(|key| amounts.get(usize::from(key.0)).copied())
            .unwrap_or_default()
    })
}

fn check_with<F>(
    state: &WorldState,
    crafter: EntityId,
    system: &CraftSystemDef,
    recipe: &Recipe,
    sub_res: usize,
    held: F,
) -> Result<Materials, Refusal>
where
    F: Fn(
        Option<(
            openshard_protocol::item_kind::ItemKindId,
            Option<openshard_protocol::item_kind::MaterialId>,
        )>,
        Drawn,
    ) -> u32,
{
    if recipe.resources.len() > MAX_CRAFT_RESOURCE_LINES {
        return Err(Refusal::TooComplex);
    }
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
                id:  res.graphic,
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
        let legacy = Drawn {
            id:  res.graphic,
            hue: hue.expect("a craft ingredient has a resolved hue"),
        };
        let Some(key) = craft_key_for(semantic, legacy.id, legacy.hue) else {
            return Err(Refusal::TooComplex);
        };
        lines.push(MaterialLine {
            key,
            graphic: res.graphic,
            hue,
            amount: res.amount,
            message: res.message,
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
    if max_amount > MAX_CRAFT_BATCH {
        return Err(Refusal::TooComplex);
    }
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
        ItemSelector::KindWithMaterial { kind, material } => {
            match material {
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
            }
        }
        ItemSelector::Tag(_) => None,
        ItemSelector::Exact(_) => None,
    }
}

/// Reserve every physical pile a craft payment will touch, without mutation.
///
/// Candidates are sorted by wire serial before selection. The temporary
/// remaining table is shared by all resource lines, so duplicate or overlapping
/// selectors can never promise the same units twice.
pub fn prepare_withdrawal(
    state: &WorldState,
    crafter: EntityId,
    materials: &Materials,
    share: Share,
) -> Result<WithdrawalPlan, Refusal> {
    let began = std::time::Instant::now();
    if materials.lines.len() > MAX_CRAFT_RESOURCE_LINES || materials.max_amount > MAX_CRAFT_BATCH {
        tracing::trace!(
            metric = "item_transaction.withdrawal_prepare",
            refused = true,
            reason = "declared_bound",
            elapsed_ns = began.elapsed().as_nanos(),
        );
        return Err(Refusal::TooComplex);
    }
    let Some(owner) = state.registry.serial_of(crafter) else {
        return Err(Refusal::NoPack);
    };
    let Some(source) = openshard_items::backpack_of(state, owner) else {
        return Err(Refusal::NoPack);
    };

    let keys: Vec<_> = materials.lines.iter().map(|line| line.key).collect();
    let (source_revision, candidates) = state
        .craft_stock_piles(source, &keys)
        .map_err(|_| Refusal::TooComplex)?;

    let mut remaining: BTreeMap<Serial, u16> =
        candidates.iter().map(|pile| (pile.serial, pile.amount)).collect();
    let mut row_by_serial = BTreeMap::<Serial, usize>::new();
    let mut rows: Vec<Withdrawal> = Vec::new();

    for line in &materials.lines {
        let mut wanted = match share {
            Share::All => u32::from(line.amount) * u32::from(materials.max_amount),
            // A failed batch pays half of one craft, matching ServUO rather than
            // multiplying the failure cost by the whole `use_all_res` batch.
            Share::Half => u32::from(line.amount / 2).max(1),
        };
        if wanted == 0 {
            continue;
        }
        for pile in &candidates {
            if wanted == 0 {
                break;
            }
            let serial = pile.serial;
            let item = pile.item;
            let expected = pile.amount;
            let Some(identity) = line_identity_if_matches(state, item, *line) else {
                continue;
            };
            let available = remaining.get(&serial).copied().unwrap_or(0);
            if available == 0 {
                continue;
            }
            let reserved = u16::try_from(wanted.min(u32::from(available)))
                .expect("one reservation never exceeds one physical u16 pile");
            remaining.insert(serial, available - reserved);
            wanted -= u32::from(reserved);

            if let Some(&index) = row_by_serial.get(&serial) {
                let row = &mut rows[index];
                row.take = row
                    .take
                    .checked_add(reserved)
                    .expect("combined reservations cannot exceed the expected pile amount");
            } else {
                if rows.len() == MAX_CRAFT_WITHDRAWALS {
                    tracing::trace!(
                        metric = "item_transaction.withdrawal_prepare",
                        refused = true,
                        reason = "fragmentation",
                        elapsed_ns = began.elapsed().as_nanos(),
                    );
                    return Err(Refusal::TooComplex);
                }
                row_by_serial.insert(serial, rows.len());
                rows.push(Withdrawal {
                    item,
                    serial,
                    source,
                    expected,
                    take: reserved,
                    identity,
                });
            }
        }
        if wanted != 0 {
            return Err(Refusal::NotEnough(line.message));
        }
    }

    rows.sort_by_key(|row| (row.source, row.serial));
    tracing::trace!(
        metric = "item_transaction.withdrawal_prepare",
        candidates = candidates.len(),
        withdrawals = rows.len(),
        refused = false,
        elapsed_ns = began.elapsed().as_nanos(),
    );
    Ok(WithdrawalPlan {
        source,
        source_revision,
        rows,
    })
}

fn line_identity_if_matches(
    state: &WorldState,
    item: EntityId,
    line: MaterialLine,
) -> Option<ExpectedIdentity> {
    let legacy = Drawn {
        id:  line.graphic,
        hue: line.hue.expect("a prepared craft ingredient has a resolved hue"),
    };
    match line.semantic {
        Some((kind, material)) => {
            match state.registry.get::<ItemKind>(item) {
                Some(ItemKind(found))
                    if *found == kind
                        && state.registry.get::<Material>(item).map(|found| found.0) == material =>
                {
                    Some(ExpectedIdentity::Semantic { kind, material })
                }
                None if state.registry.get::<Drawn>(item) == Some(&legacy) => {
                    Some(ExpectedIdentity::Legacy(legacy))
                }
                _ => None,
            }
        }
        None if state.registry.get::<Drawn>(item) == Some(&legacy) => {
            match state.registry.get::<ItemKind>(item) {
                Some(ItemKind(kind)) => {
                    Some(ExpectedIdentity::Semantic {
                        kind:     *kind,
                        material: state.registry.get::<Material>(item).map(|found| found.0),
                    })
                }
                None => Some(ExpectedIdentity::Legacy(legacy)),
            }
        }
        None => None,
    }
}

fn expected_identity_matches(state: &WorldState, item: EntityId, identity: ExpectedIdentity) -> bool {
    match identity {
        ExpectedIdentity::Semantic { kind, material } => {
            state.registry.get::<ItemKind>(item) == Some(&ItemKind(kind))
                && state.registry.get::<Material>(item).map(|found| found.0) == material
        }
        ExpectedIdentity::Legacy(drawn) => state.registry.get::<Drawn>(item) == Some(&drawn),
    }
}

#[cfg(test)]
mod tests {
    use openshard_protocol::item_kind::{
        ItemKindId,
        MaterialId,
    };

    use super::*;

    #[test]
    fn a_material_axis_resolves_using_its_explicit_material_id() {
        let any_material = |kind| {
            ItemSelector::KindWithMaterial {
                kind,
                material: MaterialRule::Any,
            }
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
