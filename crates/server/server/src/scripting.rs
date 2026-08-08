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
use openshard_protocol::serial::Serial;
use openshard_protocol::wire::{Graphic, Hue};
use openshard_scripting::{Command as ScriptCommand, DenoEngine, Event as ScriptEvent, ScriptEngine};
use openshard_world::events::{
    AdminMenuAction, CorpseCreated, GumpAnswered, MobileMoved, MobileSpawned, PlayerEntered, PlayerLeft,
    SpellRequested, StepRefused,
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
    restored: Cursor<openshard_world::events::MobileRestored>,
    cast_requested: Cursor<SpellRequested>,
    moved: Cursor<MobileMoved>,
    refused: Cursor<StepRefused>,
    left: Cursor<PlayerLeft>,
    died: Cursor<MobileDied>,
    used: Cursor<SkillUsed>,
    requested: Cursor<openshard_world::SkillRequested>,
    cast: Cursor<SpellCast>,
    spoke: Cursor<MobileSpoke>,
    item_used: Cursor<ItemUsed>,
    mobile_used: Cursor<MobileUsed>,
    items_taken: Cursor<ItemsTaken>,
    corpse: Cursor<CorpseCreated>,
    admin: Cursor<AdminMenuAction>,
    gump: Cursor<GumpAnswered>,
    quest_done: Cursor<openshard_quests::QuestCompleted>,
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
            restored: world.bus().cursor(),
            cast_requested: world.bus().cursor(),
            moved: world.bus().cursor(),
            refused: world.bus().cursor(),
            left: world.bus().cursor(),
            died: world.bus().cursor(),
            used: world.bus().cursor(),
            requested: world.bus().cursor(),
            cast: world.bus().cursor(),
            spoke: world.bus().cursor(),
            item_used: world.bus().cursor(),
            mobile_used: world.bus().cursor(),
            items_taken: world.bus().cursor(),
            corpse: world.bus().cursor(),
            admin: world.bus().cursor(),
            gump: world.bus().cursor(),
            quest_done: world.bus().cursor(),
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
            for e in bus.read(&mut self.spawned) {
                events.push(ScriptEvent::MobileSpawned {
                    serial: e.serial.raw(),
                    x: e.position.x,
                    y: e.position.y,
                    z: e.position.z,
                });
            }
            for e in bus.read(&mut self.restored) {
                events.push(ScriptEvent::MobileRestored {
                    serial: e.serial.raw(),
                    body: e.body.0,
                    // `home`, not `at`: a pack binds by the tile an NPC was placed
                    // on, and a townsperson with a routine is not standing on it
                    // when the save is taken. See `ScriptEvent::MobileRestored`.
                    x: e.home.x,
                    y: e.home.y,
                    z: e.home.z,
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
                    body: e.body.0,
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
            for e in bus.read(&mut self.requested) {
                events.push(ScriptEvent::SkillRequested {
                    serial: e.serial.raw(),
                    skill: e.skill,
                });
            }
            for e in bus.read(&mut self.cast) {
                events.push(ScriptEvent::SpellCast {
                    serial: e.serial.raw(),
                    spell: e.spell,
                    // Out through the same serialization seam the commands come
                    // in by: a spell with no mark is the script's `0`.
                    target: e.target.map_or(0, Serial::raw),
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
                    graphic: e.graphic.0,
                    by: e.by.raw(),
                });
            }
            for e in bus.read(&mut self.mobile_used) {
                events.push(ScriptEvent::MobileUsed {
                    mobile: e.mobile.raw(),
                    body: e.body.0,
                    by: e.by.raw(),
                });
            }
            for e in bus.read(&mut self.items_taken) {
                events.push(ScriptEvent::ItemsTaken {
                    player: e.player.raw(),
                    graphic: e.graphic.0,
                    taken: e.taken,
                });
            }
            for e in bus.read(&mut self.corpse) {
                events.push(ScriptEvent::CorpseCreated {
                    corpse: e.corpse.raw(),
                    body: e.body.0,
                });
            }
            for e in bus.read(&mut self.admin) {
                events.push(ScriptEvent::AdminAction {
                    serial: e.serial.map(Serial::raw),
                    action: e.action.clone(),
                });
            }
            for e in bus.read(&mut self.quest_done) {
                events.push(ScriptEvent::QuestCompleted {
                    serial: e.player.raw(),
                    key: e.key.clone(),
                    giver: e.giver.map_or(0, |g| g.raw()),
                });
            }
            for e in bus.read(&mut self.gump) {
                // The script bridge is a serialization seam, so this is where
                // the pack's own ids stop being types and become JSON numbers —
                // the same unwrapping a SQL bind or the wire itself does.
                events.push(ScriptEvent::GumpAnswered {
                    serial: e.serial.raw(),
                    gump_id: e.gump_id.0,
                    button: e.button.0,
                    switches: e.switches.iter().map(|switch| switch.0).collect(),
                    text: e.text_entries.clone(),
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
            // A command naming a serial nothing can have is dropped here rather
            // than queued: the tick would have looked it up and found nothing,
            // silently. See `script_serial`.
            if let Some(command) = into_world(command) {
                world.queue(command);
            }
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

/// A serial a script named, or `None` if the number addresses nothing.
///
/// The script bridge is a serialization seam like SQL or the wire, so this is
/// where a JSON number becomes a [`Serial`] — the same place `Command::Speak`'s
/// hue is made. Unlike a hue, the conversion can fail: `0` and everything past
/// the item pool are not serials, and `Serial::new` refuses them.
///
/// A refusal is logged and the whole command dropped. Before this the number
/// travelled to the tick as a bare `u32`, was refused there by the same
/// `Serial::new`, and the command did nothing with nothing said — a pack bug
/// that looked exactly like a mobile that had logged out.
fn script_serial(serial: u32) -> Option<Serial> {
    let made = Serial::new(serial);
    if made.is_none() {
        warn!(
            serial = format!("0x{serial:08X}"),
            "gameplay script named a serial nothing can have; command dropped"
        );
    }
    made
}

/// Turn a script's command into the world's. The one place the two vocabularies
/// meet, and the seam where §6 will grow: a new script command lands here.
///
/// `None` when the script named a serial that addresses nothing — see
/// [`script_serial`]. Every other command is total.
fn into_world(command: ScriptCommand) -> Option<Command> {
    Some(match command {
        ScriptCommand::Move { serial, direction } => Command::Step {
            serial: script_serial(serial)?,
            direction,
        },
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
            graphic: Graphic(graphic),
            hue: Hue(hue),
            amount,
            stackable,
            position: openshard_protocol::world::Point::new(x, y, z),
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
            graphic: Graphic(graphic),
            gump: Graphic(gump),
            hue: Hue(hue),
            position: openshard_protocol::world::Point::new(x, y, z),
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
            title,
            shoe,
            fame,
            karma,
            night_home,
            banker,
            vendor,
            equipment,
            skills,
        } => Command::SpawnMobile {
            body: Graphic(body),
            hue: Hue(hue),
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
            position: openshard_protocol::world::Point::new(x, y, z),
            facet,
            // An empty name from the script means nameless.
            name: (!name.is_empty()).then_some(name),
            // And an empty title means "not a townsperson" — a creature, which the
            // core never dresses and which keeps no beat.
            title: (!title.is_empty()).then_some(title),
            shoe,
            fame,
            karma,
            night_home: night_home.map(|(x, y, z)| openshard_protocol::world::Point::new(x, y, z)),
            banker,
            vendor,
            // The script bridge is a serialization seam like SQL or the wire, so
            // the JSON number becomes a `Layer` here — `docs/protocol_newtypes.md`
            // N3 amendment 9, the same place `Command::Speak`'s hue is made.
            equipment: equipment
                .into_iter()
                .map(|w| {
                    (
                        Graphic(w.graphic),
                        openshard_protocol::wire::Layer(w.layer),
                        Hue(w.hue),
                    )
                })
                .collect(),
            skills,
        },
        ScriptCommand::Damage {
            serial,
            amount,
            damage_type,
            by,
        } => Command::Damage {
            serial: script_serial(serial)?,
            amount,
            damage_type,
            // Zero is the script's word for unattributed, and `Serial::new`
            // already answers `None` to it — no log line, because absence here
            // is a value the caller may mean.
            by: Serial::new(by),
        },
        ScriptCommand::Heal { serial, amount } => Command::Heal {
            serial: script_serial(serial)?,
            amount,
        },
        ScriptCommand::CastSpell {
            serial,
            spell,
            target,
            mana,
            min_skill,
            max_skill,
            skill,
            pack,
            reagents,
        } => Command::CastSpell {
            serial: script_serial(serial)?,
            spell,
            // A spell that needs neither a target nor reagents says so with a
            // zero, which `Serial::new` reads as absent.
            target: Serial::new(target),
            mana,
            min_skill,
            max_skill,
            skill,
            pack: Serial::new(pack),
            reagents: reagents.into_iter().map(|(g, n)| (Graphic(g), n)).collect(),
        },
        ScriptCommand::SetStats {
            serial,
            strength,
            dexterity,
            intelligence,
        } => Command::SetStats {
            serial: script_serial(serial)?,
            strength,
            dexterity,
            intelligence,
        },
        ScriptCommand::SetSkill { serial, skill, value } => Command::SetSkill {
            serial: script_serial(serial)?,
            skill,
            value,
        },
        ScriptCommand::SetWeapon {
            serial,
            speed,
            min,
            max,
        } => Command::SetWeapon {
            serial: script_serial(serial)?,
            speed,
            min,
            max,
        },
        ScriptCommand::SetPoison {
            serial,
            level,
            charges,
        } => Command::SetPoison {
            serial: script_serial(serial)?,
            level,
            charges,
        },
        ScriptCommand::UseSkill {
            serial,
            skill,
            min_skill,
            max_skill,
        } => Command::UseSkill {
            serial: script_serial(serial)?,
            skill,
            min_skill,
            max_skill,
        },
        // The script boundary is a serialization seam like the wire or SQL: a JSON
        // number becomes the newtype here and stays one from here in.
        ScriptCommand::Speak { serial, hue, text } => Command::Speak {
            serial: script_serial(serial)?,
            hue: Hue(hue),
            text,
        },
        ScriptCommand::Control { serial } => Command::Control {
            serial: script_serial(serial)?,
        },
        ScriptCommand::StockVendor { serial, stock } => Command::StockVendor {
            serial: script_serial(serial)?,
            stock: stock
                .into_iter()
                .map(|line| openshard_world::StockLine {
                    graphic: Graphic(line.graphic),
                    hue: Hue(line.hue),
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
            container: script_serial(container)?,
            graphic: Graphic(graphic),
            hue: Hue(hue),
            amount,
            stackable,
        },
        ScriptCommand::ConsumeItem { serial, amount } => Command::ConsumeItem {
            serial: script_serial(serial)?,
            amount,
        },
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
                        body: Graphic(c.body),
                        hue: Hue(c.hue),
                        hits: c.hits,
                        notoriety: c.notoriety,
                        damage: c.damage,
                        resistance: c.resistance,
                        fame: c.fame,
                        karma: c.karma,
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
        ScriptCommand::RegisterRegions { facet, regions } => Command::RegisterRegions {
            facet,
            regions: regions
                .into_iter()
                .map(|region| openshard_world::Region {
                    // The world numbers them on registration, by position; this
                    // side has no id to give.
                    id: 0,
                    name: region.name,
                    priority: region.priority,
                    rects: region
                        .rects
                        .into_iter()
                        .map(
                            |(x, y, width, height, z_min, z_max)| openshard_world::RegionRect {
                                x,
                                y,
                                width,
                                height,
                                z_min,
                                z_max,
                            },
                        )
                        .collect(),
                    flags: openshard_world::RegionFlags {
                        guarded: region.guarded,
                        no_teleport: region.no_teleport,
                        no_recall: region.no_recall,
                        no_housing: region.no_housing,
                        safe: region.safe,
                    },
                    music: region.music,
                    light: region.light,
                })
                .collect(),
        },
        ScriptCommand::ClearRegions { facet } => Command::ClearRegions { facet },
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
                        Graphic(s.graphic),
                        Hue(s.hue),
                        openshard_protocol::world::Point::new(s.x, s.y, s.z),
                    )
                })
                .collect(),
            doors: doors
                .into_iter()
                .map(|d| openshard_world::DecorDoor {
                    key_value: d.key_value,
                    closed: Graphic(d.closed),
                    open: Graphic(d.open),
                    offset_x: d.offset_x,
                    offset_y: d.offset_y,
                    position: openshard_protocol::world::Point::new(d.x, d.y, d.z),
                })
                .collect(),
            containers: containers
                .into_iter()
                .map(|c| openshard_world::DecorContainer {
                    key_value: c.key_value,
                    graphic: Graphic(c.graphic),
                    gump: Graphic(c.gump),
                    hue: Hue(c.hue),
                    position: openshard_protocol::world::Point::new(c.x, c.y, c.z),
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
            serial: script_serial(serial)?,
            gump_id: openshard_protocol::gump::GumpId(gump_id),
            at: openshard_protocol::gump::GumpPoint::new(i32::from(x), i32::from(y)),
            layout,
            lines,
        },
        ScriptCommand::RegisterNpcSpeech {
            trades,
            male_names,
            female_names,
        } => Command::RegisterNpcSpeech {
            trades: trades.into_iter().map(trade_speech).collect(),
            male_names,
            female_names,
        },
        ScriptCommand::RegisterQuests { quests } => Command::RegisterQuests {
            quests: quests.into_iter().filter_map(quest_def).collect(),
        },
        ScriptCommand::BindQuestGiver { serial, keys } => Command::BindQuestGiver {
            serial: script_serial(serial)?,
            keys,
        },
        ScriptCommand::MakeEscortable { serial, destination } => Command::MakeEscortable {
            serial: script_serial(serial)?,
            destination,
        },
        ScriptCommand::CloseGump { serial, gump_id } => Command::CloseGump {
            serial: script_serial(serial)?,
            gump_id: openshard_protocol::gump::GumpId(gump_id),
        },
        ScriptCommand::Message { serial, text } => Command::Message {
            serial: script_serial(serial)?,
            text,
        },
        ScriptCommand::PlaySound { serial, sound } => Command::PlaySound {
            serial: script_serial(serial)?,
            sound: openshard_protocol::wire::SoundId(sound),
        },
        ScriptCommand::GiveItem {
            serial,
            graphic,
            hue,
            amount,
            stackable,
        } => Command::GiveItem {
            serial: script_serial(serial)?,
            graphic: Graphic(graphic),
            hue: Hue(hue),
            amount,
            stackable,
        },
        ScriptCommand::TakeItem {
            serial,
            graphic,
            amount,
        } => Command::TakeItem {
            serial: script_serial(serial)?,
            graphic: Graphic(graphic),
            amount,
        },
    })
}

/// One trade's speech, from the pack's wire-primitive form to the engine's.
///
/// Keywords are lowercased here, in the one place that knows both sides, because
/// the matcher compares against already-lowercased words — a pack that wrote
/// "Buy" would otherwise register a keyword nothing can ever match, and there is
/// nothing anywhere to say so.
fn trade_speech(trade: openshard_scripting::ScriptTradeSpeech) -> (String, openshard_world::SpeechTable) {
    use openshard_world::{SpeechEntry, SpeechTable};

    let table = SpeechTable {
        greetings: trade.greetings,
        barks: trade.barks,
        entries: trade
            .entries
            .into_iter()
            .map(|entry| SpeechEntry {
                keywords: entry
                    .keywords
                    .into_iter()
                    .map(|keyword| keyword.to_lowercase())
                    .collect(),
                lines: entry.lines,
            })
            .collect(),
        fallback: (!trade.fallback.is_empty()).then_some(trade.fallback),
    };
    (trade.title, table)
}

/// Turn the pack's quest into the engine's.
///
/// A quest with no usable objective is dropped rather than registered: an
/// objective list the engine cannot read would show as a quest that can be taken
/// and never finished, which is worse than one that is not offered. The kind
/// names are the pack's vocabulary and are matched here, in the one place that
/// knows both sides.
fn quest_def(quest: openshard_scripting::ScriptQuest) -> Option<openshard_world::QuestDef> {
    use openshard_world::{ObjectiveDef, ObjectiveKind, QuestDef, RewardDef, RewardKind};

    let mut objectives = Vec::with_capacity(quest.objectives.len());
    for objective in quest.objectives {
        let kind = match objective.kind.as_str() {
            "slay" | "kill" => ObjectiveKind::Slay {
                body: Graphic(objective.target),
            },
            "obtain" | "collect" => ObjectiveKind::Obtain {
                graphic: Graphic(objective.target),
            },
            "deliver" => ObjectiveKind::Deliver {
                graphic: Graphic(objective.target),
                to: objective.destination.clone(),
            },
            "escort" => ObjectiveKind::Escort {
                region: objective.destination.clone(),
            },
            other => {
                warn!(quest = %quest.key, kind = other, "unknown quest objective kind; quest dropped");
                return None;
            }
        };
        objectives.push(ObjectiveDef {
            kind,
            count: objective.count.max(1),
            name: objective.name,
            seconds: objective.seconds,
        });
    }
    if objectives.is_empty() {
        warn!(quest = %quest.key, "quest has no objectives; dropped");
        return None;
    }

    let rewards = quest
        .rewards
        .into_iter()
        .map(|reward| RewardDef {
            kind: if reward.gold > 0 {
                RewardKind::Gold(reward.gold)
            } else {
                RewardKind::Item {
                    graphic: Graphic(reward.graphic),
                    hue: Hue(reward.hue),
                    amount: reward.amount.max(1),
                    stackable: reward.stackable,
                }
            },
            name: reward.name,
        })
        .collect();

    Some(QuestDef {
        key: quest.key,
        title: quest.title,
        description: quest.description,
        refuse: quest.refuse,
        uncomplete: quest.uncomplete,
        complete: quest.complete,
        failed: quest.failed,
        objectives,
        rewards,
        all_objectives: quest.all_objectives,
        done_once: quest.done_once,
        restart_delay_secs: quest.restart_delay_secs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use openshard_gateway::ConnectionId;
    use openshard_protocol::containers::UseRequest;
    use openshard_protocol::identity::{AccountName, CharacterName};
    use openshard_protocol::serial::RawSerial;
    use openshard_protocol::speech::{RawFont, RawTalkMode};
    use openshard_protocol::wire::RawHue;
    use openshard_protocol::world::Facet;
    use openshard_protocol::{access::AccessLevel, version::ClientVersion};
    use openshard_world::{Character, Entering, Position};
    use std::time::Instant;

    /// A script file that lasts as long as the test and cleans up after itself.
    struct TempScript(std::path::PathBuf);

    impl TempScript {
        fn new(name: &str, source: &str) -> Self {
            let path = std::env::temp_dir().join(format!("openshard-{name}-{}.js", std::process::id()));
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

        world.queue(Command::Enter(Entering {
            connection: ConnectionId::from_raw(1),
            version: ClientVersion::TOL,
            account: AccountName("admin".to_owned()),
            name: CharacterName("Lord British".to_owned()),
            access: AccessLevel::Player,
            character: Character::fresh(Facet(0)),
        }));
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

        world.queue(Command::Enter(Entering {
            connection: ConnectionId::from_raw(1),
            version: ClientVersion::TOL,
            account: AccountName("admin".to_owned()),
            name: CharacterName("Lord British".to_owned()),
            access: AccessLevel::Player,
            character: Character::fresh(Facet(0)),
        }));
        world.tick(now); // PlayerEntered
        let _ = world.drain_outbound().count(); // the login burst
        scripts.pump(&mut world); // script drops an item, queues SpawnItem
        world.tick(now); // the item spawns and is drawn

        let drew_item = world
            .drain_outbound()
            .any(|out| out.packet.first() == Some(&0x1A));
        assert!(drew_item, "the player was sent the 0x1A for the dropped item");
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

        world.queue(Command::Enter(Entering {
            connection: ConnectionId::from_raw(1),
            version: ClientVersion::TOL,
            account: AccountName("admin".to_owned()),
            name: CharacterName("Lord British".to_owned()),
            access: AccessLevel::Player,
            character: Character::fresh(Facet(0)),
        }));
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
            request: UseRequest::Use(RawSerial(container)),
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
            body: Graphic(0x0190),
            hue: Hue(0),
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
            position: openshard_protocol::world::Point::new(1363, 1600, 0),
            facet: 0,
            name: None,
            title: None,
            shoe: 0,
            fame: 0,
            karma: 0,
            night_home: None,
            banker: false,
            vendor: false,
            equipment: Vec::new(),
            skills: Vec::new(),
        });
        world.tick(now);
        let mob = world
            .registry()
            .query::<openshard_world::Hitpoints>()
            .filter_map(|(entity, _)| world.registry().serial_of(entity))
            .next()
            .expect("the creature exists");

        world.queue(Command::Damage {
            serial: mob,
            amount: 100,
            damage_type: 0,
            by: None,
        });
        world.tick(now); // the creature dies, MobileDied is emitted
        scripts.pump(&mut world); // the script hears it and queues the loot
        world.tick(now); // the loot spawns

        assert!(
            world
                .registry()
                .query::<openshard_world::Drawn>()
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
            body: Graphic(0x0190),
            hue: Hue(0),
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
            position: openshard_protocol::world::Point::new(1363, 1600, 0),
            facet: 0,
            name: None,
            title: None,
            shoe: 0,
            fame: 0,
            karma: 0,
            night_home: None,
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

        world.queue(Command::Enter(Entering {
            connection: ConnectionId::from_raw(1),
            version: ClientVersion::TOL,
            account: AccountName("admin".to_owned()),
            name: CharacterName("Lord British".to_owned()),
            access: AccessLevel::Player,
            character: Character::fresh(Facet(0)),
        }));
        world.tick(now); // PlayerEntered
        scripts.pump(&mut world); // set + use the skill queued
        world.tick(now); // the skill is used, SkillUsed emitted
        scripts.pump(&mut world); // the script hears the success, queues the ore
        world.tick(now); // the ore spawns

        assert!(
            world
                .registry()
                .query::<openshard_world::Drawn>()
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

        world.queue(Command::Enter(Entering {
            connection: ConnectionId::from_raw(1),
            version: ClientVersion::TOL,
            account: AccountName("admin".to_owned()),
            name: CharacterName("Lord British".to_owned()),
            access: AccessLevel::Player,
            character: Character::fresh(Facet(0)),
        }));
        world.tick(now); // PlayerEntered
        scripts.pump(&mut world); // train + spawn the target queued
        world.tick(now); // skill set, target spawned

        // The caster and the target.
        let caster = world
            .registry()
            .query::<openshard_world::Client>()
            .next()
            .map(|(e, _)| world.registry().serial_of(e).unwrap())
            .expect("the player");
        let (target_entity, target) = world
            .registry()
            .query::<openshard_world::Hitpoints>()
            .find(|(e, _)| !world.registry().has::<openshard_world::Client>(*e))
            .map(|(e, _)| (e, world.registry().serial_of(e).unwrap()))
            .expect("the spawned target");

        // Cast at it (as a client or AI would); the script's SpellCast handler
        // deals the damage on success.
        world.queue(Command::CastSpell {
            serial: caster,
            spell: 18, // a fireball, say
            target: Some(target),
            mana: 10,
            min_skill: 0,
            max_skill: 0,
            skill: 1,
            pack: None,
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

        world.queue(Command::Enter(Entering {
            connection: ConnectionId::from_raw(1),
            version: ClientVersion::TOL,
            account: AccountName("admin".to_owned()),
            name: CharacterName("Lord British".to_owned()),
            access: AccessLevel::Player,
            character: Character::fresh(Facet(0)),
        }));
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
        world.queue(Command::Enter(Entering {
            connection,
            version: ClientVersion::TOL,
            account: AccountName("admin".to_owned()),
            name: CharacterName("Lord British".to_owned()),
            access: AccessLevel::Player,
            character: Character::fresh(Facet(0)),
        }));
        world.tick(now);
        scripts.pump(&mut world);
        let _ = world.drain_outbound().count();

        world.queue(Command::Say {
            connection,
            mode: RawTalkMode(0),
            hue: RawHue(0),
            font: RawFont(3),
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

    #[test]
    fn a_script_gives_a_facet_its_regions() {
        // The pack owns the map of the world: it hands the engine a whole
        // facet's named areas through one op, and the flags come across intact
        // — which is what the guards, the dark and the no-teleport rule read.
        let script = TempScript::new(
            "regions",
            "function onEvent(e) {\n\
             if (e.type === 'PlayerEntered') {\n\
                 Deno.core.ops.op_register_regions({ facet: 0, regions: [\n\
                   { name: 'Britain', priority: 50, guarded: true, music: 9,\n\
                     rects: [{ x: 1300, y: 1500, width: 200, height: 200 }] },\n\
                   { name: 'Covetous', priority: 60, noTeleport: true, light: 26,\n\
                     rects: [{ x: 1350, y: 1550, width: 10, height: 10, zMin: -128, zMax: -20 }] },\n\
                 ] });\n\
             }\n\
             }",
        );

        let now = Instant::now();
        let mut world = World::new((1363, 1600));
        let mut scripts = Scripts::load(script.path(), &world).expect("script loads");

        world.queue(Command::Enter(Entering {
            connection: ConnectionId::from_raw(1),
            version: ClientVersion::TOL,
            account: AccountName("admin".to_owned()),
            name: CharacterName("Lord British".to_owned()),
            access: AccessLevel::Player,
            character: Character::fresh(Facet(0)),
        }));
        world.tick(now); // PlayerEntered
        scripts.pump(&mut world); // the script registers the regions
        world.tick(now); // the world takes them

        let britain = world
            .region_at(0, openshard_protocol::world::Point::new(1363, 1600, 0))
            .expect("the player is standing in Britain");
        assert_eq!(britain.name, "Britain");
        assert!(britain.flags.guarded);
        assert_eq!(britain.music, Some(9));

        // The height band came across too: the dungeon is below, not underfoot.
        assert!(
            world
                .region_at(0, openshard_protocol::world::Point::new(1355, 1555, 0))
                .is_some_and(|r| r.name == "Britain")
        );
        let deep = world
            .region_at(0, openshard_protocol::world::Point::new(1355, 1555, -40))
            .expect("the dungeon is under it");
        assert_eq!(deep.name, "Covetous");
        assert!(deep.flags.no_teleport);
        assert_eq!(deep.light, Some(26));
    }
}
