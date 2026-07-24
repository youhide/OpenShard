//! The seam between the world's tick and the gameplay script.
//!
//! Neither `openshard-world` nor `openshard-scripting` knows the other exists,
//! and that is deliberate: the world emits domain events and applies commands,
//! the script consumes events and emits commands, and each is written as if the
//! other were any consumer of the same bus. This module is the glue that makes
//! them the *same* consumer — it reads what the world said happened, hands it to
//! the script, and queues what the script asks for back onto the world.
//!
//! It lives in the binary rather than a crate because wiring two crates that
//! must not depend on each other is exactly what the server is for. The one
//! thing here that is more than translation — mapping the world's rich
//! [`Serial`](openshard_world)/`Point` types down to the wire integers the
//! script speaks — is the price of that decoupling, and it is cheap.

use openshard_events::Cursor;
use openshard_scripting::{
    Command as ScriptCommand, DenoEngine, Event as ScriptEvent, ScriptEngine,
};
use openshard_world::events::{
    AdminMenuAction, CorpseCreated, GumpAnswered, MobileMoved, MobileSpawned, PlayerEntered,
    PlayerLeft, SpellRequested, StepRefused,
};
use openshard_world::{
    Command, ItemUsed, ItemsTaken, MobileDied, MobileSpoke, MobileUsed, SkillUsed, SpellCast, World,
};
use tracing::{error, info, warn};

/// The gameplay script, driven around the world's tick.
pub struct Scripts {
    engine: DenoEngine,
    entered: Cursor<PlayerEntered>,
    spawned: Cursor<MobileSpawned>,
    cast_requested: Cursor<SpellRequested>,
    moved: Cursor<MobileMoved>,
    refused: Cursor<StepRefused>,
    left: Cursor<PlayerLeft>,
    died: Cursor<MobileDied>,
    used: Cursor<SkillUsed>,
    cast: Cursor<SpellCast>,
    spoke: Cursor<MobileSpoke>,
    item_used: Cursor<ItemUsed>,
    mobile_used: Cursor<MobileUsed>,
    items_taken: Cursor<ItemsTaken>,
    corpse: Cursor<CorpseCreated>,
    admin: Cursor<AdminMenuAction>,
    gump: Cursor<GumpAnswered>,
}

impl Scripts {
    /// Load the configured script and take cursors into the world's bus.
    ///
    /// `None` when no script is configured — the shard runs without one, the
    /// same way it runs without a map. A script that fails to load is logged and
    /// yields `None` too: a syntax error in a hook drops scripting, it does not
    /// stop the shard from letting people in.
    pub fn load(path: &str, world: &World) -> Option<Self> {
        let path = path.trim();
        if path.is_empty() {
            warn!(
                "no gameplay script configured (scripting.main is empty); \
                 nothing reacts on its own"
            );
            return None;
        }
        let mut engine = DenoEngine::new();
        match engine.load_file(path) {
            Ok(Ok(())) => info!(script = path, "loaded gameplay script"),
            Ok(Err(error)) => {
                error!(script = path, %error, "gameplay script failed to load; running scriptless");
                return None;
            }
            Err(error) => {
                error!(script = path, %error, "could not read gameplay script; running scriptless");
                return None;
            }
        }
        // Cursors taken now, before the first tick, so the script sees every
        // event from here on and none from before it existed.
        Some(Self {
            entered: world.bus().cursor(),
            spawned: world.bus().cursor(),
            cast_requested: world.bus().cursor(),
            moved: world.bus().cursor(),
            refused: world.bus().cursor(),
            left: world.bus().cursor(),
            died: world.bus().cursor(),
            used: world.bus().cursor(),
            cast: world.bus().cursor(),
            spoke: world.bus().cursor(),
            item_used: world.bus().cursor(),
            mobile_used: world.bus().cursor(),
            items_taken: world.bus().cursor(),
            corpse: world.bus().cursor(),
            admin: world.bus().cursor(),
            gump: world.bus().cursor(),
            engine,
        })
    }

    /// One turn of the seam, run right after `world.tick()`.
    ///
    /// The events it reads were emitted this tick; the commands it queues are
    /// applied next tick — the same one-tick deferral every writer of the world
    /// lives by, which is what keeps a script from writing the world out from
    /// under the tick that is running.
    pub fn pump(&mut self, world: &mut World) {
        // Collect first, so the bus borrow is dropped before the engine runs —
        // an op re-borrows the world-facing state and must not find it held.
        // Per event type, because the bus keeps a queue per type; cross-type
        // order is not preserved, and no hook here depends on it.
        let mut events: Vec<ScriptEvent> = Vec::new();
        {
            let bus = world.bus();
            for e in bus.read(&mut self.entered) {
                events.push(ScriptEvent::PlayerEntered {
                    serial: e.serial.raw(),
                    x: e.position.x,
                    y: e.position.y,
                    z: e.position.z,
                });
                // Hand back the saved quest log so the pack rebuilds its state.
                // Read straight off the entity — no new world event, no cursor.
                let blob = world
                    .registry()
                    .entity_of(e.serial)
                    .and_then(|ent| {
                        world
                            .registry()
                            .get::<openshard_world::components::QuestLog>(ent)
                    })
                    .map(|q| q.0.clone());
                if let Some(blob) = blob {
                    if !blob.is_empty() {
                        events.push(ScriptEvent::QuestLoaded {
                            serial: e.serial.raw(),
                            blob,
                        });
                    }
                }
            }
            for e in bus.read(&mut self.spawned) {
                events.push(ScriptEvent::MobileSpawned {
                    serial: e.serial.raw(),
                    x: e.position.x,
                    y: e.position.y,
                    z: e.position.z,
                });
            }
            for e in bus.read(&mut self.cast_requested) {
                events.push(ScriptEvent::SpellRequested {
                    serial: e.serial.raw(),
                    spell: e.spell,
                });
            }
            for e in bus.read(&mut self.moved) {
                events.push(ScriptEvent::MobileMoved {
                    serial: e.serial.raw(),
                    x: e.to.x,
                    y: e.to.y,
                    z: e.to.z,
                    facing: e.facing.direction.to_bits(),
                });
            }
            for e in bus.read(&mut self.refused) {
                events.push(ScriptEvent::StepRefused {
                    serial: e.serial.raw(),
                    reason: e.reason as u8,
                });
            }
            for e in bus.read(&mut self.left) {
                events.push(ScriptEvent::PlayerLeft {
                    serial: e.serial.raw(),
                });
            }
            for e in bus.read(&mut self.died) {
                events.push(ScriptEvent::MobileDied {
                    serial: e.serial.raw(),
                    body: e.body,
                    killer: e.killer.map_or(0, |k| k.raw()),
                });
            }
            for e in bus.read(&mut self.used) {
                events.push(ScriptEvent::SkillUsed {
                    serial: e.serial.raw(),
                    skill: e.skill,
                    success: e.success,
                    value: e.value,
                });
            }
            for e in bus.read(&mut self.cast) {
                events.push(ScriptEvent::SpellCast {
                    serial: e.serial.raw(),
                    spell: e.spell,
                    target: e.target,
                    success: e.success,
                });
            }
            for e in bus.read(&mut self.spoke) {
                events.push(ScriptEvent::MobileSpoke {
                    serial: e.serial.raw(),
                    text: e.text.clone(),
                });
            }
            for e in bus.read(&mut self.item_used) {
                events.push(ScriptEvent::ItemUsed {
                    item: e.item.raw(),
                    graphic: e.graphic,
                    by: e.by.raw(),
                });
            }
            for e in bus.read(&mut self.mobile_used) {
                events.push(ScriptEvent::MobileUsed {
                    mobile: e.mobile.raw(),
                    body: e.body,
                    by: e.by.raw(),
                });
            }
            for e in bus.read(&mut self.items_taken) {
                events.push(ScriptEvent::ItemsTaken {
                    player: e.player.raw(),
                    graphic: e.graphic,
                    taken: e.taken,
                });
            }
            for e in bus.read(&mut self.corpse) {
                events.push(ScriptEvent::CorpseCreated {
                    corpse: e.corpse.raw(),
                    body: e.body,
                });
            }
            for e in bus.read(&mut self.admin) {
                events.push(ScriptEvent::AdminAction {
                    serial: e.serial.raw(),
                    action: e.action.clone(),
                });
            }
            for e in bus.read(&mut self.gump) {
                events.push(ScriptEvent::GumpAnswered {
                    serial: e.serial.raw(),
                    gump_id: e.gump_id,
                    button: e.button,
                    text: e.text_entries.iter().map(|(_, s)| s.clone()).collect(),
                });
            }
        }

        for event in &events {
            if let Err(error) = self.engine.deliver(event) {
                warn!(%error, "gameplay script event handler threw");
            }
        }

        // Then the per-mobile beat: every mobile a script has taken control of
        // gets its `onTick`, the read model already brought current by the events
        // above. This is the hook the scripting benchmark sized — one call per
        // controlled mobile per tick.
        for serial in world.scripted() {
            if let Err(error) = self.engine.tick(serial.raw()) {
                warn!(%error, serial = serial.raw(), "gameplay script onTick threw");
            }
        }

        for command in self.engine.take_commands() {
            world.queue(into_world(command));
        }

        match self.engine.reload_if_changed() {
            Ok(Ok(true)) => info!("reloaded gameplay script"),
            Ok(Ok(false)) => {}
            Ok(Err(error)) => {
                warn!(%error, "edited gameplay script failed to reload; keeping the running one")
            }
            Err(error) => warn!(%error, "could not re-read the gameplay script file"),
        }
    }
}

/// Turn a script's command into the world's. The one place the two vocabularies
/// meet, and the seam where §6 will grow: a new script command lands here.
fn into_world(command: ScriptCommand) -> Command {
    match command {
        ScriptCommand::Move { serial, direction } => Command::Step { serial, direction },
        ScriptCommand::SpawnItem {
            graphic,
            hue,
            amount,
            stackable,
            x,
            y,
            z,
            facet,
        } => Command::SpawnItem {
            graphic,
            hue,
            amount,
            stackable,
            position: openshard_protocol::Point::new(x, y, z),
            facet,
        },
        ScriptCommand::SpawnContainer {
            graphic,
            gump,
            hue,
            x,
            y,
            z,
            facet,
        } => Command::SpawnContainer {
            graphic,
            gump,
            hue,
            position: openshard_protocol::Point::new(x, y, z),
            facet,
        },
        ScriptCommand::SpawnMobile {
            body,
            hue,
            hits,
            notoriety,
            damage,
            resistance,
            swing,
            sight,
            aggression,
            beat,
            ranged,
            ranged_kind,
            wander,
            x,
            y,
            z,
            facet,
            name,
            banker,
            vendor,
            equipment,
            skills,
        } => Command::SpawnMobile {
            body,
            hue,
            hits,
            notoriety,
            damage,
            resistance,
            swing,
            sight,
            aggression,
            beat,
            ranged,
            ranged_kind,
            wander,
            position: openshard_protocol::Point::new(x, y, z),
            facet,
            // An empty name from the script means nameless.
            name: (!name.is_empty()).then_some(name),
            banker,
            vendor,
            equipment: equipment
                .into_iter()
                .map(|w| (w.graphic, w.layer, w.hue))
                .collect(),
            skills,
        },
        ScriptCommand::Damage {
            serial,
            amount,
            damage_type,
            by,
        } => Command::Damage {
            serial,
            amount,
            damage_type,
            by,
        },
        ScriptCommand::Heal { serial, amount } => Command::Heal { serial, amount },
        ScriptCommand::CastSpell {
            serial,
            spell,
            target,
            mana,
            difficulty,
            skill,
            pack,
            reagents,
        } => Command::CastSpell {
            serial,
            spell,
            target,
            mana,
            difficulty,
            skill,
            pack,
            reagents,
        },
        ScriptCommand::SetStats {
            serial,
            strength,
            dexterity,
            intelligence,
        } => Command::SetStats {
            serial,
            strength,
            dexterity,
            intelligence,
        },
        ScriptCommand::SetSkill {
            serial,
            skill,
            value,
        } => Command::SetSkill {
            serial,
            skill,
            value,
        },
        ScriptCommand::SetWeapon {
            serial,
            speed,
            min,
            max,
        } => Command::SetWeapon {
            serial,
            speed,
            min,
            max,
        },
        ScriptCommand::UseSkill {
            serial,
            skill,
            difficulty,
        } => Command::UseSkill {
            serial,
            skill,
            difficulty,
        },
        ScriptCommand::Speak { serial, hue, text } => Command::Speak { serial, hue, text },
        ScriptCommand::Control { serial } => Command::Control { serial },
        ScriptCommand::StockVendor { serial, stock } => Command::StockVendor {
            serial,
            stock: stock
                .into_iter()
                .map(|line| openshard_world::StockLine {
                    graphic: line.graphic,
                    hue: line.hue,
                    amount: line.amount,
                    price: line.price,
                    name: line.name,
                })
                .collect(),
        },
        ScriptCommand::AddLoot {
            container,
            graphic,
            hue,
            amount,
            stackable,
        } => Command::AddLoot {
            container,
            graphic,
            hue,
            amount,
            stackable,
        },
        ScriptCommand::ConsumeItem { serial, amount } => Command::ConsumeItem { serial, amount },
        ScriptCommand::RegisterSpawner {
            x,
            y,
            width,
            height,
            facet,
            max_count,
            respawn_delay,
            creatures,
        } => Command::RegisterSpawner {
            // Id 0 is a placeholder: the world assigns the real id (and de-dups by
            // region) when it registers, since it owns the counter.
            spawner: openshard_world::spawner::Spawner::new(
                0,
                openshard_world::spawner::SpawnArea {
                    x,
                    y,
                    width,
                    height,
                    facet,
                },
                creatures
                    .into_iter()
                    .map(|c| openshard_world::spawner::CreatureTemplate {
                        body: c.body,
                        hue: c.hue,
                        hits: c.hits,
                        notoriety: c.notoriety,
                        damage: c.damage,
                        resistance: c.resistance,
                        swing: c.swing,
                        sight: c.sight,
                        aggression: c.aggression,
                        beat: c.beat,
                        ranged: c.ranged,
                        ranged_kind: c.ranged_kind,
                        wander: c.wander,
                        skills: c.skills,
                    })
                    .collect(),
                max_count,
                respawn_delay,
            ),
        },
        ScriptCommand::ClearSpawners => Command::ClearSpawners,
        ScriptCommand::Decorate {
            facet,
            statics,
            doors,
            containers,
        } => Command::Decorate {
            facet,
            statics: statics
                .into_iter()
                .map(|s| {
                    (
                        s.graphic,
                        s.hue,
                        openshard_protocol::Point::new(s.x, s.y, s.z),
                    )
                })
                .collect(),
            doors: doors
                .into_iter()
                .map(|d| openshard_world::DecorDoor {
                    closed: d.closed,
                    open: d.open,
                    offset_x: d.offset_x,
                    offset_y: d.offset_y,
                    position: openshard_protocol::Point::new(d.x, d.y, d.z),
                })
                .collect(),
            containers: containers
                .into_iter()
                .map(|c| openshard_world::DecorContainer {
                    graphic: c.graphic,
                    gump: c.gump,
                    hue: c.hue,
                    position: openshard_protocol::Point::new(c.x, c.y, c.z),
                })
                .collect(),
        },
        ScriptCommand::ClearDecorations => Command::ClearDecorations,
        ScriptCommand::GenerateDoors {
            facet,
            x,
            y,
            width,
            height,
        } => Command::GenerateDoors {
            facet,
            x,
            y,
            width,
            height,
        },
        ScriptCommand::ShowGump {
            serial,
            gump_id,
            x,
            y,
            layout,
            lines,
        } => Command::ShowGump {
            serial,
            gump_id,
            x,
            y,
            layout,
            lines,
        },
        ScriptCommand::GiveItem {
            serial,
            graphic,
            hue,
            amount,
            stackable,
        } => Command::GiveItem {
            serial,
            graphic,
            hue,
            amount,
            stackable,
        },
        ScriptCommand::SetQuest { serial, blob } => Command::SetQuest { serial, blob },
        ScriptCommand::TakeItem {
            serial,
            graphic,
            amount,
        } => Command::TakeItem {
            serial,
            graphic,
            amount,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openshard_gateway::ConnectionId;
    use openshard_protocol::{AccessLevel, ClientVersion};
    use openshard_world::Position;
    use std::time::Instant;

    /// A script file that lasts as long as the test and cleans up after itself.
    struct TempScript(std::path::PathBuf);

    impl TempScript {
        fn new(name: &str, source: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("openshard-{name}-{}.js", std::process::id()));
            std::fs::write(&path, source).unwrap();
            Self(path)
        }
        fn path(&self) -> &str {
            self.0.to_str().unwrap()
        }
    }

    impl Drop for TempScript {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn no_script_configured_is_none_not_an_error() {
        let world = World::new((100, 100));
        assert!(Scripts::load("", &world).is_none());
        assert!(Scripts::load("   ", &world).is_none());
    }

    #[test]
    fn a_broken_script_drops_scripting_and_does_not_stop_the_shard() {
        let script = TempScript::new("broken", "function (");
        let world = World::new((100, 100));
        assert!(Scripts::load(script.path(), &world).is_none());
    }

    #[test]
    fn a_script_walks_a_mobile_in_response_to_an_event() {
        // The whole seam, end to end: a player enters, the script hears the
        // domain event and enqueues moves, and a later tick has stepped the
        // mobile north. Two moves in one handler because turning is a step of its
        // own — the first faces the mobile north, the second walks it — and both
        // land in the same tick, so the mobile ends up one tile north whatever it
        // was facing when it entered.
        let script = TempScript::new(
            "walker",
            "function onEvent(e) {\n\
             if (e.type === 'PlayerEntered') {\n\
                 Deno.core.ops.op_move(e.serial, 0);\n\
                 Deno.core.ops.op_move(e.serial, 0);\n\
             }\n\
             }",
        );

        let start = (1363u16, 1600u16);
        let now = Instant::now();
        let mut world = World::new(start);
        let mut scripts = Scripts::load(script.path(), &world).expect("script loads");

        world.queue(Command::Enter {
            connection: ConnectionId::from_raw(1),
            version: ClientVersion::TOL,
            account: "admin".to_owned(),
            name: "Lord British".to_owned(),
            serial: None,
            position: None,
            facet: 0,
            appearance: None,
            sheet: None,
            access: AccessLevel::Player,
        });
        world.tick(now); // emits PlayerEntered
        scripts.pump(&mut world); // script hears it, queues two Steps
        world.tick(now); // the Steps apply: turn north, then walk north

        let (_, &Position(pos)) = world
            .registry()
            .query::<Position>()
            .next()
            .expect("the player is in the world");
        assert!(
            pos.y < start.1,
            "the script walked the mobile north (from y={} to y={})",
            start.1,
            pos.y
        );
    }

    #[test]
    fn a_script_spawns_an_item_the_player_sees() {
        // The other command, end to end: on entering, the script drops an item
        // on the player's own tile, and the next tick the client is sent the
        // 0x1A that draws it.
        let script = TempScript::new(
            "dropper",
            "function onEvent(e) {\n\
             if (e.type === 'PlayerEntered') {\n\
                 Deno.core.ops.op_spawn_item({ graphic: 0x0EED, x: e.x, y: e.y, z: e.z });\n\
             }\n\
             }",
        );

        let now = Instant::now();
        let mut world = World::new((1363, 1600));
        let mut scripts = Scripts::load(script.path(), &world).expect("script loads");

        world.queue(Command::Enter {
            connection: ConnectionId::from_raw(1),
            version: ClientVersion::TOL,
            account: "admin".to_owned(),
            name: "Lord British".to_owned(),
            serial: None,
            position: None,
            facet: 0,
            appearance: None,
            sheet: None,
            access: AccessLevel::Player,
        });
        world.tick(now); // PlayerEntered
        let _ = world.drain_outbound().count(); // the login burst
        scripts.pump(&mut world); // script drops an item, queues SpawnItem
        world.tick(now); // the item spawns and is drawn

        let drew_item = world
            .drain_outbound()
            .any(|out| out.packet.first() == Some(&0x1A));
        assert!(
            drew_item,
            "the player was sent the 0x1A for the dropped item"
        );
    }

    #[test]
    fn a_script_spawns_a_container_the_player_can_open() {
        // A script drops a chest at the player's feet; double-clicking it (the
        // 0x06 the server would translate) opens the gump.
        let script = TempScript::new(
            "chest",
            "function onEvent(e) {\n\
             if (e.type === 'PlayerEntered') {\n\
                 Deno.core.ops.op_spawn_container({ graphic: 0x0E43, gump: 0x0049, x: e.x, y: e.y, z: e.z });\n\
             }\n\
             }",
        );

        let now = Instant::now();
        let mut world = World::new((1363, 1600));
        let mut scripts = Scripts::load(script.path(), &world).expect("script loads");

        world.queue(Command::Enter {
            connection: ConnectionId::from_raw(1),
            version: ClientVersion::TOL,
            account: "admin".to_owned(),
            name: "Lord British".to_owned(),
            serial: None,
            position: None,
            facet: 0,
            appearance: None,
            sheet: None,
            access: AccessLevel::Player,
        });
        world.tick(now);
        scripts.pump(&mut world); // spawns the container
        world.tick(now);
        let _ = world.drain_outbound().count();

        // The container is the one entity carrying a Container. Double-click it.
        let container = world
            .registry()
            .query::<openshard_world::Container>()
            .next()
            .map(|(e, _)| world.registry().serial_of(e).unwrap().raw())
            .expect("the script spawned a container");
        world.queue(Command::DoubleClick {
            connection: ConnectionId::from_raw(1),
            serial: container,
        });
        world.tick(now);

        let opened = world
            .drain_outbound()
            .any(|out| out.packet.first() == Some(&0x24));
        assert!(opened, "the container gump opens for the player");
    }

    #[test]
    fn a_script_reacts_to_a_death_by_dropping_loot() {
        // Combat's headline path, end to end: a creature dies, the world emits
        // MobileDied, the script hears it and drops loot — combat and loot
        // decoupled through the bus, exactly as the architecture intends.
        let script = TempScript::new(
            "loot",
            "function onEvent(e) {\n\
             if (e.type === 'MobileDied') {\n\
                 Deno.core.ops.op_spawn_item({ graphic: 0x0EED, x: 1363, y: 1600 });\n\
             }\n\
             }",
        );

        let now = Instant::now();
        let mut world = World::new((1363, 1600));
        let mut scripts = Scripts::load(script.path(), &world).expect("script loads");

        world.queue(Command::SpawnMobile {
            body: 0x0190,
            hue: 0,
            hits: 5,
            notoriety: 5,
            damage: 5,
            resistance: 0,
            swing: 0,
            sight: 0,
            aggression: 2,
            beat: 0,
            ranged: 0,
            ranged_kind: 0,
            wander: false,
            position: openshard_protocol::Point::new(1363, 1600, 0),
            facet: 0,
            name: None,
            banker: false,
            vendor: false,
            equipment: Vec::new(),
            skills: Vec::new(),
        });
        world.tick(now);
        let mob = world
            .registry()
            .query::<openshard_world::Hitpoints>()
            .filter_map(|(entity, _)| world.registry().serial_of(entity).map(|s| s.raw()))
            .next()
            .expect("the creature exists");

        world.queue(Command::Damage {
            serial: mob,
            amount: 100,
            damage_type: 0,
            by: 0,
        });
        world.tick(now); // the creature dies, MobileDied is emitted
        scripts.pump(&mut world); // the script hears it and queues the loot
        world.tick(now); // the loot spawns

        assert!(
            world
                .registry()
                .query::<openshard_world::Graphic>()
                .next()
                .is_some(),
            "the script dropped an item when the creature died"
        );
    }

    #[test]
    fn a_script_drives_a_controlled_mobile_from_its_on_tick() {
        // The per-mobile hook end to end: a mobile spawns, the script takes control
        // of it, and from then on its onTick walks it — a fully script-driven brain,
        // with the built-in ai standing aside.
        let script = TempScript::new(
            "shepherd",
            "function onEvent(e) {\n\
             if (e.type === 'MobileSpawned') Deno.core.ops.op_control(e.serial);\n\
             }\n\
             function onTick(s) { Deno.core.ops.op_move(s, 4); }",
        );

        let now = Instant::now();
        let mut world = World::new((1363, 1600));
        let mut scripts = Scripts::load(script.path(), &world).expect("script loads");

        // A pure creature: no brain of its own (sight 0, no wander), so nothing but
        // the script's onTick can move it.
        world.queue(Command::SpawnMobile {
            body: 0x0190,
            hue: 0,
            hits: 5,
            notoriety: 5,
            damage: 0,
            resistance: 0,
            swing: 0,
            sight: 0,
            aggression: 2,
            beat: 0,
            ranged: 0,
            ranged_kind: 0,
            wander: false,
            position: openshard_protocol::Point::new(1363, 1600, 0),
            facet: 0,
            name: None,
            banker: false,
            vendor: false,
            equipment: Vec::new(),
            skills: Vec::new(),
        });
        world.tick(now); // the mobile spawns, MobileSpawned emitted

        let mob = world
            .registry()
            .query::<openshard_world::Body>()
            .map(|(entity, _)| entity)
            .next()
            .expect("the creature exists");
        let start_y = world
            .registry()
            .get::<openshard_world::Position>(mob)
            .unwrap()
            .0
            .y;

        scripts.pump(&mut world); // onEvent hears the spawn and queues Control
        world.tick(now); // Control applies — the mobile is now scripted
                         // A few beats of the seam: onTick walks it south each tick.
        for _ in 0..4 {
            scripts.pump(&mut world);
            world.tick(now);
        }

        let end_y = world
            .registry()
            .get::<openshard_world::Position>(mob)
            .unwrap()
            .0
            .y;
        assert!(
            end_y > start_y,
            "the script's onTick walked the mobile south (from {start_y} to {end_y})"
        );
    }

    #[test]
    fn a_script_uses_a_skill_and_rewards_the_success() {
        // A skill round-trip: the script trains and uses a skill, the world rolls
        // it and emits SkillUsed, and the script — hearing the success — grants
        // the reward. Combat's death-loot pattern, for skills.
        let script = TempScript::new(
            "miner",
            "function onEvent(e) {\n\
             if (e.type === 'PlayerEntered') {\n\
                 Deno.core.ops.op_set_skill(e.serial, 1, 1000);\n\
                 Deno.core.ops.op_use_skill(e.serial, 1, 0);\n\
             }\n\
             if (e.type === 'SkillUsed' && e.success) {\n\
                 Deno.core.ops.op_spawn_item({ graphic: 0x19B9, x: 1363, y: 1600 });\n\
             }\n\
             }",
        );

        let now = Instant::now();
        let mut world = World::new((1363, 1600));
        let mut scripts = Scripts::load(script.path(), &world).expect("script loads");

        world.queue(Command::Enter {
            connection: ConnectionId::from_raw(1),
            version: ClientVersion::TOL,
            account: "admin".to_owned(),
            name: "Lord British".to_owned(),
            serial: None,
            position: None,
            facet: 0,
            appearance: None,
            sheet: None,
            access: AccessLevel::Player,
        });
        world.tick(now); // PlayerEntered
        scripts.pump(&mut world); // set + use the skill queued
        world.tick(now); // the skill is used, SkillUsed emitted
        scripts.pump(&mut world); // the script hears the success, queues the ore
        world.tick(now); // the ore spawns

        assert!(
            world
                .registry()
                .query::<openshard_world::Graphic>()
                .next()
                .is_some(),
            "the successful skill use produced its reward"
        );
    }

    #[test]
    fn a_script_casts_a_spell_and_deals_its_damage() {
        // The whole magic loop: the script trains Magery, spawns a target and
        // casts at it; the world pays mana and rolls the skill; the script hears
        // the success and deals the spell's fire damage.
        let script = TempScript::new(
            "mage",
            "function onEvent(e) {\n\
             if (e.type === 'PlayerEntered') {\n\
                 Deno.core.ops.op_set_skill(e.serial, 1, 1000);\n\
                 Deno.core.ops.op_spawn_mobile({ body: 0x0190, hits: 50, x: e.x, y: e.y });\n\
             }\n\
             if (e.type === 'SpellCast' && e.success) {\n\
                 Deno.core.ops.op_damage(e.target, 30, 1, e.serial);\n\
             }\n\
             }",
        );

        let now = Instant::now();
        let mut world = World::new((1363, 1600));
        let mut scripts = Scripts::load(script.path(), &world).expect("script loads");

        world.queue(Command::Enter {
            connection: ConnectionId::from_raw(1),
            version: ClientVersion::TOL,
            account: "admin".to_owned(),
            name: "Lord British".to_owned(),
            serial: None,
            position: None,
            facet: 0,
            appearance: None,
            sheet: None,
            access: AccessLevel::Player,
        });
        world.tick(now); // PlayerEntered
        scripts.pump(&mut world); // train + spawn the target queued
        world.tick(now); // skill set, target spawned

        // The caster and the target.
        let caster = world
            .registry()
            .query::<openshard_world::Client>()
            .next()
            .map(|(e, _)| world.registry().serial_of(e).unwrap().raw())
            .expect("the player");
        let (target_entity, target) = world
            .registry()
            .query::<openshard_world::Hitpoints>()
            .find(|(e, _)| !world.registry().has::<openshard_world::Client>(*e))
            .map(|(e, _)| (e, world.registry().serial_of(e).unwrap().raw()))
            .expect("the spawned target");

        // Cast at it (as a client or AI would); the script's SpellCast handler
        // deals the damage on success.
        world.queue(Command::CastSpell {
            serial: caster,
            spell: 18, // a fireball, say
            target,
            mana: 10,
            difficulty: 0,
            skill: 1,
            pack: 0,
            reagents: Vec::new(),
        });
        world.tick(now); // mana paid, skill rolled, SpellCast emitted
        scripts.pump(&mut world); // the script hears success, queues the damage
        world.tick(now); // the fire lands

        assert_eq!(
            world
                .registry()
                .get::<openshard_world::Hitpoints>(target_entity)
                .map(|h| h.current),
            Some(20),
            "thirty fire damage, unresisted, took the target from fifty to twenty"
        );
    }

    #[test]
    fn a_script_spawns_an_aggressive_creature_that_fights() {
        // AI end to end: a script drops an aggressive creature on the player's
        // tile, and the built-in brain — no further scripting — notices, and the
        // player takes damage. Combat, movement and the brain all reused.
        let script = TempScript::new(
            "spawner",
            "function onEvent(e) {\n\
             if (e.type === 'PlayerEntered') {\n\
                 Deno.core.ops.op_spawn_mobile({ body: 0x0009, hits: 50, damage: 8, sight: 10, x: e.x, y: e.y });\n\
             }\n\
             }",
        );

        let now = Instant::now();
        let mut world = World::new((1363, 1600));
        let mut scripts = Scripts::load(script.path(), &world).expect("script loads");

        world.queue(Command::Enter {
            connection: ConnectionId::from_raw(1),
            version: ClientVersion::TOL,
            account: "admin".to_owned(),
            name: "Lord British".to_owned(),
            serial: None,
            position: None,
            facet: 0,
            appearance: None,
            sheet: None,
            access: AccessLevel::Player,
        });
        world.tick(now);
        scripts.pump(&mut world); // the creature is spawned
        world.tick(now);

        let player = world
            .registry()
            .query::<openshard_world::Client>()
            .next()
            .map(|(e, _)| e)
            .expect("the player");

        // Give the brain time to notice and the swing time to land.
        for _ in 0..80 {
            world.tick(now);
        }
        assert!(
            world
                .registry()
                .get::<openshard_world::Hitpoints>(player)
                .unwrap()
                .current
                < 100,
            "the creature the script spawned attacked the player on its own"
        );
    }

    #[test]
    fn a_script_answers_a_spoken_keyword() {
        // Chat as a gameplay hook: a player says a word, the script hears it off
        // the bus and answers. The words round-trip through the world twice.
        let script = TempScript::new(
            "greeter",
            "function onEvent(e) {\n\
             if (e.type === 'MobileSpoke' && e.text === 'ping') {\n\
                 Deno.core.ops.op_say(e.serial, 'pong', 0);\n\
             }\n\
             }",
        );

        let now = Instant::now();
        let mut world = World::new((1363, 1600));
        let mut scripts = Scripts::load(script.path(), &world).expect("script loads");

        let connection = ConnectionId::from_raw(1);
        world.queue(Command::Enter {
            connection,
            version: ClientVersion::TOL,
            account: "admin".to_owned(),
            name: "Lord British".to_owned(),
            serial: None,
            position: None,
            facet: 0,
            appearance: None,
            sheet: None,
            access: AccessLevel::Player,
        });
        world.tick(now);
        scripts.pump(&mut world);
        let _ = world.drain_outbound().count();

        world.queue(Command::Say {
            connection,
            mode: 0,
            hue: 0,
            font: 3,
            text: "ping".to_owned(),
        });
        world.tick(now); // the player says it, MobileSpoke emitted
        let _ = world.drain_outbound().count(); // the "ping" bubble
        scripts.pump(&mut world); // the script hears it, queues the answer
        world.tick(now); // the answer is spoken

        // Speech is Unicode `0xAE` now, so "pong" is UTF-16; strip the zero bytes
        // and the ASCII characters read straight through.
        let answered = world.drain_outbound().any(|out| {
            out.packet.first() == Some(&0xAE) && {
                let text: Vec<u8> = out.packet.iter().copied().filter(|&b| b != 0).collect();
                text.windows(4).any(|w| w == b"pong")
            }
        });
        assert!(answered, "the script answered the keyword");
    }
}
