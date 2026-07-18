//! The `deno_core` backend for [`ScriptEngine`](crate::ScriptEngine).
//!
//! One [`JsRuntime`], one V8 isolate, owned by value — no static, no singleton,
//! the same rule the rest of the engine lives by. Everything V8 is in this file;
//! nothing above [`crate`] sees a `v8::` type.
//!
//! # Why `execute_script` and not ES modules
//!
//! Loading an ES module in `deno_core` is asynchronous: it drives an event loop
//! to resolve imports. A tick never awaits, and load/reload for the spike needs
//! nothing an event loop offers — there are no imports to resolve yet. So a
//! script is *evaluated* with [`JsRuntime::execute_script`], which is fully
//! synchronous, and the hot path ([`DenoEngine::tick`]) calls the captured hook
//! through raw V8 with no future in sight. Real module resolution is a later,
//! additive change (§6/§7) that does not touch this seam.
//!
//! # How a hook is found
//!
//! The script is wrapped so its body runs inside an arrow that returns whatever
//! `onTick` / `onEvent` it defined. Evaluating the wrapper yields that object;
//! the two functions are captured as `v8::Global` handles and called later.
//! Re-evaluating rebinds them in the same isolate — that is the whole of hot
//! reload.

use std::collections::HashMap;
use std::path::Path;
use std::time::SystemTime;

use deno_core::{extension, op2, v8, JsRuntime, OpState, RuntimeOptions};

use crate::{Command, Event, ScriptEngine, ScriptError, Serial};

/// Where a mobile is, as far as a script can see — the read model the engine
/// keeps up to date from the events it is handed, so a hook reads it without a
/// round-trip into the world.
#[derive(Clone, Copy, Debug, Default)]
struct View {
    x: u16,
    y: u16,
    z: i8,
}

/// The Rust state the ops reach, stored in the runtime's [`OpState`].
///
/// Reads come out of `entities`; writes go into `outbox`. That asymmetry is the
/// engine's whole contract with a script in one struct: look at the world
/// directly, change it only by asking.
#[derive(Default)]
struct Host {
    entities: HashMap<Serial, View>,
    outbox: Vec<Command>,
}

impl Host {
    /// Fold a domain event into the read model. The same event the script's
    /// handler sees also keeps this current — there is no second bookkeeping
    /// path to forget.
    fn apply(&mut self, event: &Event) {
        // By reference, not by value: an `Event` is no longer `Copy` (speech
        // carries a `String`), and the read model only needs the position-bearing
        // events anyway.
        match event {
            Event::PlayerEntered { serial, x, y, z } | Event::MobileSpawned { serial, x, y, z } => {
                self.entities.insert(
                    *serial,
                    View {
                        x: *x,
                        y: *y,
                        z: *z,
                    },
                );
            }
            Event::MobileMoved {
                serial, x, y, z, ..
            } => {
                self.entities.insert(
                    *serial,
                    View {
                        x: *x,
                        y: *y,
                        z: *z,
                    },
                );
            }
            Event::PlayerLeft { serial } => {
                self.entities.remove(serial);
            }
            _ => {}
        }
    }
}

/// Read a mobile's position: `[x, y, z]`, or `null` if the script asked about a
/// serial the engine has never been told about.
///
/// A direct read — the "look at the world" half of the contract. Not a fast op
/// because it returns a structured value; that cost is measured in the
/// benchmark and is the honest cost of a hook that reads state.
#[op2]
#[serde]
fn op_position(state: &mut OpState, serial: u32) -> Option<[i32; 3]> {
    state
        .borrow::<Host>()
        .entities
        .get(&serial)
        .map(|v| [v.x as i32, v.y as i32, v.z as i32])
}

/// Enqueue a move for the world to apply on its next tick.
///
/// The "change it only by asking" half. A fast op: no allocation, no return
/// value, just a push onto the outbox the engine drains after the hooks run.
#[op2(fast)]
fn op_move(state: &mut OpState, serial: u32, direction: u32) {
    state.borrow_mut::<Host>().outbox.push(Command::Move {
        serial,
        direction: direction as u8,
    });
}

/// What a script passes to spawn an item — a plain object, so the JS reads
/// `op_spawn_item({ graphic, x, y })` rather than seven positional arguments,
/// most of which have sensible defaults.
#[derive(serde::Deserialize)]
struct SpawnSpec {
    graphic: u16,
    #[serde(default)]
    hue: u16,
    #[serde(default = "one")]
    amount: u16,
    #[serde(default)]
    stackable: bool,
    x: u16,
    y: u16,
    #[serde(default)]
    z: i8,
    #[serde(default)]
    facet: u8,
}

/// The default stack size: a single item.
fn one() -> u16 {
    1
}

/// Put an item on the ground. Enqueues a command; the world creates the entity
/// and draws it on the tick that applies it.
#[op2]
fn op_spawn_item(state: &mut OpState, #[serde] spec: SpawnSpec) {
    state.borrow_mut::<Host>().outbox.push(Command::SpawnItem {
        graphic: spec.graphic,
        hue: spec.hue,
        amount: spec.amount,
        stackable: spec.stackable,
        x: spec.x,
        y: spec.y,
        z: spec.z,
        facet: spec.facet,
    });
}

/// What a script passes to spawn a container: a [`SpawnSpec`] plus the gump the
/// client opens for it.
#[derive(serde::Deserialize)]
struct ContainerSpec {
    graphic: u16,
    gump: u16,
    #[serde(default)]
    hue: u16,
    x: u16,
    y: u16,
    #[serde(default)]
    z: i8,
    #[serde(default)]
    facet: u8,
}

/// Put a container on the ground.
#[op2]
fn op_spawn_container(state: &mut OpState, #[serde] spec: ContainerSpec) {
    state
        .borrow_mut::<Host>()
        .outbox
        .push(Command::SpawnContainer {
            graphic: spec.graphic,
            gump: spec.gump,
            hue: spec.hue,
            x: spec.x,
            y: spec.y,
            z: spec.z,
            facet: spec.facet,
        });
}

/// What a script passes to spawn a mobile.
#[derive(serde::Deserialize)]
struct MobileSpec {
    body: u16,
    #[serde(default)]
    hue: u16,
    #[serde(default = "one")]
    hits: u16,
    #[serde(default)]
    notoriety: u8,
    #[serde(default)]
    damage: u16,
    #[serde(default)]
    resistance: u8,
    #[serde(default)]
    swing: u64,
    #[serde(default)]
    sight: u8,
    #[serde(default)]
    wander: bool,
    x: u16,
    y: u16,
    #[serde(default)]
    z: i8,
    #[serde(default)]
    facet: u8,
}

/// Put a creature or NPC in the world.
#[op2]
fn op_spawn_mobile(state: &mut OpState, #[serde] spec: MobileSpec) {
    state
        .borrow_mut::<Host>()
        .outbox
        .push(Command::SpawnMobile {
            body: spec.body,
            hue: spec.hue,
            hits: spec.hits,
            notoriety: spec.notoriety,
            damage: spec.damage,
            resistance: spec.resistance,
            swing: spec.swing,
            sight: spec.sight,
            wander: spec.wander,
            x: spec.x,
            y: spec.y,
            z: spec.z,
            facet: spec.facet,
        });
}

/// Deal damage to a mobile, of a kind (0 physical, 1 fire, …).
#[op2(fast)]
fn op_damage(state: &mut OpState, serial: u32, amount: u32, damage_type: u32, by: u32) {
    state.borrow_mut::<Host>().outbox.push(Command::Damage {
        serial,
        amount: amount.min(u32::from(u16::MAX)) as u16,
        damage_type: damage_type.min(u32::from(u8::MAX)) as u8,
        by,
    });
}

/// Heal a mobile, up to its maximum.
#[op2(fast)]
fn op_heal(state: &mut OpState, serial: u32, amount: u32) {
    state.borrow_mut::<Host>().outbox.push(Command::Heal {
        serial,
        amount: amount.min(u32::from(u16::MAX)) as u16,
    });
}

/// What a script passes to cast a spell — a plain object, so reagents can be a
/// list and the fields that default (target, difficulty, pack, reagents) can be
/// left off.
#[derive(serde::Deserialize)]
struct CastSpec {
    serial: u32,
    spell: u16,
    #[serde(default)]
    target: u32,
    mana: u16,
    #[serde(default)]
    difficulty: u16,
    skill: u8,
    #[serde(default)]
    pack: u32,
    /// `(graphic, count)` pairs the spell consumes from `pack`.
    #[serde(default)]
    reagents: Vec<(u16, u16)>,
}

/// Cast a spell. The outcome comes back as a `SpellCast` event, not a return —
/// the mana, reagents and skill roll happen on the tick.
#[op2]
fn op_cast_spell(state: &mut OpState, #[serde] spec: CastSpec) {
    state.borrow_mut::<Host>().outbox.push(Command::CastSpell {
        serial: spec.serial,
        spell: spec.spell,
        target: spec.target,
        mana: spec.mana,
        difficulty: spec.difficulty.min(100),
        skill: spec.skill,
        pack: spec.pack,
        reagents: spec.reagents,
    });
}

/// Set a mobile's skill value, in tenths.
#[op2(fast)]
fn op_set_skill(state: &mut OpState, serial: u32, skill: u32, value: u32) {
    state.borrow_mut::<Host>().outbox.push(Command::SetSkill {
        serial,
        skill: skill as u8,
        value: value.min(u32::from(u16::MAX)) as u16,
    });
}

/// Set a mobile's stats; strength re-caps hits, intelligence re-caps mana.
#[op2(fast)]
fn op_set_stats(
    state: &mut OpState,
    serial: u32,
    strength: u32,
    dexterity: u32,
    intelligence: u32,
) {
    let clamp = |v: u32| v.min(u32::from(u16::MAX)) as u16;
    state.borrow_mut::<Host>().outbox.push(Command::SetStats {
        serial,
        strength: clamp(strength),
        dexterity: clamp(dexterity),
        intelligence: clamp(intelligence),
    });
}

/// Use a skill against a difficulty (0–100). The result comes back as a
/// `SkillUsed` event, not a return value: the roll and any gain happen on the
/// tick, not in the op.
#[op2(fast)]
fn op_use_skill(state: &mut OpState, serial: u32, skill: u32, difficulty: u32) {
    state.borrow_mut::<Host>().outbox.push(Command::UseSkill {
        serial,
        skill: skill as u8,
        difficulty: difficulty.min(100) as u16,
    });
}

/// Make a mobile speak — an NPC's line, a keyword answer.
#[op2(fast)]
fn op_say(state: &mut OpState, serial: u32, #[string] text: String, hue: u32) {
    state.borrow_mut::<Host>().outbox.push(Command::Speak {
        serial,
        hue: hue.min(u32::from(u16::MAX)) as u16,
        text,
    });
}

/// Take control of a mobile: from now on its `onTick` runs it, not the built-in
/// brain. The world starts handing it to this script each tick.
#[op2(fast)]
fn op_control(state: &mut OpState, serial: u32) {
    state
        .borrow_mut::<Host>()
        .outbox
        .push(Command::Control { serial });
}

/// One creature template inside a [`SpawnerSpec`]. Mirrors [`MobileSpec`] minus
/// the position, which the region supplies.
#[derive(serde::Deserialize)]
struct CreatureSpec {
    body: u16,
    #[serde(default)]
    hue: u16,
    #[serde(default = "one")]
    hits: u16,
    #[serde(default)]
    notoriety: u8,
    #[serde(default)]
    damage: u16,
    #[serde(default)]
    resistance: u8,
    #[serde(default)]
    swing: u64,
    #[serde(default)]
    sight: u8,
    #[serde(default)]
    wander: bool,
}

/// A spawn region, from the script: `op_register_spawner({ x, y, width, height,
/// maxCount, respawnDelay, creatures: [...] })`.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpawnerSpec {
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    #[serde(default)]
    facet: u8,
    max_count: u16,
    respawn_delay: u64,
    creatures: Vec<CreatureSpec>,
}

/// Register a spawn region the world will keep populated.
#[op2]
fn op_register_spawner(state: &mut OpState, #[serde] spec: SpawnerSpec) {
    let creatures = spec
        .creatures
        .into_iter()
        .map(|c| crate::SpawnCreature {
            body: c.body,
            hue: c.hue,
            hits: c.hits,
            notoriety: c.notoriety,
            damage: c.damage,
            resistance: c.resistance,
            swing: c.swing,
            sight: c.sight,
            wander: c.wander,
        })
        .collect();
    state
        .borrow_mut::<Host>()
        .outbox
        .push(Command::RegisterSpawner {
            x: spec.x,
            y: spec.y,
            width: spec.width,
            height: spec.height,
            facet: spec.facet,
            max_count: spec.max_count,
            respawn_delay: spec.respawn_delay,
            creatures,
        });
}

/// Remove every spawn region and the creatures they were maintaining.
#[op2(fast)]
fn op_clear_spawners(state: &mut OpState) {
    state
        .borrow_mut::<Host>()
        .outbox
        .push(Command::ClearSpawners);
}

/// One placed decoration in a [`DecorSpec`].
#[derive(serde::Deserialize)]
struct DecorStaticSpec {
    graphic: u16,
    #[serde(default)]
    hue: u16,
    x: u16,
    y: u16,
    #[serde(default)]
    z: i8,
}

/// One placed door in a [`DecorSpec`].
#[derive(serde::Deserialize)]
struct DecorDoorSpec {
    closed: u16,
    open: u16,
    #[serde(default)]
    offset_x: i16,
    #[serde(default)]
    offset_y: i16,
    x: u16,
    y: u16,
    #[serde(default)]
    z: i8,
}

/// One placed container in a [`DecorSpec`].
#[derive(serde::Deserialize)]
struct DecorContainerSpec {
    graphic: u16,
    gump: u16,
    #[serde(default)]
    hue: u16,
    x: u16,
    y: u16,
    #[serde(default)]
    z: i8,
}

/// A batch of decoration:
/// `op_decorate({ facet, statics: [...], doors: [...], containers: [...] })`.
#[derive(serde::Deserialize)]
struct DecorSpec {
    #[serde(default)]
    facet: u8,
    #[serde(default)]
    statics: Vec<DecorStaticSpec>,
    #[serde(default)]
    doors: Vec<DecorDoorSpec>,
    #[serde(default)]
    containers: Vec<DecorContainerSpec>,
}

/// Place a batch of decoration the shard adds on top of the map's art: plain
/// statics, openable doors, and openable containers.
#[op2]
fn op_decorate(state: &mut OpState, #[serde] spec: DecorSpec) {
    let statics = spec
        .statics
        .into_iter()
        .map(|s| crate::DecorStatic {
            graphic: s.graphic,
            hue: s.hue,
            x: s.x,
            y: s.y,
            z: s.z,
        })
        .collect();
    let doors = spec
        .doors
        .into_iter()
        .map(|d| crate::DecorDoor {
            closed: d.closed,
            open: d.open,
            offset_x: d.offset_x,
            offset_y: d.offset_y,
            x: d.x,
            y: d.y,
            z: d.z,
        })
        .collect();
    let containers = spec
        .containers
        .into_iter()
        .map(|c| crate::DecorContainer {
            graphic: c.graphic,
            gump: c.gump,
            hue: c.hue,
            x: c.x,
            y: c.y,
            z: c.z,
        })
        .collect();
    state.borrow_mut::<Host>().outbox.push(Command::Decorate {
        facet: spec.facet,
        statics,
        doors,
        containers,
    });
}

/// Remove every script-placed decoration.
#[op2(fast)]
fn op_clear_decorations(state: &mut OpState) {
    state
        .borrow_mut::<Host>()
        .outbox
        .push(Command::ClearDecorations);
}

extension!(
    openshard_ops,
    ops = [
        op_position,
        op_move,
        op_spawn_item,
        op_spawn_container,
        op_spawn_mobile,
        op_damage,
        op_heal,
        op_cast_spell,
        op_control,
        op_set_stats,
        op_set_skill,
        op_use_skill,
        op_say,
        op_register_spawner,
        op_clear_spawners,
        op_decorate,
        op_clear_decorations
    ],
    docs = "OpenShard's script-facing ops: read entity state, enqueue commands.",
);

/// A `deno_core`-backed [`ScriptEngine`].
pub struct DenoEngine {
    runtime: JsRuntime,
    on_tick: Option<v8::Global<v8::Function>>,
    on_event: Option<v8::Global<v8::Function>>,
    /// The path the script was loaded from, and the mtime seen then. Only set by
    /// [`load_file`](Self::load_file); drives [`reload_if_changed`](Self::reload_if_changed).
    watched: Option<(std::path::PathBuf, SystemTime)>,
}

impl std::fmt::Debug for DenoEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DenoEngine")
            .field("on_tick", &self.on_tick.is_some())
            .field("on_event", &self.on_event.is_some())
            .field("watched", &self.watched.as_ref().map(|(p, _)| p))
            .finish()
    }
}

impl Default for DenoEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl DenoEngine {
    /// A fresh isolate with the ops installed and no script loaded yet.
    pub fn new() -> Self {
        let runtime = JsRuntime::new(RuntimeOptions {
            extensions: vec![openshard_ops::init()],
            ..Default::default()
        });
        runtime.op_state().borrow_mut().put(Host::default());
        Self {
            runtime,
            on_tick: None,
            on_event: None,
            watched: None,
        }
    }

    /// Load a script from a file and remember it for [`reload_if_changed`](Self::reload_if_changed).
    pub fn load_file(
        &mut self,
        path: impl AsRef<Path>,
    ) -> std::io::Result<Result<(), ScriptError>> {
        let path = path.as_ref();
        let source = std::fs::read_to_string(path)?;
        let mtime = std::fs::metadata(path)?.modified()?;
        let loaded = self.load(&source);
        if loaded.is_ok() {
            self.watched = Some((path.to_path_buf(), mtime));
        }
        Ok(loaded)
    }

    /// Reload the watched file if it has changed on disk since it was loaded.
    ///
    /// Returns `Ok(true)` if a reload happened. This is hot reload as a poll: the
    /// caller ticks it between world ticks — no watcher thread, no dependency, no
    /// shared state, and iterating on a hook is save-the-file, not bounce-the-shard.
    pub fn reload_if_changed(&mut self) -> std::io::Result<Result<bool, ScriptError>> {
        let Some((path, seen)) = self.watched.clone() else {
            return Ok(Ok(false));
        };
        let mtime = std::fs::metadata(&path)?.modified()?;
        if mtime == seen {
            return Ok(Ok(false));
        }
        let source = std::fs::read_to_string(&path)?;
        match self.load(&source) {
            Ok(()) => {
                self.watched = Some((path, mtime));
                Ok(Ok(true))
            }
            Err(e) => Ok(Err(e)),
        }
    }

    /// The captured hook of a given kind, cloned so the isolate can be borrowed
    /// mutably for the call without the borrow checker seeing the handle and the
    /// runtime as one borrow.
    fn hook(&self, which: Hook) -> Option<v8::Global<v8::Function>> {
        match which {
            Hook::Tick => self.on_tick.clone(),
            Hook::Event => self.on_event.clone(),
        }
    }
}

/// Which exported function a call is for — only used to name it in an error.
#[derive(Clone, Copy)]
enum Hook {
    Tick,
    Event,
}

impl Hook {
    const fn name(self) -> &'static str {
        match self {
            Hook::Tick => "onTick",
            Hook::Event => "onEvent",
        }
    }
}

/// Wrap a script so evaluating it yields the object of hooks it defined. The
/// body runs inside an arrow; `typeof` is the one reference that is safe on a
/// name the script never declared, so an absent hook comes back `undefined`.
fn wrap(source: &str) -> String {
    format!(
        "(()=>{{\n{source}\n;return{{\
         onTick:typeof onTick===\"function\"?onTick:undefined,\
         onEvent:typeof onEvent===\"function\"?onEvent:undefined\
         }};}})()"
    )
}

/// Pull a named function property off the hooks object, as a `Global`.
fn capture(
    scope: &mut v8::PinScope<'_, '_>,
    obj: v8::Local<'_, v8::Object>,
    name: &str,
) -> Option<v8::Global<v8::Function>> {
    let key = v8::String::new(scope, name)?;
    let value = obj.get(scope, key.into())?;
    let function: v8::Local<'_, v8::Function> = value.try_into().ok()?;
    Some(v8::Global::new(scope, function))
}

impl ScriptEngine for DenoEngine {
    fn load(&mut self, source: &str) -> Result<(), ScriptError> {
        let result = self
            .runtime
            .execute_script("[openshard:script]", wrap(source))
            .map_err(|e| ScriptError::Evaluate(e.to_string()))?;

        let context = self.runtime.main_context();
        let isolate = self.runtime.v8_isolate();
        v8::scope_with_context!(scope, isolate, context);
        let value = v8::Local::new(scope, result);
        let obj: v8::Local<'_, v8::Object> = value.try_into().map_err(|_| {
            ScriptError::Evaluate("script did not evaluate to an object".to_owned())
        })?;
        // Replace, never merge: a reload that dropped a hook should lose it, not
        // keep calling the stale one.
        self.on_tick = capture(scope, obj, "onTick");
        self.on_event = capture(scope, obj, "onEvent");
        Ok(())
    }

    fn deliver(&mut self, event: &Event) -> Result<(), ScriptError> {
        // Fold into the read model first, borrow dropped before any JS runs so
        // the op that reads `Host` can borrow it in turn.
        self.runtime
            .op_state()
            .borrow_mut()
            .borrow_mut::<Host>()
            .apply(event);

        let Some(func) = self.hook(Hook::Event) else {
            return Ok(());
        };
        // Cloned, not copied: an `Event` owns its words now. Cheap on the sparse
        // event path, and V8 needs an owned value to serialise anyway.
        let event = event.clone();
        let context = self.runtime.main_context();
        let isolate = self.runtime.v8_isolate();
        v8::scope_with_context!(scope, isolate, context);
        v8::tc_scope!(let tc, scope);
        // Sparse path — events are rare next to ticks — so a serde round-trip
        // into a plain object is fine and keeps the shape readable from JS as
        // `e.type`, `e.serial`, ….
        let arg =
            deno_core::serde_v8::to_v8(tc, event).unwrap_or_else(|_| v8::undefined(tc).into());
        let f = v8::Local::new(tc, &func);
        let recv = v8::undefined(tc).into();
        if f.call(tc, recv, &[arg]).is_none() {
            let message = tc
                .exception()
                .map(|e| e.to_rust_string_lossy(tc))
                .unwrap_or_else(|| "unknown error".to_owned());
            return Err(ScriptError::Hook {
                hook: Hook::Event.name(),
                message,
            });
        }
        Ok(())
    }

    fn tick(&mut self, entity: Serial) -> Result<(), ScriptError> {
        let Some(func) = self.hook(Hook::Tick) else {
            return Ok(());
        };
        let context = self.runtime.main_context();
        let isolate = self.runtime.v8_isolate();
        v8::scope_with_context!(scope, isolate, context);
        v8::tc_scope!(let tc, scope);
        let arg = v8::Integer::new_from_unsigned(tc, entity).into();
        let f = v8::Local::new(tc, &func);
        let recv = v8::undefined(tc).into();
        if f.call(tc, recv, &[arg]).is_none() {
            let message = tc
                .exception()
                .map(|e| e.to_rust_string_lossy(tc))
                .unwrap_or_else(|| "unknown error".to_owned());
            return Err(ScriptError::Hook {
                hook: Hook::Tick.name(),
                message,
            });
        }
        Ok(())
    }

    fn take_commands(&mut self) -> Vec<Command> {
        std::mem::take(
            &mut self
                .runtime
                .op_state()
                .borrow_mut()
                .borrow_mut::<Host>()
                .outbox,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_script_with_no_hooks_loads_and_ticks_to_nothing() {
        // The empty case has to be silent, not a panic: a script may only care
        // about events, or only be a stub during development.
        let mut engine = DenoEngine::new();
        engine.load("const answer = 42;").unwrap();
        engine.tick(1).unwrap();
        assert!(engine.take_commands().is_empty());
    }

    #[test]
    fn a_tick_hook_reads_position_and_enqueues_a_command() {
        // The whole seam in one test: an event feeds the read model, the hook
        // reads it through an op, and acts by enqueuing — never touching state.
        let mut engine = DenoEngine::new();
        engine
            .load(
                "function onTick(serial) {\n\
                 const p = Deno.core.ops.op_position(serial);\n\
                 if (p !== null && p[0] === 100) Deno.core.ops.op_move(serial, 2);\n\
                 }",
            )
            .unwrap();

        engine
            .deliver(&Event::PlayerEntered {
                serial: 7,
                x: 100,
                y: 200,
                z: 0,
            })
            .unwrap();
        engine.tick(7).unwrap();

        assert_eq!(
            engine.take_commands(),
            vec![Command::Move {
                serial: 7,
                direction: 2
            }]
        );
        // Drained, not duplicated.
        assert!(engine.take_commands().is_empty());
    }

    #[test]
    fn a_moved_event_updates_what_a_hook_sees() {
        // The read model tracks the world: after a move, the op reports the new
        // tile, not the one the mobile entered on.
        let mut engine = DenoEngine::new();
        engine
            .load(
                "function onTick(serial) {\n\
                 const p = Deno.core.ops.op_position(serial);\n\
                 Deno.core.ops.op_move(serial, p[0] & 7);\n\
                 }",
            )
            .unwrap();
        engine
            .deliver(&Event::PlayerEntered {
                serial: 1,
                x: 0,
                y: 0,
                z: 0,
            })
            .unwrap();
        engine
            .deliver(&Event::MobileMoved {
                serial: 1,
                x: 5,
                y: 0,
                z: 0,
                facing: 1,
            })
            .unwrap();
        engine.tick(1).unwrap();
        assert_eq!(
            engine.take_commands(),
            vec![Command::Move {
                serial: 1,
                direction: 5
            }]
        );
    }

    #[test]
    fn a_left_event_removes_the_entity_from_the_read_model() {
        let mut engine = DenoEngine::new();
        engine
            .load(
                "function onTick(serial) {\n\
                 if (Deno.core.ops.op_position(serial) === null) Deno.core.ops.op_move(serial, 0);\n\
                 }",
            )
            .unwrap();
        engine
            .deliver(&Event::PlayerEntered {
                serial: 3,
                x: 1,
                y: 1,
                z: 0,
            })
            .unwrap();
        engine.deliver(&Event::PlayerLeft { serial: 3 }).unwrap();
        engine.tick(3).unwrap();
        // Gone, so the hook saw `null` and reacted.
        assert_eq!(engine.take_commands().len(), 1);
    }

    #[test]
    fn a_hook_can_spawn_an_item() {
        // The other command a script can emit: put a thing on the ground. The
        // spec object's defaults mean a hook names only what it cares about.
        let mut engine = DenoEngine::new();
        engine
            .load(
                "function onEvent(e) {\n\
                 if (e.type === 'PlayerEntered') {\n\
                     Deno.core.ops.op_spawn_item({ graphic: 0x0EED, x: e.serial & 0xFFFF, y: 100 });\n\
                 }\n\
                 }",
            )
            .unwrap();
        engine
            .deliver(&Event::PlayerEntered {
                serial: 42,
                x: 0,
                y: 0,
                z: 0,
            })
            .unwrap();
        assert_eq!(
            engine.take_commands(),
            vec![Command::SpawnItem {
                graphic: 0x0EED,
                hue: 0,
                amount: 1,
                stackable: false,
                x: 42,
                y: 100,
                z: 0,
                facet: 0,
            }]
        );
    }

    #[test]
    fn an_admin_action_can_register_a_spawner() {
        // The pack-driven spawn path: a staff button emits AdminAction, and the
        // pack turns the verb into a spawn region through op_register_spawner.
        let mut engine = DenoEngine::new();
        engine
            .load(
                "function onEvent(e) {\n\
                 if (e.type === 'AdminAction' && e.action === 'populate:cemetery') {\n\
                     Deno.core.ops.op_register_spawner({ x: 1349, y: 1455, width: 40, height: 40,\n\
                         maxCount: 4, respawnDelay: 60,\n\
                         creatures: [{ body: 0x0032, hits: 34, notoriety: 6, sight: 8 }] });\n\
                 }\n\
                 }",
            )
            .unwrap();
        engine
            .deliver(&Event::AdminAction {
                serial: 1,
                action: "populate:cemetery".to_owned(),
            })
            .unwrap();
        assert_eq!(
            engine.take_commands(),
            vec![Command::RegisterSpawner {
                x: 1349,
                y: 1455,
                width: 40,
                height: 40,
                facet: 0,
                max_count: 4,
                respawn_delay: 60,
                creatures: vec![crate::SpawnCreature {
                    body: 0x0032,
                    hue: 0,
                    hits: 34,
                    notoriety: 6,
                    damage: 0,
                    resistance: 0,
                    swing: 0,
                    sight: 8,
                    wander: false,
                }],
            }]
        );
    }

    #[test]
    fn a_hook_can_decorate() {
        // The pack places a batch of statics on top of the map's art.
        let mut engine = DenoEngine::new();
        engine
            .load(
                "function onEvent(e) {\n\
                 if (e.type === 'AdminAction' && e.action === 'decorate:britain') {\n\
                     Deno.core.ops.op_decorate({ statics: [\n\
                         { graphic: 0x07C1, x: 1436, y: 1559, z: 30 },\n\
                         { graphic: 0x08DA, x: 1424, y: 1715, z: 20 }] });\n\
                 }\n\
                 }",
            )
            .unwrap();
        engine
            .deliver(&Event::AdminAction {
                serial: 1,
                action: "decorate:britain".to_owned(),
            })
            .unwrap();
        assert_eq!(
            engine.take_commands(),
            vec![Command::Decorate {
                facet: 0,
                statics: vec![
                    crate::DecorStatic {
                        graphic: 0x07C1,
                        hue: 0,
                        x: 1436,
                        y: 1559,
                        z: 30
                    },
                    crate::DecorStatic {
                        graphic: 0x08DA,
                        hue: 0,
                        x: 1424,
                        y: 1715,
                        z: 20
                    },
                ],
                doors: vec![],
                containers: vec![],
            }]
        );
    }

    #[test]
    fn a_hook_can_place_doors_and_containers() {
        // The same op carries the interactive decoration: a door (its graphics and
        // hinge offset already resolved) and a container with its gump.
        let mut engine = DenoEngine::new();
        engine
            .load(
                "function onEvent(e) {\n\
                 if (e.type === 'AdminAction' && e.action === 'decorate:britain') {\n\
                     Deno.core.ops.op_decorate({\n\
                         doors: [{ closed: 0x0675, open: 0x0676, offset_x: -1, offset_y: 1, x: 1411, y: 1621, z: 30 }],\n\
                         containers: [{ graphic: 0x0E42, gump: 0x49, x: 1500, y: 1600, z: 0 }] });\n\
                 }\n\
                 }",
            )
            .unwrap();
        engine
            .deliver(&Event::AdminAction {
                serial: 1,
                action: "decorate:britain".to_owned(),
            })
            .unwrap();
        assert_eq!(
            engine.take_commands(),
            vec![Command::Decorate {
                facet: 0,
                statics: vec![],
                doors: vec![crate::DecorDoor {
                    closed: 0x0675,
                    open: 0x0676,
                    offset_x: -1,
                    offset_y: 1,
                    x: 1411,
                    y: 1621,
                    z: 30,
                }],
                containers: vec![crate::DecorContainer {
                    graphic: 0x0E42,
                    gump: 0x49,
                    hue: 0,
                    x: 1500,
                    y: 1600,
                    z: 0,
                }],
            }]
        );
    }

    #[test]
    fn a_hook_can_set_stats() {
        // A script fitting out a fresh mobile hands the world all three stats in
        // one op; the tick is what turns strength into hit points.
        let mut engine = DenoEngine::new();
        engine
            .load(
                "function onEvent(e) {\n\
                 if (e.type === 'PlayerEntered') {\n\
                     Deno.core.ops.op_set_stats(e.serial, 60, 80, 40);\n\
                 }\n\
                 }",
            )
            .unwrap();
        engine
            .deliver(&Event::PlayerEntered {
                serial: 7,
                x: 0,
                y: 0,
                z: 0,
            })
            .unwrap();
        assert_eq!(
            engine.take_commands(),
            vec![Command::SetStats {
                serial: 7,
                strength: 60,
                dexterity: 80,
                intelligence: 40,
            }]
        );
    }

    #[test]
    fn an_event_hook_receives_a_typed_object() {
        // `onEvent` gets the event as a plain object it can switch on.
        let mut engine = DenoEngine::new();
        engine
            .load(
                "function onEvent(e) {\n\
                 if (e.type === \"StepRefused\" && e.reason === 2) Deno.core.ops.op_move(e.serial, 4);\n\
                 }",
            )
            .unwrap();
        engine
            .deliver(&Event::StepRefused {
                serial: 9,
                reason: 2,
            })
            .unwrap();
        assert_eq!(
            engine.take_commands(),
            vec![Command::Move {
                serial: 9,
                direction: 4
            }]
        );
    }

    #[test]
    fn reloading_rebinds_the_hook_in_the_live_isolate() {
        // Hot reload's core claim: the second load replaces the first hook's
        // behaviour without a new isolate.
        let mut engine = DenoEngine::new();
        engine
            .load("function onTick(s) { Deno.core.ops.op_move(s, 1); }")
            .unwrap();
        engine.tick(1).unwrap();
        assert_eq!(
            engine.take_commands()[0],
            Command::Move {
                serial: 1,
                direction: 1
            }
        );

        engine
            .load("function onTick(s) { Deno.core.ops.op_move(s, 6); }")
            .unwrap();
        engine.tick(1).unwrap();
        assert_eq!(
            engine.take_commands()[0],
            Command::Move {
                serial: 1,
                direction: 6
            }
        );
    }

    #[test]
    fn reloading_a_script_that_drops_a_hook_stops_calling_it() {
        // Replace, not merge: after a reload with no `onTick`, ticking is silent.
        let mut engine = DenoEngine::new();
        engine
            .load("function onTick(s) { Deno.core.ops.op_move(s, 1); }")
            .unwrap();
        engine.load("const x = 1;").unwrap();
        engine.tick(1).unwrap();
        assert!(engine.take_commands().is_empty());
    }

    #[test]
    fn a_throwing_hook_is_an_error_not_a_crash() {
        // A script bug drops that call, it does not take the shard down.
        let mut engine = DenoEngine::new();
        engine
            .load("function onTick() { throw new Error(\"boom\"); }")
            .unwrap();
        let err = engine.tick(1).unwrap_err();
        assert!(matches!(err, ScriptError::Hook { hook: "onTick", .. }));
    }

    #[test]
    fn a_syntax_error_is_reported_by_load() {
        let mut engine = DenoEngine::new();
        assert!(matches!(
            engine.load("function ("),
            Err(ScriptError::Evaluate(_))
        ));
    }

    #[test]
    fn reload_if_changed_picks_up_a_file_edit() {
        use std::io::Write;
        let dir = std::env::temp_dir();
        let path = dir.join(format!("openshard-hotreload-{}.js", std::process::id()));
        std::fs::write(&path, "function onTick(s){Deno.core.ops.op_move(s,1);}").unwrap();

        let mut engine = DenoEngine::new();
        engine.load_file(&path).unwrap().unwrap();
        engine.tick(1).unwrap();
        assert_eq!(engine.take_commands()[0].direction(), 1);

        // No change yet: nothing reloads.
        assert!(!engine.reload_if_changed().unwrap().unwrap());

        // An edit with a distinct mtime; sleep so the filesystem clock ticks.
        std::thread::sleep(std::time::Duration::from_millis(10));
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"function onTick(s){Deno.core.ops.op_move(s,7);}")
            .unwrap();
        f.sync_all().unwrap();

        assert!(engine.reload_if_changed().unwrap().unwrap());
        engine.tick(1).unwrap();
        assert_eq!(engine.take_commands()[0].direction(), 7);

        let _ = std::fs::remove_file(&path);
    }

    impl Command {
        fn direction(&self) -> u8 {
            match self {
                Command::Move { direction, .. } => *direction,
                other => panic!("expected a Move, got {other:?}"),
            }
        }
    }
}
