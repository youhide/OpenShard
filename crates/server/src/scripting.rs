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
use openshard_world::events::{MobileDied, MobileMoved, PlayerEntered, PlayerLeft, StepRefused};
use openshard_world::{Command, World};
use tracing::{error, info, warn};

/// The gameplay script, driven around the world's tick.
pub struct Scripts {
    engine: DenoEngine,
    entered: Cursor<PlayerEntered>,
    moved: Cursor<MobileMoved>,
    refused: Cursor<StepRefused>,
    left: Cursor<PlayerLeft>,
    died: Cursor<MobileDied>,
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
            moved: world.bus().cursor(),
            refused: world.bus().cursor(),
            left: world.bus().cursor(),
            died: world.bus().cursor(),
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
                });
            }
        }

        for event in &events {
            if let Err(error) = self.engine.deliver(event) {
                warn!(%error, "gameplay script event handler threw");
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
            x,
            y,
            z,
            facet,
        } => Command::SpawnMobile {
            body,
            hue,
            hits,
            notoriety,
            damage,
            resistance,
            position: openshard_protocol::Point::new(x, y, z),
            facet,
        },
        ScriptCommand::Damage { serial, amount } => Command::Damage { serial, amount },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openshard_gateway::ConnectionId;
    use openshard_protocol::ClientVersion;
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
            position: openshard_protocol::Point::new(1363, 1600, 0),
            facet: 0,
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
}
