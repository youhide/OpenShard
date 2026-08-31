//! Making the thing: the gates, the beat, and what comes out.
//!
//! ServUO's `CraftItem.Craft` → `InternalTimer` → `CompleteCraft`, which is one
//! sequence written across three places there and here too — [`begin`],
//! [`advance_crafts`] and [`complete`].
//!
//! **Every gate is checked twice, and that is the design.** ServUO dry-runs the
//! whole of `ConsumeRes` before it starts the timer and again when the timer ends,
//! and re-asks `CanCraft` both times. A craft takes seconds, and in those seconds
//! a player can step away from the forge, give the ingots away, or wear the tongs
//! out. Checking only at the start makes a smith who walked out of the shop finish
//! a sword in the street; checking only at the end makes a player watch an
//! animation for a craft that was never possible.

use openshard_entities::EntityId;
use openshard_protocol::item_kind::{
    ItemKindId,
    MaterialId,
};
use openshard_protocol::wire::{
    ClilocId,
    Graphic,
    Hue,
    SoundId,
};
use openshard_state::components::{
    CraftedBy,
    Crafting,
    Name,
    Position,
    Quality,
    Tool,
};
use openshard_state::{
    Drawn,
    WorldState,
    presentation_of,
};

use crate::chance::{
    roll,
    train_per_item,
};
use crate::consume::{
    self,
    Refusal,
    Share,
};
use crate::defs::{
    SYSTEMS,
    system,
};
use crate::environment;
use crate::recipe::{
    OutputMaterial,
    Recipe,
};
use crate::system::{
    CraftSystemDef,
    SystemId,
};

/// "You have worn out your tool!"
const TOOL_WORN_OUT: ClilocId = ClilocId(1_044_038);
/// "The tool must be on your person to use."
const TOOL_NOT_ON_PERSON: ClilocId = ClilocId(1_044_263);
/// "You don't have the required skills to attempt this item."
const NO_SKILL: ClilocId = ClilocId(1_044_153);
/// "You failed to create the item, and some of your materials are lost."
const FAILED_LOST: ClilocId = ClilocId(1_044_043);
/// "You failed to create the item, but no materials were lost."
const FAILED_KEPT: ClilocId = ClilocId(1_044_157);
/// "You create the item."
const MADE: ClilocId = ClilocId(1_044_154);
/// "You create an exceptional quality item."
const MADE_EXCEPTIONAL: ClilocId = ClilocId(1_044_155);
/// "You create an exceptional quality item and affix your maker's mark."
const MADE_MARKED: ClilocId = ClilocId(1_044_156);
/// "You must wait to perform another action." — ServUO's `BeginAction` refusal.
const ALREADY_BUSY: ClilocId = ClilocId(500_119);
/// The base skill a maker's mark wants: grandmaster, in tenths.
const MARK_AT: u16 = 1000;

/// An inclusive craft-beat range, stored as the non-zero bound the RNG needs.
#[derive(Clone, Copy, Debug)]
struct BeatRange {
    min:   u8,
    width: std::num::NonZeroU16,
}

impl BeatRange {
    fn inclusive(min: u8, max: u8) -> Option<Self> {
        let width = u16::from(max).checked_sub(u16::from(min))?.checked_add(1)?;
        Some(Self {
            min,
            width: std::num::NonZeroU16::new(width)?,
        })
    }

    fn roll(self, rng: &mut openshard_state::Rng) -> u8 {
        let beat = u16::from(self.min)
            + u16::try_from(rng.below(u32::from(self.width.get())))
                .expect("a draw below a u16 beat width fits u16");
        u8::try_from(beat).expect("a draw inside an inclusive u8 beat range fits u8")
    }
}

/// Somebody made something.
///
/// Emitted after the item is in the pack, so a pack reading this is reacting and
/// not deciding — the split `Harvested` and `MobileDied` already use.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ItemCrafted {
    /// Who made it.
    pub crafter:     EntityId,
    /// The item that came out, or `None` for a batch that could not be placed.
    pub item:        Option<EntityId>,
    /// Its art.
    pub graphic:     Graphic,
    /// Its hue, which for most materials is the material.
    pub hue:         Hue,
    /// Stable semantic kind when this was a migrated recipe row.
    pub item_kind:   Option<ItemKindId>,
    /// Semantic material when the result has one.
    pub material:    Option<MaterialId>,
    /// How many.
    pub amount:      u16,
    /// Whether it came out exceptional.
    pub exceptional: bool,
    /// Which system made it, so a pack can key a table on the trade.
    pub system:      SystemId,
}

/// Why a craft cannot be begun at all. The tool half of ServUO's `CanCraft`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Blocked {
    /// The tool is gone, or was never a tool.
    NoTool,
    /// It has no uses left.
    Spent,
    /// It is not on the crafter's person.
    NotCarried,
    /// The workshop is not here. The message is the *system's*, and a system
    /// that wants no workshop of its own has none — a recipe can add a
    /// requirement its system did not have, and then there is nothing written
    /// down to say. Silence beats cliloc zero, which the client would look up.
    NoWorkshop(Option<ClilocId>),
}

/// The tool and workshop gates — ServUO's `CanCraft`, less the recipe's own.
fn can_craft(
    state: &WorldState,
    crafter: EntityId,
    tool: EntityId,
    def: &CraftSystemDef,
    recipe: &Recipe,
) -> Result<(), Blocked> {
    if state.registry.serial_of(tool).is_none() {
        return Err(Blocked::NoTool);
    }
    let Some(tool_state) = state.registry.get::<Tool>(tool) else {
        return Err(Blocked::NoTool);
    };
    if tool_state.uses_left == 0 {
        return Err(Blocked::Spent);
    }
    // "On your person" is held, worn or in a container the crafter is carrying —
    // and a tool lying on the ground is none of those. `Position` is the exact
    // test, because the three item places are exclusive: an item with one is on
    // the floor by definition.
    if state.registry.has::<Position>(tool) {
        return Err(Blocked::NotCarried);
    }
    let needs = def.needs.union(recipe.needs);
    if needs.any() && !environment::around(state, crafter).satisfy(needs) {
        return Err(Blocked::NoWorkshop(def.needs_message));
    }
    Ok(())
}

/// Say why a craft was refused.
fn say(state: &mut WorldState, crafter: EntityId, blocked: Blocked) {
    let cliloc = match blocked {
        Blocked::NoTool | Blocked::NoWorkshop(None) => return,
        Blocked::Spent => TOOL_WORN_OUT,
        Blocked::NotCarried => TOOL_NOT_ON_PERSON,
        Blocked::NoWorkshop(Some(cliloc)) => cliloc,
    };
    state.localized_message(crafter, cliloc, "");
}

/// Say why the materials will not do.
fn say_materials(state: &mut WorldState, crafter: EntityId, refusal: Refusal) {
    match refusal {
        // ServUO says nothing at all for a missing pack, and neither does this:
        // a mobile with no backpack is a creature or a corpse, not a player who
        // needs telling.
        Refusal::NoPack => {}
        Refusal::NotEnough(text) | Refusal::CannotWork(text) => {
            match text {
                crate::system::Text::Cliloc(cliloc) => state.localized_message(crafter, cliloc, ""),
                crate::system::Text::Str(line) => state.system_message(crafter, line),
            }
        }
    }
}

/// Start a craft, or say why not.
///
/// Returns whether the hammer was raised. Everything checked here is checked
/// again in [`complete`]; see the module note for why that is not redundant.
pub fn begin(
    state: &mut WorldState,
    crafter: EntityId,
    tool: EntityId,
    system_id: SystemId,
    recipe_index: u16,
    sub_res: u8,
) -> bool {
    let Some(def) = system(system_id) else {
        return false;
    };
    let Some(recipe) = def.recipes.get(usize::from(recipe_index)) else {
        return false;
    };
    // One craft at a time — ServUO's per-mobile `BeginAction(typeof(CraftSystem))`.
    if state.registry.has::<Crafting>(crafter) {
        state.localized_message(crafter, ALREADY_BUSY, "");
        return false;
    }
    if let Err(blocked) = can_craft(state, crafter, tool, def, recipe) {
        say(state, crafter, blocked);
        return false;
    }
    // The skill gate is a refusal and not a failure: it costs nothing, and it is
    // checked before the materials so a hopeless recipe never reports a shortage
    // the player cannot act on.
    if !crate::chance::chance(state, crafter, def, recipe).all_skills {
        state.localized_message(crafter, NO_SKILL, "");
        return false;
    }
    if let Err(refusal) = consume::check(state, crafter, def, recipe, usize::from(sub_res)) {
        say_materials(state, crafter, refusal);
        return false;
    }

    // ServUO rolls the number of beats between the system's two bounds; both are
    // one nearly everywhere, which is what makes a craft take a moment rather
    // than an instant without ever making the player wait.
    let beats = BeatRange::inclusive(def.min_beats, def.max_beats)
        .expect("a shipped craft system's beat range is ordered")
        .roll(&mut state.rng);
    state.registry.insert(
        crafter,
        Crafting {
            system: system_id.raw(),
            recipe: recipe_index,
            tool,
            sub_res,
            beats_left: beats.max(1),
            next_beat: state.ticks + def.delay_ticks,
        },
    );
    // The first blow lands now rather than a beat from now, for the reason the
    // harvest's first swing does: otherwise the gump closes and nothing at all
    // happens for a second and a quarter.
    strike(state, crafter, def);
    true
}

/// Beat every craft in flight, and resolve those whose last beat has come.
pub fn advance_crafts(state: &mut WorldState) {
    let now = state.ticks;
    let live: Vec<(EntityId, Crafting)> = state
        .registry
        .query::<Crafting>()
        .map(|(entity, work)| (entity, *work))
        .collect();
    for (crafter, work) in live {
        if now < work.next_beat {
            continue;
        }
        let Some(def) = system(SystemId::new(work.system)) else {
            state.registry.remove::<Crafting>(crafter);
            continue;
        };
        if work.beats_left > 1 {
            state.registry.insert(
                crafter,
                Crafting {
                    beats_left: work.beats_left - 1,
                    next_beat: now + def.delay_ticks,
                    ..work
                },
            );
            strike(state, crafter, def);
            continue;
        }
        state.registry.remove::<Crafting>(crafter);
        complete(state, crafter, &work, def);
    }
}

/// The blow: what a craft looks and sounds like while it is happening.
///
/// Sound only, and that is ServUO's own choice — `DefBlacksmithy.PlayCraftEffect`
/// has its `Animate(9, 5, 1, …)` commented out, because the smithing gesture
/// reads badly against the anvil the player is standing at. Every other visible
/// action in this engine plays both; this one is the reference's exception, kept
/// deliberately rather than forgotten.
fn strike(state: &mut WorldState, crafter: EntityId, def: &CraftSystemDef) {
    // ServUO's `DisruptiveAction` on every tick: hammering is not meditating, and
    // it gives a hidden crafter away.
    state.disrupt(crafter);
    state.break_cover(crafter);
    if def.craft_sound != SoundId(0) {
        state.play_sound(crafter, def.craft_sound);
    }
}

/// The last beat: re-check everything, roll, and pay out.
fn complete(state: &mut WorldState, crafter: EntityId, work: &Crafting, def: &CraftSystemDef) {
    let Some(recipe) = def.recipes.get(usize::from(work.recipe)) else {
        return;
    };
    if let Err(blocked) = can_craft(state, crafter, work.tool, def, recipe) {
        say(state, crafter, blocked);
        return;
    }
    let materials = match consume::check(state, crafter, def, recipe, usize::from(work.sub_res)) {
        Ok(materials) => materials,
        Err(refusal) => {
            say_materials(state, crafter, refusal);
            return;
        }
    };

    let outcome = roll(state, crafter, def, recipe);
    if !outcome.all_skills {
        // Still a refusal even here — the crafter's skill can have *fallen* under
        // a bard's Discordance while the hammer was up. No materials lost.
        state.localized_message(crafter, NO_SKILL, "");
        wear_tool(state, crafter, work.tool);
        return;
    }
    if !outcome.success {
        let share = if recipe.use_all_res {
            Share::Half
        } else {
            Share::All
        };
        let lost = consume::take(state, crafter, &materials, share);
        state.localized_message(crafter, if lost { FAILED_LOST } else { FAILED_KEPT }, "");
        wear_tool(state, crafter, work.tool);
        return;
    }

    let (identity, hue) = match output_identity(recipe, &materials) {
        Ok(Some((kind, material, drawn))) => (Some((kind, material)), drawn.hue),
        Ok(None) => {
            let hue = if recipe.hue != Hue(0) {
                recipe.hue
            } else if recipe.retain_color {
                materials.res_hue
            } else {
                Hue(0)
            };
            (None, hue)
        }
        Err(()) => {
            // A bad typed row is shard data, not a player mistake. Do not spend
            // ingredients trying to make an item whose identity has no valid
            // presentation in the registry.
            state.system_message(crafter, "This recipe is not configured correctly.");
            return;
        }
    };

    consume::take(state, crafter, &materials, Share::All);
    if recipe.use_all_res {
        // The passive per-skill check is skipped for a batch craft, and this is
        // what stands in for it: one roll per item made, so a hundred boards
        // teach a hundred boards' worth.
        train_per_item(state, crafter, recipe, materials.max_amount);
    }

    let made = recipe.amount.saturating_mul(materials.max_amount).max(1);
    let (item, placed) = place(state, crafter, recipe, hue, identity, made);
    if placed != made {
        state.system_message(
            crafter,
            &format!("Only {placed} of {made} crafted items could be placed in your pack."),
        );
        wear_tool(state, crafter, work.tool);
        return;
    }

    let marked = outcome.exceptional && recipe.markable && grandmaster(state, crafter, def);
    if let Some(item) = item {
        if outcome.exceptional {
            state.registry.insert(item, Quality { exceptional: true });
        }
        if marked {
            if let Some(Name(name)) = state.registry.get::<Name>(crafter) {
                let name = name.clone();
                state.registry.insert(item, CraftedBy(name));
            }
        }
    }
    let line = match (outcome.exceptional, marked) {
        (true, true) => MADE_MARKED,
        (true, false) => MADE_EXCEPTIONAL,
        (false, _) => MADE,
    };
    state.localized_message(crafter, line, "");
    state.bus.send(ItemCrafted {
        crafter,
        item,
        graphic: recipe.graphic,
        hue,
        item_kind: identity.map(|(kind, _)| kind),
        material: identity.and_then(|(_, material)| material),
        amount: made,
        exceptional: outcome.exceptional,
        system: SystemId::new(work.system),
    });
    wear_tool(state, crafter, work.tool);
}

/// Put the finished item in the crafter's pack.
///
/// A stacking recipe (arrows, boards) merges onto the pile already there; a
/// discrete one is placed as its own piece, which is what lets it carry a quality
/// and a maker's name at all — a signature on a stack would belong to whichever
/// pile it merged into.
fn place(
    state: &mut WorldState,
    crafter: EntityId,
    recipe: &Recipe,
    hue: Hue,
    identity: Option<(ItemKindId, Option<MaterialId>)>,
    amount: u16,
) -> (Option<EntityId>, u16) {
    let Some(serial) = state.registry.serial_of(crafter) else {
        return (None, 0);
    };
    let Some(pack) = openshard_items::backpack_of(state, serial) else {
        return (None, 0);
    };
    if recipe.use_all_res || recipe.amount > 1 || amount > 1 {
        let outcome = match identity {
            Some((kind, material)) => {
                openshard_items::give_kind(state, pack, kind, material, u32::from(amount))
                    .expect("a checked typed recipe has a presentation")
            }
            None => openshard_items::give(state, pack, recipe.graphic, hue, u32::from(amount)),
        };
        (
            outcome.last,
            u16::try_from(outcome.given).expect("give cannot exceed the u16 amount requested"),
        )
    } else {
        let item = match identity {
            Some((kind, material)) => openshard_items::place_one_kind(state, pack, kind, material, amount),
            None => openshard_items::place_one(state, pack, recipe.graphic, hue, amount),
        };
        (item, u16::from(item.is_some()))
    }
}

/// Resolve the semantic output before ingredients are spent.
fn output_identity(
    recipe: &Recipe,
    materials: &consume::Materials,
) -> Result<Option<(ItemKindId, Option<MaterialId>, Drawn)>, ()> {
    let Some(kind) = recipe.kind else {
        return matches!(recipe.output_material, OutputMaterial::Legacy)
            .then_some(None)
            .ok_or(());
    };
    let material = match recipe.output_material {
        OutputMaterial::Legacy => return Err(()),
        OutputMaterial::None => None,
        OutputMaterial::Fixed(material) => Some(material),
        OutputMaterial::InheritInput(input) => {
            materials
                .lines
                .get(usize::from(input))
                .and_then(|line| line.semantic)
                .and_then(|(_, material)| material)
                .ok_or(())
                .map(Some)?
        }
    };
    presentation_of(kind, material)
        .map(|drawn| Some((kind, material, drawn)))
        .ok_or(())
}

/// Whether the crafter is good enough to sign their work — ServUO's
/// `Skills[MainSkill].Base >= 100.0`, the **base** and not the stat-lent value.
fn grandmaster(state: &WorldState, crafter: EntityId, def: &CraftSystemDef) -> bool {
    state
        .registry
        .get::<openshard_state::components::Skills>(crafter)
        .is_some_and(|skills| skills.get(def.skill) >= MARK_AT)
}

/// Spend a use off the tool, and make it gone if that was the last.
///
/// A craft costs a use whether it worked or not — ServUO decrements on both the
/// success and the failure path, which is what makes a run of bad luck expensive
/// in more than materials.
fn wear_tool(state: &mut WorldState, crafter: EntityId, tool: EntityId) {
    let Some(left) = state.registry.get::<Tool>(tool).map(|worn| worn.uses_left) else {
        return;
    };
    let left = left.saturating_sub(1);
    if left > 0 {
        state.registry.insert(tool, Tool { uses_left: left });
        return;
    }
    state.registry.remove::<Tool>(tool);
    state.localized_message(crafter, TOOL_WORN_OUT, "");
    if let Some(serial) = state.registry.serial_of(tool) {
        openshard_items::consume(state, serial, 0);
    }
}

/// Which system a double-clicked item drives, if any.
///
/// Two steps rather than one table: [`openshard_state::craft::craft_tool`] says
/// which *trade* a graphic practises, and the systems are found by their main
/// skill. The tool table lives in `state` because `items` reads it too, and a
/// second copy here keyed by system is the pair of hand-kept halves the mount
/// table's lesson is about.
#[must_use]
pub fn tool_system(graphic: Graphic) -> Option<SystemId> {
    let tool = openshard_state::craft::craft_tool(graphic)?;
    SYSTEMS
        .iter()
        .position(|def| def.skill == tool.skill)
        .and_then(SystemId::from_index)
}

/// The craft system a registered tool kind opens.
///
/// Legacy items without semantic identity still use [`tool_system`], but a
/// declared non-tool kind cannot become a crafting tool merely by sharing art.
#[must_use]
pub fn tool_system_for_kind(kind: ItemKindId) -> Option<SystemId> {
    let tool = openshard_state::craft::craft_tool_for_kind(kind)?;
    SYSTEMS
        .iter()
        .position(|def| def.skill == tool.skill)
        .and_then(SystemId::from_index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_craft_system_has_a_bounded_representable_beat_roll() {
        let mut rng = openshard_state::Rng::new(7);
        for (index, def) in SYSTEMS.iter().enumerate() {
            let range = BeatRange::inclusive(def.min_beats, def.max_beats)
                .unwrap_or_else(|| panic!("craft system {index} has a reversed beat range"));
            for _ in 0..1000 {
                assert!((def.min_beats..=def.max_beats).contains(&range.roll(&mut rng)));
            }
        }

        assert!(
            BeatRange::inclusive(2, 1).is_none(),
            "a reversed range is not a roll"
        );
        let whole = BeatRange::inclusive(u8::MIN, u8::MAX).expect("the full u8 range fits u16");
        assert_eq!(whole.width.get(), u16::from(u8::MAX) + 1);
    }
}
