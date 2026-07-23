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
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use deno_core::{extension, op2, v8, JsRuntime, OpState, RuntimeOptions};

use crate::{Command, Event, ScriptEngine, ScriptError, Serial};

mod host;
mod ops;

use host::Host;
use ops::openshard_ops;

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

    /// Load a script and remember it for [`reload_if_changed`](Self::reload_if_changed).
    ///
    /// `path` is either a single script file or a **directory**: a whole pack, whose
    /// `.js` files are concatenated (see [`read_pack`]). A directory lets a shard
    /// split its data into folders by place and facet, the way Sphere's scriptpack
    /// is many files — the engine still evaluates one script, so hot reload and the
    /// single isolate are unchanged.
    pub fn load_file(
        &mut self,
        path: impl AsRef<Path>,
    ) -> std::io::Result<Result<(), ScriptError>> {
        let path = path.as_ref();
        let (source, mtime) = read_pack(path)?;
        let loaded = self.load(&source);
        if loaded.is_ok() {
            self.watched = Some((path.to_path_buf(), mtime));
        }
        Ok(loaded)
    }

    /// Reload the watched script if anything under it has changed on disk since it
    /// was loaded.
    ///
    /// Returns `Ok(true)` if a reload happened. This is hot reload as a poll: the
    /// caller ticks it between world ticks — no watcher thread, no dependency, no
    /// shared state, and iterating on a hook is save-the-file, not bounce-the-shard.
    /// For a directory pack, the change signal is the newest modification time
    /// across the whole tree, so saving any file in the pack reloads it.
    pub fn reload_if_changed(&mut self) -> std::io::Result<Result<bool, ScriptError>> {
        let Some((path, seen)) = self.watched.clone() else {
            return Ok(Ok(false));
        };
        let mtime = newest_mtime(&path)?;
        if mtime == seen {
            return Ok(Ok(false));
        }
        let (source, mtime) = read_pack(&path)?;
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

/// Read a pack into one script and the newest mtime across it.
///
/// A single file is itself; a directory is every `.js` under it (recursively),
/// concatenated in path order with `index.js` files last — so a data file that
/// registers spawns or decoration runs before the `index.js` that wires `onEvent`
/// over them. The files share one script scope, so the pack convention is to
/// register into a `globalThis` namespace rather than collide on top-level names.
fn read_pack(path: &Path) -> std::io::Result<(String, SystemTime)> {
    if !path.is_dir() {
        return Ok((std::fs::read_to_string(path)?, mtime_of(path)?));
    }
    let mut files = Vec::new();
    collect_js(path, &mut files)?;
    files.sort();
    // A stable sort by "is it an index.js" floats those to the end while keeping
    // the alphabetical order within each group.
    files.sort_by_key(|p| p.file_name().is_some_and(|n| n == "index.js"));

    let mut source = String::new();
    let mut newest = SystemTime::UNIX_EPOCH;
    for file in &files {
        newest = newest.max(mtime_of(file)?);
        source.push_str("\n;\n");
        source.push_str(&std::fs::read_to_string(file)?);
    }
    Ok((source, newest))
}

/// The newest modification time across a pack — a single file's, or the latest of
/// every `.js` under a directory. Cheap enough to poll: it stats, it does not read.
fn newest_mtime(path: &Path) -> std::io::Result<SystemTime> {
    if !path.is_dir() {
        return mtime_of(path);
    }
    let mut files = Vec::new();
    collect_js(path, &mut files)?;
    let mut newest = SystemTime::UNIX_EPOCH;
    for file in &files {
        newest = newest.max(mtime_of(file)?);
    }
    Ok(newest)
}

/// One file's modification time.
fn mtime_of(path: &Path) -> std::io::Result<SystemTime> {
    std::fs::metadata(path)?.modified()
}

/// Collect every `.js` file under `dir`, recursively, into `out`.
fn collect_js(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_js(&path, out)?;
        } else if path.extension().is_some_and(|ext| ext == "js") {
            out.push(path);
        }
    }
    Ok(())
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
                    aggression: 2,
                    beat: 0,
                    ranged: 0,
                    ranged_kind: 0,
                    wander: false,
                }],
            }]
        );
    }

    #[test]
    fn an_admin_action_can_clear_spawners() {
        // The "Clear spawns" button: a staff verb the pack turns into a wipe
        // through op_clear_spawners, the mirror of the populate path above.
        let mut engine = DenoEngine::new();
        engine
            .load(
                "function onEvent(e) {\n\
                 if (e.type === 'AdminAction' && e.action === 'clear') {\n\
                     Deno.core.ops.op_clear_spawners();\n\
                 }\n\
                 }",
            )
            .unwrap();
        engine
            .deliver(&Event::AdminAction {
                serial: 1,
                action: "clear".to_owned(),
            })
            .unwrap();
        assert_eq!(engine.take_commands(), vec![Command::ClearSpawners]);
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
    fn an_item_use_trigger_reaches_the_hook_by_graphic() {
        // The item-trigger seam from the script's side: `onEvent` sees an
        // `ItemUsed` as a plain object it switches on by `graphic`, and the `by`
        // and `item` serials cross the serde boundary intact.
        let mut engine = DenoEngine::new();
        engine
            .load(
                "function onEvent(e) {\n\
                 if (e.type === \"ItemUsed\" && e.graphic === 3854)\n\
                 Deno.core.ops.op_say(e.by, \"drink \" + e.item, 0);\n\
                 }",
            )
            .unwrap();
        engine
            .deliver(&Event::ItemUsed {
                item: 0x4000_0007,
                graphic: 3854,
                by: 42,
            })
            .unwrap();
        assert_eq!(
            engine.take_commands(),
            vec![Command::Speak {
                serial: 42,
                hue: 0,
                text: format!("drink {}", 0x4000_0007u32),
            }]
        );
    }

    #[test]
    fn a_corpse_hook_fills_by_body_through_op_add_loot() {
        // The loot seam from the script's side: `onEvent` sees a `CorpseCreated`
        // as a plain object, switches on `body`, and enqueues `op_add_loot` to
        // fill the corpse by serial — the "default in core, customise in pack"
        // split for loot.
        let mut engine = DenoEngine::new();
        engine
            .load(
                "function onEvent(e) {\n\
                 if (e.type === \"CorpseCreated\" && e.body === 400)\n\
                 Deno.core.ops.op_add_loot(e.corpse, 3823, 0, 25, true);\n\
                 }",
            )
            .unwrap();
        engine
            .deliver(&Event::CorpseCreated {
                corpse: 0x4000_0009,
                body: 400,
            })
            .unwrap();
        assert_eq!(
            engine.take_commands(),
            vec![Command::AddLoot {
                container: 0x4000_0009,
                graphic: 3823,
                hue: 0,
                amount: 25,
                stackable: true,
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

    #[test]
    fn a_pack_directory_is_concatenated_with_index_last() {
        // A data file in a subfolder registers into `globalThis.Pack`; the
        // top-level index.js — which must run last — reads it in `onEvent`. Loading
        // the directory concatenates them in that order.
        let dir = std::env::temp_dir().join(format!("openshard-pack-{}", std::process::id()));
        let sub = dir.join("felucca").join("britain");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(
            sub.join("deco.js"),
            "globalThis.Pack = globalThis.Pack || { moves: {} };\n\
             Pack.moves['go'] = 5;",
        )
        .unwrap();
        std::fs::write(
            dir.join("index.js"),
            "function onEvent(e){ Deno.core.ops.op_move(e.serial, globalThis.Pack.moves['go']); }",
        )
        .unwrap();

        let mut engine = DenoEngine::new();
        engine.load_file(&dir).unwrap().unwrap();
        engine
            .deliver(&Event::AdminAction {
                serial: 1,
                action: "go".to_owned(),
            })
            .unwrap();
        assert_eq!(
            engine.take_commands()[0].direction(),
            5,
            "index.js read what the data file in the subfolder registered"
        );

        let _ = std::fs::remove_dir_all(&dir);
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
