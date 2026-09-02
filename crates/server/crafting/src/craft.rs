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
use openshard_protocol::casting::SpellId;
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
use openshard_skills::skill_value;
use openshard_state::components::{
    AddonDeed,
    CraftedBy,
    Crafting,
    Mana,
    Name,
    Position,
    Quality,
    Runebook,
    Tool,
    scroll_spell,
};
use openshard_state::{
    Drawn,
    Skill,
    WorldState,
    presentation_of,
};

use crate::chance::{
    roll,
    train_attempt,
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
/// "You don't have that spell!" — ServUO's `DefInscription.CanCraft`, refusing to
/// write down a spell the scribe's own book has not got.
const NO_SPELL: ClilocId = ClilocId(1_042_404);
/// "You inscribe the spell and put the scroll in your backpack."
const SCROLL_MADE: ClilocId = ClilocId(501_629);
/// "You fail to inscribe the scroll, and the scroll is ruined."
const SCROLL_RUINED: ClilocId = ClilocId(501_630);
/// ServUO says this one as a plain string rather than a cliloc, and so does this.
const NO_MANA: &str = "You lack the required mana to make that.";
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

/// Every fallible fact of a successful craft, settled before ingredients,
/// output, training, tool wear, or the domain event become visible.
struct PreparedCraft {
    withdrawal: consume::WithdrawalPlan,
    placement:  openshard_items::PreparedPlacement,
    identity:   Option<(ItemKindId, Option<MaterialId>)>,
    hue:        Hue,
    amount:     u16,
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
    /// The row writes a spell down and no spellbook in the scribe's pack holds
    /// it — ServUO's `DefInscription.CanCraft`.
    UnknownSpell,
    /// The row costs mana and the crafter has not got that much — ServUO's
    /// `CraftItem.ConsumeAttributes`, dry-run.
    NoMana,
}

/// The Magery spell a row writes down, or `None` for a row that makes an
/// ordinary thing.
///
/// Derived from the output art rather than carried as a column: the art of a
/// Magery scroll names its spell (`openshard_state::scroll_spell`), and that is
/// the same table the spellbook reads when a scroll is dropped on it. A column
/// beside it would be a second place to be wrong, and the pair could disagree
/// about which spell a scroll is — which matters more here than it looks, since
/// the run is not `base + spell`: the first circle is rotated.
fn writes_a_spell(recipe: &Recipe) -> Option<SpellId> {
    scroll_spell(recipe.graphic)
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
    // A scribe may only write down a spell they have. ServUO asks the crafter's
    // own spellbook (`Spellbook.Find`), so a book lying on the table is no help
    // and neither is one in the bank.
    if let Some(spell) = writes_a_spell(recipe) {
        let carries = state
            .registry
            .serial_of(crafter)
            .is_some_and(|serial| openshard_items::carries_spell(state, serial, spell));
        if !carries {
            return Err(Blocked::UnknownSpell);
        }
    }
    // And pay for it in mana, which only those rows cost. Zero is "no
    // requirement" and not "free": a crafter with no mana pool at all — an NPC
    // smith, a creature — is refused nothing by this.
    if recipe.mana > 0 {
        let held = state.registry.get::<Mana>(crafter).map_or(0, |mana| mana.current);
        if held < recipe.mana {
            return Err(Blocked::NoMana);
        }
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
        Blocked::UnknownSpell => NO_SPELL,
        Blocked::NoMana => {
            state.system_message(crafter, NO_MANA);
            return;
        }
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
        Refusal::TooComplex => {
            state.system_message(
                crafter,
                "That material payment is too fragmented to craft at once.",
            );
        }
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

    // Stage the two outcome draws on a private copy. A successful/failing
    // attempt commits that stream position; an output allocation/capacity
    // refusal does not consume randomness for a craft that never happened.
    let mut prepared_rng = state.rng.clone();
    let outcome = roll(state, crafter, def, recipe, &mut prepared_rng);
    if !outcome.all_skills {
        // Still a refusal even here — the crafter's skill can have *fallen* under
        // a bard's Discordance while the hammer was up. No materials lost.
        state.localized_message(crafter, NO_SKILL, "");
        wear_tool(state, crafter, work.tool);
        return;
    }
    if !outcome.success {
        state.rng = prepared_rng;
        let share = if recipe.use_all_res {
            Share::Half
        } else {
            Share::All
        };
        let lost = match consume::prepare_withdrawal(state, crafter, &materials, share) {
            Ok(plan) => {
                plan.commit(state);
                true
            }
            Err(_) => false,
        };
        train_attempt(state, crafter, recipe);
        // A ruined scroll says so in its own words, and says nothing about the
        // materials — ServUO's `PlayEndingEffect` takes the whole scroll branch
        // before it ever looks at `lostMaterial`. The mana is *not* spent: only
        // the finished scroll costs it.
        let line = if writes_a_spell(recipe).is_some() {
            SCROLL_RUINED
        } else if lost {
            FAILED_LOST
        } else {
            FAILED_KEPT
        };
        state.localized_message(crafter, line, "");
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

    let made = match recipe.amount.checked_mul(materials.max_amount) {
        Some(made) if openshard_items::is_valid_stack_amount(made) => made,
        _ => {
            state.system_message(crafter, "This recipe produces too many items at once.");
            return;
        }
    };
    let withdrawal = match consume::prepare_withdrawal(state, crafter, &materials, Share::All) {
        Ok(plan) => plan,
        Err(refusal) => {
            say_materials(state, crafter, refusal);
            return;
        }
    };
    let Some(owner) = state.registry.serial_of(crafter) else {
        return;
    };
    let Some(pack) = openshard_items::backpack_of(state, owner) else {
        return;
    };
    let stacks = recipe.use_all_res || recipe.amount > 1 || made > 1;
    let output_began = std::time::Instant::now();
    let placement_result = match identity {
        Some((kind, material)) => {
            openshard_items::prepare_kind_placement(state, crafter, pack, kind, material, made, stacks)
        }
        None => openshard_items::prepare_placement(state, crafter, pack, recipe.graphic, hue, made, stacks),
    };
    tracing::trace!(
        metric = "item_transaction.output_prepare",
        amount = made,
        refused = placement_result.is_err(),
        elapsed_ns = output_began.elapsed().as_nanos(),
    );
    let placement = match placement_result {
        Ok(placement) => placement,
        Err(openshard_items::PreparePlacementError::Full(full)) => {
            state.system_message(crafter, full.message());
            return;
        }
        Err(openshard_items::PreparePlacementError::NoSerials) => {
            state.system_message(crafter, "The crafted item could not be created.");
            return;
        }
        Err(
            openshard_items::PreparePlacementError::NoContainer
            | openshard_items::PreparePlacementError::InvalidAmount
            | openshard_items::PreparePlacementError::UnknownIdentity,
        ) => {
            state.system_message(crafter, "This recipe is not configured correctly.");
            return;
        }
    };
    let prepared = PreparedCraft {
        withdrawal,
        placement,
        identity,
        hue,
        amount: made,
    };

    let commit_began = std::time::Instant::now();
    state.rng = prepared_rng;
    prepared.withdrawal.commit(state);
    // Mana is spent here and nowhere else: after the last refusal has passed and
    // beside the materials, so a scribe who could not be given the scroll — a
    // full pack — has paid neither. `can_craft` re-checked the pool a moment
    // ago, on this same tick, so the subtraction cannot go under.
    if recipe.mana > 0 {
        if let Some(&Mana { current, max }) = state.registry.get::<Mana>(crafter) {
            state.set_mana(
                crafter,
                Mana {
                    current: current.saturating_sub(recipe.mana),
                    max,
                },
            );
        }
    }
    if recipe.use_all_res {
        // The passive per-skill check is skipped for a batch craft, and this is
        // what stands in for it: one roll per item made, so a hundred boards
        // teach a hundred boards' worth.
        train_per_item(state, crafter, recipe, materials.max_amount);
    } else {
        train_attempt(state, crafter, recipe);
    }
    let item = prepared.placement.commit(state);

    let marked = outcome.exceptional && recipe.markable && grandmaster(state, crafter, def);
    if let Some(item) = item {
        if let Some(addon) = recipe.addon {
            // Identity is the typed output's `ItemKind`, already installed by
            // the placement above; this label is cosmetic only.
            state.registry.insert(item, AddonDeed { addon });
            state.registry.insert(item, Name(addon.label().to_owned()));
        }
        if outcome.exceptional {
            state.registry.insert(item, Quality { exceptional: true });
        }
        stamp_runebook_charges(state, crafter, item, outcome.exceptional);
        if marked {
            if let Some(Name(name)) = state.registry.get::<Name>(crafter) {
                let name = name.clone();
                state.registry.insert(item, CraftedBy(name));
            }
        }
    }
    // A scroll says the same thing whatever its quality — ServUO's ending effect
    // branches on the type before it reads the quality at all, which is also why
    // no scroll row is markable.
    let line = if writes_a_spell(recipe).is_some() {
        SCROLL_MADE
    } else {
        match (outcome.exceptional, marked) {
            (true, true) => MADE_MARKED,
            (true, false) => MADE_EXCEPTIONAL,
            (false, _) => MADE,
        }
    };
    state.localized_message(crafter, line, "");
    state.bus.send(ItemCrafted {
        crafter,
        item,
        graphic: recipe.graphic,
        hue: prepared.hue,
        item_kind: prepared.identity.map(|(kind, _)| kind),
        material: prepared.identity.and_then(|(_, material)| material),
        amount: prepared.amount,
        exceptional: outcome.exceptional,
        system: SystemId::new(work.system),
    });
    wear_tool(state, crafter, work.tool);
    tracing::trace!(
        metric = "item_transaction.craft_commit",
        amount = prepared.amount,
        elapsed_ns = commit_began.elapsed().as_nanos(),
    );
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

/// Give a freshly made runebook the charges its scribe earned it.
///
/// ServUO's `Runebook.OnCraft`: `5 + quality + Inscribe/30`, capped at ten, with
/// `quality` 1 for an ordinary book and 2 for an exceptional one. A book that
/// nobody crafted keeps the flat six `openshard_items` gives it — a vendor's
/// book is nobody's work — so this is the only place the number is earned.
///
/// **One divergence, deliberate:** upstream sets `MaxCharges` and leaves
/// `CurCharges` at zero, so a new book there is empty until Recall scrolls are
/// dropped on it. This engine hands a made book its charges the same way it
/// hands a shelf book its six, because the two rules living side by side would
/// mean a bought book that works and a made one that does not.
fn stamp_runebook_charges(state: &mut WorldState, crafter: EntityId, item: EntityId, exceptional: bool) {
    let Some(book) = state.registry.get::<Runebook>(item).cloned() else {
        return;
    };
    let quality = if exceptional { 2 } else { 1 };
    let inscribe = u32::from(skill_value(state, crafter, Skill::Inscribe));
    // Tenths here, whole points there: ServUO divides `Skills.Value` by 30.
    let charges =
        u8::try_from((5 + quality + inscribe / 300).min(10)).expect("a charge count under ten fits u8");
    state.registry.insert(
        item,
        Runebook {
            charges,
            max_charges: charges,
            ..book
        },
    );
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
