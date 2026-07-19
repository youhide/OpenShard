//! The scripting runtime: TypeScript/JavaScript in-process, behind a narrow seam.
//!
//! # What this crate is
//!
//! The spike for roadmap §5 — the largest open technical risk in the project.
//! The question it answers is not "can we run JavaScript" but "can we run it
//! *inside a tick*": at 20Hz a tick has 50ms, and if a script hook has to fire
//! for a thousand mobiles every one of those, the per-call cost has to be small
//! enough that `entities × cost` leaves room for everything else a tick does.
//! [`DenoEngine`] is the answer and `examples/benchmark.rs` is the measurement.
//!
//! # The seam
//!
//! A script is not a new kind of thing wired through the engine. It is one more
//! consumer of the same two channels every system already uses:
//!
//! - **Domain events in.** [`ScriptEngine::deliver`] hands the script a
//!   [`Event`] — `PlayerEntered`, `MobileMoved`, `StepRefused`, `PlayerLeft` —
//!   exactly as the client and persistence receive the same facts. The engine
//!   also keeps a small read model from these, so a hook can read where a mobile
//!   *is* without a round-trip into the world.
//! - **Commands out.** A script never writes the world. It enqueues a
//!   [`Command`], drained with [`ScriptEngine::take_commands`], and the tick
//!   applies it in order — the same rule the network layer lives by. Reads are
//!   direct; writes go through the queue.
//!
//! [`ScriptEngine`] is deliberately small. Nothing in its signatures is
//! V8-shaped: no isolate, no `deno_core` type, no `v8::Local`. That is the graded
//! constraint of the spike — the runtime behind the trait has to be replaceable,
//! so `deno_core` lives *entirely* inside [`DenoEngine`] and never leaks past it.

mod engine;

pub use engine::DenoEngine;

/// A wire serial: the identity every packet about a mobile already carries, and
/// so the identity a script names an entity by.
///
/// A plain `u32` on purpose. The scripting layer has no opinion about how the
/// world stores entities — it speaks the same identity the protocol does, and
/// the glue that owns both maps one to the other.
pub type Serial = u32;

/// Something the world says happened, handed to a script.
///
/// These mirror the world's domain events rather than re-inventing them: a
/// script is another reader of the same bus. The engine both forwards each to
/// the script's handler and updates its own read model from it, which is what
/// lets a hook read a mobile's position without asking the world.
// Not `Copy`: `MobileSpoke` carries the words, an owned `String`. The engine
// clones an event to hand it to V8, which is the sparse path anyway.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize)]
#[serde(tag = "type")]
pub enum Event {
    /// A character entered the world.
    PlayerEntered {
        /// Its wire identity.
        serial: Serial,
        /// Where it appeared.
        x: u16,
        /// Where it appeared.
        y: u16,
        /// Where it appeared.
        z: i8,
    },
    /// A creature or NPC appeared — the mobile a script can take control of.
    MobileSpawned {
        /// Its wire identity.
        serial: Serial,
        /// Where it appeared.
        x: u16,
        /// Where it appeared.
        y: u16,
        /// Where it appeared.
        z: i8,
    },
    /// A client asked to cast a spell — the hook a script turns into a real cast,
    /// looking up the spell's mana and reagents from its own data.
    SpellRequested {
        /// The caster's wire identity.
        serial: Serial,
        /// Which spell, zero-based.
        spell: u16,
    },
    /// A mobile took a step.
    MobileMoved {
        /// Its wire identity.
        serial: Serial,
        /// Where it is now.
        x: u16,
        /// Where it is now.
        y: u16,
        /// Where it is now.
        z: i8,
        /// Which way it now faces.
        facing: u8,
    },
    /// A step was refused — what a speedhack looks like from outside.
    StepRefused {
        /// Its wire identity.
        serial: Serial,
        /// Why, as the world's `RefusedReason` discriminant.
        reason: u8,
    },
    /// A character left the world.
    PlayerLeft {
        /// Its wire identity, now released.
        serial: Serial,
    },
    /// A mobile died — combat's headline event, for loot, notoriety and quests.
    MobileDied {
        /// Its wire identity.
        serial: Serial,
    },
    /// A skill was used: the check is resolved and any gain applied. A script
    /// reads this to grant what the use was *for* — the ore, the pick's turn.
    SkillUsed {
        /// Whose, by wire identity.
        serial: Serial,
        /// Which skill, by id.
        skill: u8,
        /// Whether the check passed.
        success: bool,
        /// The skill's value now, in tenths.
        value: u16,
    },
    /// A spell was cast: mana paid, skill rolled. The script reads this and gives
    /// the spell its effect — damage, a heal, a summon.
    SpellCast {
        /// The caster's wire identity.
        serial: Serial,
        /// Which spell, by id.
        spell: u16,
        /// The target's serial, or 0 for none.
        target: u32,
        /// Whether the cast took.
        success: bool,
    },
    /// A mobile spoke: the hook for chat commands, keyword answers, NPC dialogue.
    MobileSpoke {
        /// The speaker's wire identity.
        serial: Serial,
        /// What was said.
        text: String,
    },
    /// A game master pressed a button in the `.admin` menu. The engine only
    /// carries the verb across; the pack decides what it does — which spawn set to
    /// register, what to clear. This is how staff tools reach the script pack.
    AdminAction {
        /// The game master's wire identity.
        serial: Serial,
        /// The action the button asked for, e.g. `"populate:britain"`.
        action: String,
    },
}

/// What a script asks the world to do.
///
/// A script never touches the world directly; it enqueues one of these and the
/// tick applies it, in order, on the tick's thread. The vocabulary is small on
/// purpose — the spike proves the path, gameplay (§6) fills it in.
// Not `Copy`: `Speak` carries an owned `String`. Commands are drained by value,
// so `Clone` is all the engine asks of them.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Command {
    /// Move a mobile one step in a direction (0–7, N clockwise), the world to
    /// validate it exactly as it validates a client's step.
    Move {
        /// Whom to move.
        serial: Serial,
        /// Which way (0–7).
        direction: u8,
    },
    /// Put an item on the ground for the world to draw to everyone in range.
    SpawnItem {
        /// The tiledata graphic id.
        graphic: u16,
        /// Its hue, or 0 for none.
        hue: u16,
        /// How many, for a stackable item; 0 or 1 is a single.
        amount: u16,
        /// Whether it merges with an identical pile when dropped onto one.
        stackable: bool,
        /// Where it lies.
        x: u16,
        /// Where it lies.
        y: u16,
        /// Where it lies.
        z: i8,
        /// Which facet.
        facet: u8,
    },
    /// Put a container on the ground — an item others can be put inside.
    SpawnContainer {
        /// The tiledata graphic id.
        graphic: u16,
        /// The gump the client opens when it is double-clicked.
        gump: u16,
        /// Its hue, or 0 for none.
        hue: u16,
        /// Where it lies.
        x: u16,
        /// Where it lies.
        y: u16,
        /// Where it lies.
        z: i8,
        /// Which facet.
        facet: u8,
    },
    /// Put a mobile in the world — a creature to fight or an NPC to stand there.
    SpawnMobile {
        /// The body graphic.
        body: u16,
        /// Its hue.
        hue: u16,
        /// Its starting and maximum hit points.
        hits: u16,
        /// Its standing (health-bar colour) as a wire byte: 1 innocent, 5 enemy,
        /// 7 invulnerable.
        notoriety: u8,
        /// How hard it hits in melee, before the target's armour.
        damage: u16,
        /// Its physical resistance, 0–100.
        resistance: u8,
        /// Ticks between its swings; 0 takes the default.
        swing: u64,
        /// How far it notices a foe, in tiles; 0 is passive.
        sight: u8,
        /// Whether it starts fights (2), answers them (1), or only runs (0).
        aggression: u8,
        /// Ticks between its beats while hunting; 0 takes the shard default.
        beat: u64,
        /// How far its ranged attack reaches, in tiles; 0 fights hand to hand.
        ranged: u8,
        /// The ranged attack's damage type wire value.
        ranged_kind: u8,
        /// Whether it wanders when idle.
        wander: bool,
        /// Where it stands.
        x: u16,
        /// Where it stands.
        y: u16,
        /// Where it stands.
        z: i8,
        /// Which facet.
        facet: u8,
        /// A name the client shows on single-click, for a townsperson; empty for a
        /// nameless creature.
        name: String,
        /// Whether it is a banker — saying "bank" near it opens the box.
        banker: bool,
        /// Whether it is a shopkeeper — double-click opens its shop.
        vendor: bool,
        /// Worn clothing and gear, so it is not naked.
        equipment: Vec<WornItem>,
    },
    /// Fill a vendor's stock crate with priced goods.
    StockVendor {
        /// The vendor mobile's wire serial.
        serial: u32,
        /// The goods, priced and labelled.
        stock: Vec<StockItem>,
    },
    /// Deal damage to a mobile, of a kind (0 physical, 1 fire, 2 cold, 3 poison,
    /// 4 energy) the target's resistance to that kind reduces.
    Damage {
        /// Whom.
        serial: Serial,
        /// How much, before resistance.
        amount: u16,
        /// The damage type as a wire byte.
        damage_type: u8,
        /// Who dealt it, or 0 for unattributed — a spell's caster, so a kill is
        /// blamed the same as a melee blow.
        by: u32,
    },
    /// Heal a mobile, up to its maximum.
    Heal {
        /// Whom.
        serial: Serial,
        /// By how much.
        amount: u16,
    },
    /// Cast a spell: the world pays mana and rolls the skill, and reports back
    /// with a [`SpellCast`](Event::SpellCast) event for the script to give the
    /// spell its effect.
    CastSpell {
        /// The caster.
        serial: Serial,
        /// Which spell, by id.
        spell: u16,
        /// The target's serial, or 0 for none.
        target: u32,
        /// The mana cost.
        mana: u16,
        /// The casting difficulty, 0–100.
        difficulty: u16,
        /// The skill id it rolls (Magery).
        skill: u8,
        /// The container to draw reagents from, or 0 for none.
        pack: u32,
        /// The reagents the spell consumes, as `(graphic, count)`.
        reagents: Vec<(u16, u16)>,
    },
    /// Set a mobile's stats; strength and intelligence re-cap hits and mana.
    SetStats {
        /// Whose.
        serial: Serial,
        /// Strength.
        strength: u16,
        /// Dexterity.
        dexterity: u16,
        /// Intelligence.
        intelligence: u16,
    },
    /// Set a mobile's skill value, in tenths.
    SetSkill {
        /// Whose.
        serial: Serial,
        /// Which skill, by id.
        skill: u8,
        /// The value in tenths.
        value: u16,
    },
    /// Use a skill against a difficulty (0–100): roll, gain, and report back
    /// through a [`SkillUsed`](Event::SkillUsed) event.
    UseSkill {
        /// Whose.
        serial: Serial,
        /// Which skill, by id.
        skill: u8,
        /// The difficulty, 0–100.
        difficulty: u16,
    },
    /// Make a mobile speak — an NPC's line, a keyword answer.
    Speak {
        /// Who.
        serial: Serial,
        /// The colour.
        hue: u16,
        /// The words.
        text: String,
    },
    /// Take control of a mobile's brain: the built-in `ai` stops driving it and
    /// this script's `onTick` runs it each tick instead.
    Control {
        /// The mobile.
        serial: Serial,
    },
    /// Register a spawn region the world keeps populated. The pack builds these
    /// from its own spawn data — a town's animals, a graveyard's undead — and the
    /// tick maintains them: a creature dies, another takes its place.
    RegisterSpawner {
        /// West edge.
        x: u16,
        /// North edge.
        y: u16,
        /// Width in tiles.
        width: u16,
        /// Height in tiles.
        height: u16,
        /// Which facet.
        facet: u8,
        /// The most live creatures the region keeps.
        max_count: u16,
        /// Ticks to wait after a spawn before the next.
        respawn_delay: u64,
        /// The creatures it may put down; each spawn picks one.
        creatures: Vec<SpawnCreature>,
    },
    /// Remove every spawn region and the creatures they were maintaining.
    ClearSpawners,
    /// Place decoration: script-added statics — signs, furniture — the shard puts
    /// on top of the static art the client's map already draws. In a batch,
    /// because a city is many at once.
    Decorate {
        /// Which facet.
        facet: u8,
        /// The plain statics to place.
        statics: Vec<DecorStatic>,
        /// The doors to place — decoration that opens on double-click.
        doors: Vec<DecorDoor>,
        /// The containers to place — decoration that opens onto a gump.
        containers: Vec<DecorContainer>,
    },
    /// Remove every script-placed decoration.
    ClearDecorations,
    /// Generate functional doors from the map's static frames in a region — the
    /// shop doors a building's static art only implies.
    GenerateDoors {
        /// Which facet.
        facet: u8,
        /// The region's north-west corner and size, in tiles.
        x: u16,
        /// North-west corner y.
        y: u16,
        /// Region width.
        width: u16,
        /// Region height.
        height: u16,
    },
}

/// One worn item on a spawned mobile — a robe, hair, a weapon. The clothing a
/// [`SpawnMobile`](Command::SpawnMobile) dresses an NPC in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WornItem {
    /// The item graphic.
    pub graphic: u16,
    /// The equipment layer.
    pub layer: u8,
    /// Its hue, or 0.
    pub hue: u16,
}

/// One placed decoration — a graphic at a tile. The batch a
/// [`Decorate`](Command::Decorate) carries.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DecorStatic {
    /// The tiledata graphic id.
    pub graphic: u16,
    /// Its hue, or 0.
    pub hue: u16,
    /// Where.
    pub x: u16,
    /// Where.
    pub y: u16,
    /// Where.
    pub z: i8,
}

/// One placed door — a decoration that opens and closes. The pack computes the
/// closed/open graphics and the hinge offset from the door's facing; the engine
/// only toggles.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DecorDoor {
    /// The shut graphic (open is `closed + 1`, but carried explicitly).
    pub closed: u16,
    /// The open graphic.
    pub open: u16,
    /// East/west hinge swing.
    pub offset_x: i16,
    /// North/south hinge swing.
    pub offset_y: i16,
    /// Where.
    pub x: u16,
    /// Where.
    pub y: u16,
    /// Where.
    pub z: i8,
}

/// One placed container — a decoration that opens onto a gump, like a town chest
/// or crate.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DecorContainer {
    /// The item graphic.
    pub graphic: u16,
    /// The gump the client opens for it (from the client's container table).
    pub gump: u16,
    /// Its hue, or 0.
    pub hue: u16,
    /// Where.
    pub x: u16,
    /// Where.
    pub y: u16,
    /// Where.
    pub z: i8,
}

/// One creature a spawn region may put down — the template a
/// [`RegisterSpawner`](Command::RegisterSpawner) carries, mirroring the fields of
/// [`SpawnMobile`](Command::SpawnMobile) minus the position, which the region
/// supplies.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SpawnCreature {
    /// The body graphic.
    pub body: u16,
    /// Its hue.
    pub hue: u16,
    /// Starting and maximum hit points.
    pub hits: u16,
    /// Health-bar colour, as a wire byte.
    pub notoriety: u8,
    /// Melee damage before the target's resistance.
    pub damage: u16,
    /// Physical resistance, 0–100.
    pub resistance: u8,
    /// Ticks between swings; 0 takes the default.
    pub swing: u64,
    /// How far it notices a foe; 0 is passive.
    pub sight: u8,
    /// Whether it starts fights (2), answers them (1), or only runs (0).
    pub aggression: u8,
    /// Ticks between its beats while hunting; 0 takes the shard default.
    pub beat: u64,
    /// How far its ranged attack reaches, in tiles; 0 fights hand to hand.
    pub ranged: u8,
    /// The ranged attack's damage type wire value.
    pub ranged_kind: u8,
    /// Whether it wanders when idle.
    pub wander: bool,
}

/// One line of vendor stock, as `op_stock` supplies it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StockItem {
    /// The goods' graphic.
    pub graphic: u16,
    /// Their hue.
    pub hue: u16,
    /// How many the vendor holds.
    pub amount: u16,
    /// What one unit costs.
    pub price: u32,
    /// The label the client shows.
    pub name: String,
}

/// Why a script call failed.
#[derive(Debug, thiserror::Error)]
pub enum ScriptError {
    /// The script did not compile or threw while evaluating.
    #[error("script evaluation failed: {0}")]
    Evaluate(String),
    /// A hook threw when called.
    #[error("script hook `{hook}` threw: {message}")]
    Hook {
        /// Which hook.
        hook: &'static str,
        /// The exception message.
        message: String,
    },
}

/// A scripting runtime, narrow enough to be swapped.
///
/// The whole surface: evaluate a script (again, to hot-reload), deliver a domain
/// event, run the per-tick hook for one entity, and take the commands the script
/// wants applied. A backend that is not V8 could implement exactly this.
pub trait ScriptEngine {
    /// Evaluate `source`, replacing whatever was loaded before.
    ///
    /// This *is* hot reload: calling it again with new source rebinds the hooks
    /// in the live runtime, no restart. Setup code in the script runs now; the
    /// exported `onTick` / `onEvent` functions are captured for later calls.
    fn load(&mut self, source: &str) -> Result<(), ScriptError>;

    /// Forward a domain event to the script and fold it into the read model.
    ///
    /// Calls the script's `onEvent` if it exports one. Either way the engine's
    /// own view of where mobiles are is updated, so a later [`tick`](Self::tick)
    /// can read it.
    fn deliver(&mut self, event: &Event) -> Result<(), ScriptError>;

    /// Run the per-tick hook for one entity, if the script exports `onTick`.
    ///
    /// Synchronous by contract: this is called from inside a tick, and a tick
    /// never awaits. A hook that reads state does so through a direct op; a hook
    /// that changes the world enqueues a [`Command`].
    fn tick(&mut self, entity: Serial) -> Result<(), ScriptError>;

    /// Take the commands enqueued since the last drain.
    fn take_commands(&mut self) -> Vec<Command>;
}
