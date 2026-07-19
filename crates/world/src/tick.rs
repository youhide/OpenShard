//! The simulation loop.
//!
//! # Why there is a tick at all
//!
//! Everything so far answers a packet: a client asks to walk, the server says
//! yes. That works right up until something has to happen *without* a client
//! asking — an item decaying, a wound healing, an NPC deciding to move. There is
//! nowhere to put any of it in a request/response server.
//!
//! The tick is that place. It is also what makes the simulation deterministic:
//! commands arrive from network tasks on whatever thread at whatever moment,
//! queue up, and are applied in a fixed order at a fixed rate. Replay the same
//! commands and you get the same world.
//!
//! # The boundary
//!
//! ```text
//!   network tasks          the tick               network tasks
//!   ─────────────>  [ commands ]  ─────────>  [ outbound packets ]
//!        async         drained in order            async again
//! ```
//!
//! The gateway already draws half of this line by handing events to a channel
//! rather than calling back. This is the other half: nothing inside
//! [`World::tick`] awaits, reads a clock, or touches a socket.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};

use openshard_entities::{EntityId, Registry, Serial, SerialKind};
use openshard_events::{Cursor, EventBus};
use openshard_gateway::ConnectionId;
use openshard_movement::{step_from, OpenWorld, Terrain, Walk, Walker};
use openshard_persistence::{
    CharacterRecord, Inventory, ItemLocation, ItemRecord, Journal, Snapshot, SCHEMA_VERSION,
};
use openshard_protocol::{
    encode_light_level, encode_login_complete, encode_map_change, encode_message, encode_walk_ack,
    encode_walk_reject, AccessLevel, ClientVersion, Direction, Facing, MobileStatus, Notoriety,
    PlayerStart, PlayerUpdate, Point, WalkRequest, DEFAULT_MAP_HEIGHT, DEFAULT_MAP_WIDTH,
    LABEL_MODE,
};
use tracing::{debug, info, warn};

use openshard_state::components::{
    Access, Account, Amount, Banker, Body, Brain, Client, Combat, Contained, Container, DamageType,
    Decoration, Door, Equipped, Facet, Graphic, Heading, Hitpoints, Mana, MeleeDamage, Movement,
    Name, Npc, Position, Resistance, Scripted, SpawnedBy, Stackable, Stats, SwingSpeed,
};
use openshard_state::rng::Rng;
use openshard_state::sectors::Sectors;
use openshard_state::{FacetState, Gameplay, Outbound, WorldState, TICKS_PER_SECOND};

use openshard_ai as ai;
use openshard_chat as chat;
use openshard_combat as combat;
use openshard_items as items;
use openshard_magic as magic;
use openshard_npc as npc;
use openshard_skills as skills;

use crate::doorgen;
use crate::events::{
    AdminMenuAction, MobileMoved, MobileSpawned, MobileTurned, PlayerEntered, PlayerLeft,
    RefusedReason, SpellRequested, StepRefused,
};
use crate::gm;
use crate::terrain::MapTerrain;

mod command;
mod defaults;
mod enter;
mod persist;

pub use command::{Appearance, Command, DecorContainer, DecorDoor};
use defaults::*;
pub use defaults::{SAVE_EVERY_TICKS, TICK_INTERVAL};

/// Everything [`World::enter`] needs: who is entering, and as what. A plain
/// bundle so the one function that puts a character in the world takes one
/// argument instead of seven.
struct Entering {
    connection: ConnectionId,
    version: ClientVersion,
    account: String,
    name: String,
    serial: Option<u32>,
    position: Option<Point>,
    facet: u8,
    appearance: Option<Appearance>,
    access: AccessLevel,
}

/// Everything [`World::spawn_mobile`] needs — a plain bundle, so the one function
/// that makes a creature takes one argument instead of eleven.
struct SpawnMobile {
    body: u16,
    hue: u16,
    hits: u16,
    notoriety: u8,
    damage: u16,
    resistance: u8,
    swing: u64,
    sight: u8,
    wander: bool,
    position: Point,
    facet: u8,
    /// A name the client shows on single-click, if any. Townsfolk have one.
    name: Option<String>,
    /// Whether this mobile is a banker — it answers "bank".
    banker: bool,
    /// Worn clothing and gear, `(graphic, layer, hue)` — so it is not naked.
    equipment: Vec<(u16, u8, u16)>,
}

// `Outbound`, `FacetState`, `HeldItem` and `Origin` are the world's runtime
// state, moved down into `openshard-state` with `WorldState` so the systems can
// live in their own crates. Imported at the top of the file.

/// The world: the runtime state plus the tick that drives it and the journal
/// that saves it.
///
/// The gameplay state — registry, bus, facets, who-sees-what — lives in
/// [`WorldState`], one level down, so systems can operate on it from their own
/// crates. What stays here is what a system never touches: the persistence
/// journal, the save cadence, and the command queue the tick drains. A plain
/// value: nothing is a static, and a test builds as many as it likes.
pub struct World {
    /// The runtime state every gameplay system reads and writes.
    state: WorldState,
    /// What has changed since the last save.
    journal: Journal,
    /// How often to offer a snapshot, in ticks. Zero never saves.
    save_every: u64,
    /// Snapshots the tick has taken and nobody has collected yet.
    saves: Vec<Snapshot>,
    /// Characters that left this tick, with the state they left in. The server
    /// drains these to keep its in-memory character list current, so a re-login in
    /// the same run finds the character where it logged out — not where it was at
    /// boot. The store gets the same record through the journal; this is the
    /// immediate copy, because a re-login can beat the next deferred save.
    departed: Vec<CharacterRecord>,
    /// Read to find out what to mark dirty. See `mark_dirty`.
    entered: Cursor<PlayerEntered>,
    /// Read to find out what to mark dirty. See `mark_dirty`.
    moved: Cursor<MobileMoved>,
    /// Read to find out what to mark dirty. See `mark_dirty`.
    turned: Cursor<MobileTurned>,
    /// Commands waiting for the next tick.
    inbox: Vec<Command>,
    /// The spawn regions the tick keeps populated. Registered by the script pack,
    /// maintained here, and persisted — a populated area stays populated across a
    /// restart, and a rare spawn keeps its remaining respawn wait.
    spawners: Vec<crate::spawner::Spawner>,
    /// The next id to hand a newly registered spawner. Bumped past every id loaded
    /// from the store so a fresh registration never collides with a restored one.
    next_spawner_id: u32,
    /// Saved inventories waiting for their owners to log in, keyed by character
    /// serial. Loaded from the store at boot by [`restore_inventory`]; a character
    /// entering takes its own and equips it, once.
    ///
    /// [`restore_inventory`]: World::restore_inventory
    pending_inventories: HashMap<u32, Vec<ItemRecord>>,
}

impl std::fmt::Debug for World {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("World")
            .field("state", &self.state)
            .field("unsaved", &self.journal.len())
            .finish()
    }
}

impl World {
    /// An empty world with no map, spawning at `start`.
    pub fn new(start: (u16, u16)) -> Self {
        // Always at least the default facet, so there is somewhere to stand even
        // with no map loaded — the same no-map mode the shard has always had.
        let mut facets = BTreeMap::new();
        facets.insert(
            DEFAULT_FACET,
            FacetState {
                terrain: None,
                sectors: Sectors::new(FACET_WITHOUT_A_MAP.0, FACET_WITHOUT_A_MAP.1),
            },
        );
        Self {
            state: WorldState {
                registry: Registry::new(),
                bus: EventBus::new(),
                facets,
                default_facet: DEFAULT_FACET,
                players: HashMap::new(),
                seen: HashMap::new(),
                held: HashMap::new(),
                start,
                rng: Rng::new(DEFAULT_SEED),
                ticks: 0,
                outbox: Vec::new(),
                open_containers: HashMap::new(),
                pending_targets: HashMap::new(),
                gameplay: Gameplay::default(),
                save_requested: false,
            },
            journal: Journal::new(),
            save_every: SAVE_EVERY_TICKS,
            saves: Vec::new(),
            departed: Vec::new(),
            entered: Cursor::default(),
            moved: Cursor::default(),
            turned: Cursor::default(),
            inbox: Vec::new(),
            spawners: Vec::new(),
            next_spawner_id: 1,
            pending_inventories: HashMap::new(),
        }
    }

    /// How often to offer a snapshot, in ticks. Zero never saves.
    ///
    /// Zero is a real mode and not a broken one: the shard already runs with no
    /// map, and running with nothing to save to is the same bargain. What it
    /// must not do is pretend — a world with nowhere to write is a world that
    /// says so, not one that keeps a journal nobody ever collects.
    pub const fn with_save_every(mut self, ticks: u64) -> Self {
        self.save_every = ticks;
        self
    }

    /// How often to save, in *seconds* — what the operator sets in the config. `0`
    /// keeps the periodic save off (only shutdown and a staff `.save` write). The
    /// world owns the tick rate, so the conversion lives here rather than in the
    /// server.
    pub const fn with_save_seconds(self, seconds: u64) -> Self {
        self.with_save_every(seconds.saturating_mul(TICKS_PER_SECOND))
    }

    /// Set the tunable gameplay rules. The server builds these from the
    /// `[gameplay]` config; a test or the default takes [`Gameplay::default`],
    /// the pre-AoS numbers the systems were written with.
    #[must_use]
    pub const fn with_gameplay(mut self, gameplay: Gameplay) -> Self {
        self.state.gameplay = gameplay;
        self
    }

    /// Give the default facet a map.
    pub fn with_terrain(self, terrain: MapTerrain) -> Self {
        let facet = self.state.default_facet;
        self.with_facet(facet, terrain)
    }

    /// Load `terrain` as facet `facet`, its interest grid sized to the map.
    pub fn with_facet(mut self, facet: u8, terrain: MapTerrain) -> Self {
        let sectors = Sectors::new(terrain.map().width(), terrain.map().height());
        // Boxed as `dyn Terrain`: the state crate holds the abstraction, and the
        // world supplies the concrete map here.
        self.state.facets.insert(
            facet,
            FacetState {
                terrain: Some(Box::new(terrain) as Box<dyn Terrain + Send + Sync>),
                sectors,
            },
        );
        self
    }

    /// The default facet's spatial index.
    pub fn sectors(&self) -> &Sectors {
        &self.state.facets[&self.state.default_facet].sectors
    }

    /// The event bus, for reading what happened.
    pub const fn bus(&self) -> &EventBus {
        &self.state.bus
    }

    /// Everything in the world.
    pub const fn registry(&self) -> &Registry {
        &self.state.registry
    }

    /// How many ticks have run.
    pub const fn ticks(&self) -> u64 {
        self.state.ticks
    }

    /// How many people are in the world.
    pub fn player_count(&self) -> usize {
        self.state.players.len()
    }

    /// Queue a command for the next tick.
    ///
    /// Never acts immediately. That is the whole point: a command that took
    /// effect the moment it arrived would run world code on a network thread at
    /// an arbitrary point in the tick, and two clients racing would produce a
    /// different world depending on which packet won.
    pub fn queue(&mut self, command: Command) {
        self.inbox.push(command);
    }

    /// Take the packets the last tick produced.
    pub fn drain_outbound(&mut self) -> std::vec::Drain<'_, Outbound> {
        self.state.outbox.drain(..)
    }

    /// Take the snapshots the tick has offered to persistence.
    ///
    /// The same shape as [`drain_outbound`](Self::drain_outbound), and for the
    /// same reason: the world produces owned values and never waits for anyone
    /// to take them. What the caller does with a snapshot — write it, queue it,
    /// drop it — is not the tick's problem, and the tick is not slower for the
    /// answer being "write it to a disk in Frankfurt".
    pub fn drain_saves(&mut self) -> std::vec::Drain<'_, Snapshot> {
        self.saves.drain(..)
    }

    /// Take the records of characters that logged out since the last call.
    ///
    /// The server keeps an in-memory character list — where each stored character
    /// was, so playing one spawns it back at its spot — seeded from the store at
    /// boot. Without this it would go stale the moment a character moved and logged
    /// out, and a re-login in the same run would rewind to the boot position. These
    /// are the fresh records to fold in; the store gets the same data through the
    /// journal, but a re-login can beat that deferred write, so this is the copy
    /// that closes the gap.
    pub fn drain_departed(&mut self) -> std::vec::Drain<'_, CharacterRecord> {
        self.departed.drain(..)
    }

    /// How many entities are waiting to be saved.
    pub fn unsaved(&self) -> usize {
        self.journal.len()
    }

    /// Mark everything as needing saving, whatever the tracking thinks.
    ///
    /// # This is what a failed save costs
    ///
    /// The precise answer is to remember which entities were in the snapshot
    /// that failed and mark those. The reason not to is that it means the world
    /// tracking in-flight writes — a map of tick to entities, a message back per
    /// success, and a leak the first time a store neither succeeds nor fails.
    /// That is real bookkeeping on the common path to make the rare path cheap.
    ///
    /// So the rare path is expensive instead: a save that failed makes the next
    /// one a full sweep. It is more rows than necessary, it is always correct
    /// whatever was lost, and it costs nothing at all when nothing goes wrong.
    ///
    /// Also for shutdown, where "everything" is the only right answer.
    pub fn resweep(&mut self) {
        let characters: Vec<EntityId> = self
            .state
            .registry
            .query::<Name>()
            .map(|(entity, _)| entity)
            .collect();
        self.journal.touch_all(characters);
    }

    /// Run one tick.
    ///
    /// `now` is a parameter, like everywhere else on this path: a tick that read
    /// the clock could not be replayed, and a simulation that cannot be replayed
    /// cannot be debugged from a log.
    pub fn tick(&mut self, now: Instant) {
        self.state.ticks += 1;

        // Take the whole inbox. A command queued *during* a tick belongs to the
        // next one — otherwise a system that queues work could starve the loop,
        // and the tick's length would depend on what happened in it.
        let commands = std::mem::take(&mut self.inbox);
        for command in commands {
            self.apply(command, now);
        }

        // Strike whatever swings are due, lift any criminal flags that have run
        // out, then rot away what has lain on the ground too long. All after the
        // commands and all driven by the tick counter, so a fight, a flag and a
        // decay are as replayable as everything else.
        self.think();
        // The townsfolk beat: `npc::live` greets and faces on its own and hands
        // back the idle steps it wants, which the tick applies through `step` —
        // the same decide-then-apply split the creature brain uses.
        for (serial, direction) in npc::live(&mut self.state) {
            self.step(serial, direction);
        }
        combat::swings(&mut self.state);
        combat::expire_criminality(&mut self.state);
        combat::decay_murders(&mut self.state);
        magic::regen_mana(&mut self.state);
        items::decay(&mut self.state);
        items::close_doors(&mut self.state);
        self.maintain_spawners();

        // Before the bus retires anything: what happened is what needs saving,
        // and reading it after `update` would read it a tick late.
        self.mark_dirty();
        // A staff `.save` this tick forces a snapshot now; otherwise the cadence
        // decides. Either way the world never pauses — the snapshot is instant.
        if std::mem::take(&mut self.state.save_requested) {
            self.take_snapshot();
        } else {
            self.offer_snapshot();
        }

        // Retire the oldest events. Once per tick, after every system, so that
        // "one tick" means the same thing for every event type.
        self.state.bus.update();
    }

    fn apply(&mut self, command: Command, now: Instant) {
        match command {
            Command::Enter {
                connection,
                version,
                account,
                name,
                serial,
                position,
                facet,
                appearance,
                access,
            } => self.enter(Entering {
                connection,
                version,
                account,
                name,
                serial,
                position,
                facet,
                appearance,
                access,
            }),
            Command::Walk {
                connection,
                request,
            } => self.walk(connection, request, now),
            Command::RequestStatus { connection } => {
                if let Some(&entity) = self.state.players.get(&connection) {
                    self.send_status(connection, entity);
                }
            }
            Command::GumpResponse {
                connection,
                response,
            } => self.handle_admin_gump(connection, response),
            Command::TargetResponse {
                connection,
                response,
            } => self.handle_target(connection, response),
            Command::RegisterSpawner { spawner } => self.register_spawner(spawner),
            Command::ClearSpawners => self.clear_spawners(),
            Command::Decorate {
                facet,
                statics,
                doors,
                containers,
            } => self.decorate(facet, &statics, &doors, &containers),
            Command::GenerateDoors {
                facet,
                x,
                y,
                width,
                height,
            } => self.generate_doors(facet, x, y, width, height),
            Command::ClearDecorations => self.clear_decorations(),
            Command::Step { serial, direction } => self.step(serial, direction),
            Command::SpawnItem {
                graphic,
                hue,
                amount,
                stackable,
                position,
                facet,
            } => {
                items::spawn_item(
                    &mut self.state,
                    graphic,
                    hue,
                    amount,
                    stackable,
                    position,
                    facet,
                );
            }
            Command::SpawnContainer {
                graphic,
                gump,
                hue,
                position,
                facet,
            } => items::spawn_container(&mut self.state, graphic, gump, hue, position, facet),
            Command::SpawnMobile {
                body,
                hue,
                hits,
                notoriety,
                damage,
                resistance,
                swing,
                sight,
                wander,
                position,
                facet,
                name,
                banker,
                equipment,
            } => {
                self.spawn_mobile(SpawnMobile {
                    body,
                    hue,
                    hits,
                    notoriety,
                    damage,
                    resistance,
                    swing,
                    sight,
                    wander,
                    position,
                    facet,
                    name,
                    banker,
                    equipment,
                });
            }
            Command::Damage {
                serial,
                amount,
                damage_type,
                by,
            } => combat::damage(
                &mut self.state,
                serial,
                amount,
                DamageType::from_u8(damage_type),
                openshard_entities::Serial::new(by),
            ),
            Command::CastSpell {
                serial,
                spell,
                target,
                mana,
                difficulty,
                skill,
                pack,
                reagents,
            } => magic::cast_spell(
                &mut self.state,
                magic::Cast {
                    serial,
                    spell,
                    target,
                    mana,
                    difficulty,
                    skill,
                    pack,
                    reagents: &reagents,
                },
            ),
            Command::Heal { serial, amount } => magic::heal(&mut self.state, serial, amount),
            Command::SetStats {
                serial,
                strength,
                dexterity,
                intelligence,
            } => skills::set_stats(&mut self.state, serial, strength, dexterity, intelligence),
            Command::SetSkill {
                serial,
                skill,
                value,
            } => skills::set_skill(&mut self.state, serial, skill, value),
            Command::UseSkill {
                serial,
                skill,
                difficulty,
            } => skills::use_skill(&mut self.state, serial, skill, difficulty),
            Command::WarMode { connection, war } => {
                combat::war_mode(&mut self.state, connection, war)
            }
            Command::Attack { connection, target } => {
                combat::attack(&mut self.state, connection, target)
            }
            Command::Say {
                connection,
                mode,
                hue,
                font,
                text,
            } => self.say(connection, mode, hue, font, text),
            Command::Speak { serial, hue, text } => {
                if let Some(entity) =
                    Serial::new(serial).and_then(|s| self.state.registry.entity_of(s))
                {
                    chat::speak(&mut self.state, entity, 0, hue, chat::DEFAULT_FONT, &text);
                }
            }
            Command::DoubleClick { connection, serial } => {
                items::double_click(&mut self.state, connection, serial)
            }
            Command::SingleClick { connection, serial } => self.single_click(connection, serial),
            Command::EquipItem {
                connection,
                item,
                layer,
                mobile,
            } => items::equip_item(&mut self.state, connection, item, layer, mobile),
            Command::PickUpItem {
                connection,
                serial,
                amount,
            } => items::pick_up(&mut self.state, connection, serial, amount),
            Command::DropItem {
                connection,
                serial,
                position,
                container,
            } => items::drop_item(&mut self.state, connection, serial, position, container),
            Command::Disconnect { connection } => self.disconnect(connection),
            Command::Control { serial } => self.control(serial),
            Command::RequestCast { connection, spell } => self.request_cast(connection, spell),
        }
    }

    /// A player's speech, with staff commands split off the front. A
    /// `.`-prefixed line from a game master runs as a command and never reaches
    /// anyone's screen; from an ordinary player it is just speech, so a player can
    /// still say ".hello" out loud. The authority gate lives here, not in `gm`,
    /// so the command module can assume a call is already cleared.
    fn say(&mut self, connection: ConnectionId, mode: u8, hue: u16, font: u16, text: String) {
        if let Some(rest) = text.strip_prefix(gm::COMMAND_PREFIX) {
            if let Some(&actor) = self.state.players.get(&connection) {
                let is_gm = self
                    .state
                    .registry
                    .get::<Access>(actor)
                    .is_some_and(|access| access.0 >= AccessLevel::GameMaster);
                if is_gm {
                    gm::run(&mut self.state, actor, rest);
                    return;
                }
            }
        }
        chat::say(&mut self.state, connection, mode, hue, font, &text);

        // Townsperson services triggered by keyword: saying "bank" near a banker
        // opens your bank box. The words were still spoken above, so it reads as a
        // request the banker answers, not a hidden command.
        if let Some(&actor) = self.state.players.get(&connection) {
            npc::banker_keywords(&mut self.state, connection, actor, &text);
        }
    }

    /// Answer a single-click (`0x09`): draw the clicked mobile's name over its
    /// head, seen only by the asker, in its notoriety colour.
    ///
    /// Mobiles with a name only — a townsperson, a player. A nameless creature and
    /// a plain item say nothing rather than a blank label; item names wait on a
    /// tiledata name lookup.
    fn single_click(&mut self, connection: ConnectionId, serial: u32) {
        let Some(target) = Serial::new(serial).and_then(|s| self.state.registry.entity_of(s))
        else {
            return;
        };
        let Some(name) = self.state.registry.get::<Name>(target) else {
            return;
        };
        let name = name.0.clone();
        let Some(body) = self.state.registry.get::<Body>(target).map(|b| b.id) else {
            return;
        };
        let hue = self
            .state
            .registry
            .get::<Notoriety>(target)
            .copied()
            .unwrap_or(Notoriety::Innocent)
            .name_hue();
        // The object's own serial makes the client draw the text over it; an empty
        // speaker name and the label mode make it a name tag, not speech.
        let packet = encode_message(serial, body, LABEL_MODE, hue, 3, "", &name);
        self.state.send(connection, packet);
    }

    fn walk(&mut self, connection: ConnectionId, request: WalkRequest, now: Instant) {
        let Some(&entity) = self.state.players.get(&connection) else {
            // A walk before a character. Not fatal — a stray packet from a
            // client that reconnected — but nothing to act on either.
            debug!(%connection, "0x02 from a connection with no character");
            return;
        };
        let Some(serial) = self.state.registry.serial_of(entity) else {
            return;
        };
        let Some(Movement(mut walker)) = self.state.registry.get::<Movement>(entity).copied()
        else {
            return;
        };

        let facet = self.state.facet_of(entity);
        let was = walker.position;
        let out_of_sequence = walker.sequence.is_fresh() && request.sequence != 0;
        let outcome = match &self.state.facet_state(facet).terrain {
            Some(terrain) => walker.request(request, terrain.as_ref(), now),
            None => walker.request(request, &OpenWorld, now),
        };
        self.state.registry.insert(entity, Movement(walker));

        match outcome {
            Walk::Moved { position, facing } => {
                self.state.registry.insert(entity, Position(position));
                self.state.registry.insert(entity, Heading(facing));
                // The index is a second copy of the position; this is the line
                // that keeps it honest.
                self.state
                    .facet_state_mut(facet)
                    .sectors
                    .insert(entity, position);
                self.state.send(
                    connection,
                    encode_walk_ack(request.sequence, NOTORIETY_INNOCENT),
                );
                self.state.bus.send(MobileMoved {
                    entity,
                    serial,
                    from: was,
                    to: position,
                    facing,
                });
                self.state.refresh_around(entity);
            }
            Walk::Turned { facing } => {
                self.state.registry.insert(entity, Heading(facing));
                self.state.send(
                    connection,
                    encode_walk_ack(request.sequence, NOTORIETY_INNOCENT),
                );
                self.state.bus.send(MobileTurned {
                    entity,
                    serial,
                    facing,
                });
                // A turn moves nobody, but it changes what everyone watching
                // draws — the client animates a facing it is told about.
                self.state.broadcast_move(entity);
            }
            Walk::Refused => {
                // Which of the three it was is not something `Walk` says, and
                // teaching it to would put the reasons in the wrong crate. The
                // sequence is checked before anything else, so a fresh walker
                // with a non-zero sequence can only have failed that; past it,
                // the pace and the terrain are the two left and this cannot yet
                // tell them apart. Better a coarse reason than a wrong one.
                let reason = if out_of_sequence {
                    RefusedReason::OutOfSequence
                } else {
                    RefusedReason::Blocked
                };
                self.state.send(
                    connection,
                    encode_walk_reject(request.sequence, walker.position, walker.facing),
                );
                self.state.bus.send(StepRefused {
                    entity,
                    serial,
                    reason,
                });
                debug!(%serial, ?reason, "step refused");
            }
        }
    }

    /// Move a mobile one step by server decree. See [`Command::Step`].
    ///
    /// Shares the interest-management tail with [`walk`](Self::walk) —
    /// [`refresh_around`](Self::refresh_around) and
    /// [`broadcast_move`](Self::broadcast_move) — because a mobile the server
    /// moved has to appear on the same screens, and leave the same ones, as a
    /// mobile that walked itself. What it does not share is the client half:
    /// there is no `0x22`/`0x21` ack, because there may be no client, and the
    /// mobile might be an NPC nobody is driving.
    fn step(&mut self, serial: u32, direction: u8) {
        let Some(serial) = Serial::new(serial) else {
            return;
        };
        let Some(entity) = self.state.registry.entity_of(serial) else {
            return;
        };
        let Some(Movement(mut walker)) = self.state.registry.get::<Movement>(entity).copied()
        else {
            return;
        };
        let direction = Direction::from_bits(direction);
        let facet = self.state.facet_of(entity);
        let was = walker.position;

        // Turn-as-step: a mobile not yet facing this way turns and stays put.
        if walker.facing.direction != direction {
            let facing = Facing::walking(direction);
            walker.facing = facing;
            self.state.registry.insert(entity, Movement(walker));
            self.state.registry.insert(entity, Heading(facing));
            self.state.bus.send(MobileTurned {
                entity,
                serial,
                facing,
            });
            self.state.broadcast_move(entity);
            return;
        }

        let Some(target) = step_from(walker.position, direction) else {
            // Off the edge of the coordinate space — nowhere to go, and no client
            // to snap back, so it is simply refused.
            self.state.bus.send(StepRefused {
                entity,
                serial,
                reason: RefusedReason::Blocked,
            });
            return;
        };
        let landed = match &self.state.facet_state(facet).terrain {
            Some(terrain) => terrain.can_step(walker.position, target),
            None => OpenWorld.can_step(walker.position, target),
        };
        let Some(landed) = landed else {
            self.state.bus.send(StepRefused {
                entity,
                serial,
                reason: RefusedReason::Blocked,
            });
            return;
        };

        let facing = Facing::walking(direction);
        walker.position = landed;
        walker.facing = facing;
        self.state.registry.insert(entity, Movement(walker));
        self.state.registry.insert(entity, Position(landed));
        self.state.registry.insert(entity, Heading(facing));
        self.state
            .facet_state_mut(facet)
            .sectors
            .insert(entity, landed);
        self.state.bus.send(MobileMoved {
            entity,
            serial,
            from: was,
            to: landed,
            facing,
        });
        self.state.refresh_around(entity);
    }

    /// Put a mobile in the world. See [`Command::SpawnMobile`].
    ///
    /// The same bundle a player is built from — a body, a position, a facing, a
    /// walker, hit points — minus the [`Client`]. That absence is the whole
    /// difference between a creature and a person; everything that draws or moves
    /// a mobile already treats "has a client" as the question, so a spawned one
    /// falls out of the machinery already there.
    #[allow(clippy::too_many_arguments)]
    fn spawn_mobile(&mut self, spec: SpawnMobile) -> Option<EntityId> {
        let SpawnMobile {
            body,
            hue,
            hits,
            notoriety,
            damage,
            resistance,
            swing,
            sight,
            wander,
            position,
            facet,
            name,
            banker,
            equipment,
        } = spec;
        let facet = if self.state.facets.contains_key(&facet) {
            facet
        } else {
            warn!(facet, "unloaded facet; spawning the mobile on the default");
            self.state.default_facet
        };
        // Drop the mobile onto the ground, the way a client's spawner does: the
        // pack gives x/y and a rough height, and the floor it stands on — the top
        // of the static surface there, a building's raised floor and all — is the
        // map's to say. Without this a banker sinks to the given z and reads as
        // "inside a wall".
        let position = match self
            .state
            .facet_state(facet)
            .terrain
            .as_ref()
            .and_then(|t| t.stand_z(position.x, position.y, i32::from(position.z)))
            .and_then(|z| i8::try_from(z).ok())
        {
            Some(z) => Point::new(position.x, position.y, z),
            None => position,
        };
        let (entity, serial) = match self.state.registry.spawn_with_serial(SerialKind::Mobile) {
            Ok(pair) => pair,
            Err(error) => {
                warn!(?error, "out of mobile serials; not spawning");
                return None;
            }
        };
        let hits = hits.max(1);
        let facing = Facing::walking(Direction::South);
        self.state.registry.insert(entity, Body { id: body, hue });
        self.state.registry.insert(entity, Position(position));
        self.state.registry.insert(entity, Heading(facing));
        self.state.registry.insert(entity, Facet(facet));
        self.state.registry.insert(
            entity,
            Hitpoints {
                current: hits,
                max: hits,
            },
        );
        self.state
            .registry
            .insert(entity, Notoriety::from_bits(notoriety));
        self.state
            .registry
            .insert(entity, MeleeDamage { amount: damage });
        self.state.registry.insert(
            entity,
            Resistance {
                physical: resistance.min(100),
                ..Default::default()
            },
        );
        // Zero means "derive from dexterity", so a script that does not care about
        // pace names no number and gets the wrestling formula. A non-zero value
        // pins an exact cadence — a special creature that ignores its stats.
        if swing != 0 {
            self.state
                .registry
                .insert(entity, SwingSpeed { ticks: swing });
        }
        // A brain only for a creature that needs one — something that hunts or
        // wanders. A pure prop (a shopkeeper standing still) gets none and never
        // enters `think`. `Combat` it earns when it first picks a fight.
        if sight > 0 || wander {
            self.state.registry.insert(
                entity,
                Brain {
                    sight,
                    wander,
                    next_think: 0,
                },
            );
        }
        // A banker earns a generated name and title ("Rowena the banker") when the
        // spawn did not name it, the townsperson AI base (so it greets, faces and
        // keeps near its post), and the service mark that answers "bank".
        let name = if banker && name.is_none() {
            Some(npc::banker_name(&mut self.state.rng))
        } else {
            name
        };
        if let Some(name) = name {
            self.state.registry.insert(entity, Name(name));
        }
        if banker {
            self.state.registry.insert(entity, Banker { next_greet: 0 });
            self.state.registry.insert(
                entity,
                Npc {
                    home: position,
                    wander: BANKER_WANDER,
                    next_beat: 0,
                },
            );
        }
        // Dress it before the reveal, so the clothing rides in the `0x78` that
        // draws it — a naked banker is a bug that looks like nudity.
        for (graphic, layer, item_hue) in equipment {
            items::equip_worn_item(&mut self.state, serial, graphic, item_hue, layer);
        }
        self.state
            .registry
            .insert(entity, Movement(Walker::new(position, facing)));
        self.state
            .facet_state_mut(facet)
            .sectors
            .insert(entity, position);
        self.state.reveal(entity);
        // Say who and where, so a script can take control of it: the mobile
        // counterpart of `PlayerEntered`, and how `op_control` learns a serial.
        self.state.bus.send(MobileSpawned {
            entity,
            serial,
            position,
        });
        debug!(%serial, body, "mobile spawned");
        Some(entity)
    }

    /// Act on a targeting cursor's answer. Looks up what the cursor was raised for
    /// and, if the click was not cancelled, does it. A cancel just clears the
    /// pending target.
    fn handle_target(
        &mut self,
        connection: ConnectionId,
        response: openshard_protocol::TargetResponse,
    ) {
        let Some(&actor) = self.state.players.get(&connection) else {
            return;
        };
        let Some(purpose) = self.state.pending_targets.remove(&actor) else {
            return; // no cursor was up for this mobile
        };
        if response.cancelled {
            return;
        }
        match purpose {
            openshard_state::TargetPurpose::Teleport => {
                crate::gm::teleport_to(&mut self.state, actor, response.location);
            }
        }
    }

    /// Act on an admin-gump button. The gump crate reads the response and gates it
    /// (game-master only); the acting — registering or clearing spawn regions,
    /// which only the tick can touch — is here.
    fn handle_admin_gump(
        &mut self,
        connection: ConnectionId,
        response: openshard_protocol::GumpResponse,
    ) {
        let Some((actor, verb)) = crate::admin::button_action(&self.state, connection, &response)
        else {
            return;
        };
        // The engine holds no spawn data: it emits the verb, and the script pack —
        // where a shard's spawns are edited without a rebuild — decides what it
        // means, registering regions through `op_register_spawner` or clearing them.
        if let Some(serial) = self.state.registry.serial_of(actor) {
            self.state.bus.send(AdminMenuAction {
                serial,
                action: verb.to_owned(),
            });
        }
        gm::notify(&mut self.state, actor, &format!("Admin: {verb}."));
    }

    /// Keep every spawn region at its ceiling. Once per tick, but cheap: a region
    /// not yet due to respawn is a single counter check, and only a due one that
    /// is short a creature does the work of counting and spawning. One creature
    /// per region per pass, so a wiped region refills at its own pace rather than
    /// snapping back full in a tick. Deterministic — the picks draw on the world's
    /// seeded rng, so a replay repopulates the same.
    fn maintain_spawners(&mut self) {
        let now = self.state.ticks;
        for index in 0..self.spawners.len() {
            if now < self.spawners[index].next_spawn {
                continue;
            }
            let id = index as u32;
            let live = self
                .state
                .registry
                .query::<SpawnedBy>()
                .filter(|(_, owner)| owner.0 == id)
                .count() as u16;
            let spawner = &self.spawners[index];
            if spawner.creatures.is_empty() || live >= spawner.max_count {
                continue;
            }

            // Pick a creature and a tile with the tick's rng.
            let area = spawner.area;
            let which = self.state.rng.below(spawner.creatures.len() as u32) as usize;
            let creature = spawner.creatures[which].clone();
            let delay = spawner.respawn_delay;
            let facet = area.facet;
            let dx = self.state.rng.below(u32::from(area.width.max(1)));
            let dy = self.state.rng.below(u32::from(area.height.max(1)));
            let x = area.x.wrapping_add(dx as u16);
            let y = area.y.wrapping_add(dy as u16);

            // Stand it on the ground the client will compute, or a flat default
            // where there is no map.
            let z = self
                .state
                .facet_state(facet)
                .terrain
                .as_ref()
                .and_then(|terrain| terrain.ground_z(x, y))
                .unwrap_or(0);

            if let Some(entity) = self.spawn_mobile(SpawnMobile {
                body: creature.body,
                hue: creature.hue,
                hits: creature.hits,
                notoriety: creature.notoriety,
                damage: creature.damage,
                resistance: creature.resistance,
                swing: creature.swing,
                sight: creature.sight,
                wander: creature.wander,
                position: Point::new(x, y, z),
                facet,
                // A maintained spawn is a monster or an animal, never a named
                // townsperson; those are placed once, not respawned.
                name: None,
                banker: false,
                equipment: Vec::new(),
            }) {
                self.state.registry.insert(entity, SpawnedBy(id));
            }
            self.spawners[index].next_spawn = now + delay;
        }
    }

    /// Drop every spawn region and despawn the creatures they were maintaining —
    /// Register a spawn region, giving it a fresh id and replacing any earlier one
    /// over the same box. Re-running the pack's "populate" does not stack a second
    /// spawner on a region — it re-places it, with a reset timer — and after a
    /// restart the regions come from the store, not from here, so their timers hold.
    fn register_spawner(&mut self, mut spawner: crate::spawner::Spawner) {
        self.spawners.retain(|s| s.area != spawner.area);
        spawner.id = self.next_spawner_id;
        self.next_spawner_id += 1;
        self.spawners.push(spawner);
    }

    /// The spawn regions as saveable records. The live timer is a tick count; it is
    /// saved as the *seconds still to wait*, so it means the same after a restart
    /// resets the tick counter — a rare spawn killed with hours left comes back with
    /// those hours ahead of it, and downtime does not spend them.
    fn spawner_records(&self) -> Vec<openshard_persistence::SpawnerRecord> {
        let now = self.state.ticks;
        self.spawners
            .iter()
            .map(|s| openshard_persistence::SpawnerRecord {
                id: s.id,
                facet: s.area.facet,
                x: s.area.x,
                y: s.area.y,
                width: s.area.width,
                height: s.area.height,
                max_count: s.max_count,
                respawn_secs: s.respawn_delay / TICKS_PER_SECOND,
                remaining_secs: s.next_spawn.saturating_sub(now) / TICKS_PER_SECOND,
                creatures: s
                    .creatures
                    .iter()
                    .map(|c| openshard_persistence::CreatureData {
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
                    .collect(),
            })
            .collect()
    }

    /// Re-create the spawn regions from saved records at boot. The remaining-wait
    /// seconds become a tick offset from now (the tick counter is zero at boot), so
    /// the timer resumes where it stood; downtime is not counted against it. Call
    /// once, before anyone connects.
    pub fn restore_spawners(&mut self, records: Vec<openshard_persistence::SpawnerRecord>) {
        let now = self.state.ticks;
        for record in records {
            self.next_spawner_id = self.next_spawner_id.max(record.id + 1);
            let area = crate::spawner::SpawnArea {
                x: record.x,
                y: record.y,
                width: record.width,
                height: record.height,
                facet: record.facet,
            };
            let creatures = record
                .creatures
                .into_iter()
                .map(|c| crate::spawner::CreatureTemplate {
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
            let mut spawner = crate::spawner::Spawner::new(
                record.id,
                area,
                creatures,
                record.max_count,
                record.respawn_secs * TICKS_PER_SECOND,
            );
            spawner.next_spawn = now + record.remaining_secs * TICKS_PER_SECOND;
            self.spawners.push(spawner);
        }
    }

    /// "Clear spawns". A creature belongs to a region by its [`SpawnedBy`]; taking
    /// it off every screen before despawning, so no client is left drawing a ghost.
    fn clear_spawners(&mut self) {
        self.spawners.clear();
        let owned: Vec<EntityId> = self
            .state
            .registry
            .query::<SpawnedBy>()
            .map(|(entity, _)| entity)
            .collect();
        for entity in owned {
            let serial = self.state.registry.serial_of(entity);
            let facet = self.state.facet_of(entity);
            if let Some(serial) = serial {
                for watcher in self.state.watchers_of(entity) {
                    self.state.forget(watcher, entity, serial);
                }
            }
            self.state.facet_state_mut(facet).sectors.remove(entity);
            self.state.registry.despawn(entity);
        }
    }

    /// Place a batch of decoration: script-added statics the shard puts on top of
    /// the map's art, plus the interactive kinds — doors and containers. Each is an
    /// item — a `Graphic` and a `Position`, drawn to clients through the same
    /// `0x1A`/interest path as any item — but marked [`Decoration`], so it never
    /// decays and cannot be picked up. A door also carries [`Door`] (toggled by
    /// double-click) and a container [`Container`] (opened by double-click). See
    /// [`crate::gm`] and `items::pick_up`.
    fn decorate(
        &mut self,
        facet: u8,
        statics: &[(u16, u16, Point)],
        doors: &[DecorDoor],
        containers: &[DecorContainer],
    ) {
        let facet = if self.state.facets.contains_key(&facet) {
            facet
        } else {
            self.state.default_facet
        };
        // A closure that spawns one decoration item at a tile and reveals it,
        // returning the entity so the caller can hang a `Door` or `Container` on
        // it. `None` when the serial pool is empty.
        for &(graphic, hue, position) in statics {
            if self
                .place_decoration(facet, graphic, hue, position)
                .is_none()
            {
                return;
            }
        }
        for door in doors {
            let Some(entity) = self.place_decoration(facet, door.closed, 0, door.position) else {
                return;
            };
            self.state.registry.insert(
                entity,
                Door {
                    closed: door.closed,
                    open: door.open,
                    offset_x: door.offset_x,
                    offset_y: door.offset_y,
                    is_open: false,
                    close_at: 0,
                },
            );
        }
        for container in containers {
            let Some(entity) =
                self.place_decoration(facet, container.graphic, container.hue, container.position)
            else {
                return;
            };
            self.state.registry.insert(
                entity,
                Container {
                    gump: container.gump,
                },
            );
        }
    }

    /// Spawn one decoration item — a `Graphic`, `Position`, `Facet` and the
    /// [`Decoration`] marker — index it and draw it to everyone in range. Returns
    /// its entity, or `None` if the item-serial pool is empty (the caller stops the
    /// batch).
    fn place_decoration(
        &mut self,
        facet: u8,
        graphic: u16,
        hue: u16,
        position: Point,
    ) -> Option<EntityId> {
        let Ok((entity, _serial)) = self.state.registry.spawn_with_serial(SerialKind::Item) else {
            warn!("out of item serials; stopping decoration");
            return None;
        };
        self.state
            .registry
            .insert(entity, Graphic { id: graphic, hue });
        self.state.registry.insert(entity, Position(position));
        self.state.registry.insert(entity, Facet(facet));
        self.state.registry.insert(entity, Decoration);
        self.state
            .facet_state_mut(facet)
            .sectors
            .insert(entity, position);
        self.state.reveal(entity);
        Some(entity)
    }

    /// Generate functional doors from the map's static door frames in a region.
    ///
    /// ServUO's `DoorGenerator`, ported (see [`crate::doorgen`]): where a west
    /// frame faces an east frame across a one- or two-tile gap — or a north faces a
    /// south — a `DarkWoodDoor` (single) or a linked pair (double) is dropped into
    /// the gap, so a building's implied shop door becomes one that opens. Reading
    /// the terrain and placing entities cannot overlap borrows, so the scan
    /// collects every placement first and lays them down after.
    fn generate_doors(&mut self, facet: u8, x: u16, y: u16, width: u16, height: u16) {
        let facet = if self.state.facets.contains_key(&facet) {
            facet
        } else {
            self.state.default_facet
        };

        // Tiles that already hold a door — the named metal/special doors placed
        // from decoration data, and doors generated earlier in this same pass. A
        // generated door never lands on one of these, which is what stops the bank
        // door being doubled and a doorway being filled twice.
        let door_entities: Vec<EntityId> = self
            .state
            .registry
            .query::<Door>()
            .map(|(entity, _)| entity)
            .collect();
        let mut occupied: HashSet<(u16, u16)> = HashSet::new();
        for entity in door_entities {
            if self.state.facet_of(entity) == facet {
                if let Some(&Position(p)) = self.state.registry.get::<Position>(entity) {
                    occupied.insert((p.x, p.y));
                }
            }
        }

        // (closed, open, offset_x, offset_y, where-it-sits-closed).
        let mut placements: Vec<(u16, u16, i16, i16, Point)> = Vec::new();
        {
            let Some(terrain) = self.state.facet_state(facet).terrain.as_ref() else {
                warn!(facet, "no map on this facet; no doors to generate");
                return;
            };
            // Is there a frame of the given side at (tx, ty) sharing height z?
            let frame_at = |tx: u16, ty: u16, tz: i8, pred: fn(u16) -> bool| -> bool {
                let mut here = Vec::new();
                terrain.statics_at(tx, ty, &mut here);
                here.iter().any(|&(id, z)| z == tz && pred(id))
            };
            // Place a door in the gap, but only if a door actually fits there — an
            // open doorway with a floor, not a solid wall or thin air — and it is
            // not already doored. `can_fit` is ServUO's `CanFit` guard (16 tall);
            // the `occupied` set is our own de-dup.
            let mut try_place = |gap: Point, door: (u16, u16, i16, i16)| {
                let key = (gap.x, gap.y);
                if occupied.contains(&key) || !terrain.can_fit(gap.x, gap.y, i32::from(gap.z), 16) {
                    return;
                }
                occupied.insert(key);
                let (c, o, ox, oy) = door;
                placements.push((c, o, ox, oy, gap));
            };
            let east = |vx: u16| vx.checked_add(2);
            let mut here = Vec::new();
            for ry in 0..height {
                for rx in 0..width {
                    let (Some(vx), Some(vy)) = (x.checked_add(rx), y.checked_add(ry)) else {
                        continue;
                    };
                    here.clear();
                    terrain.statics_at(vx, vy, &mut here);
                    for &(id, z) in &here {
                        if doorgen::is_west_frame(id) {
                            // A single door: one gap tile to an east frame two away.
                            if east(vx).is_some_and(|e| frame_at(e, vy, z, doorgen::is_east_frame))
                            {
                                try_place(
                                    Point::new(vx + 1, vy, z),
                                    doorgen::GenFacing::WestCw.door(),
                                );
                            } else if vx
                                .checked_add(3)
                                .is_some_and(|e| frame_at(e, vy, z, doorgen::is_east_frame))
                            {
                                // A double door fills the two-tile gap.
                                try_place(
                                    Point::new(vx + 1, vy, z),
                                    doorgen::GenFacing::WestCw.door(),
                                );
                                try_place(
                                    Point::new(vx + 2, vy, z),
                                    doorgen::GenFacing::EastCcw.door(),
                                );
                            }
                        } else if doorgen::is_north_frame(id) {
                            if vy
                                .checked_add(2)
                                .is_some_and(|s| frame_at(vx, s, z, doorgen::is_south_frame))
                            {
                                try_place(
                                    Point::new(vx, vy + 1, z),
                                    doorgen::GenFacing::SouthCw.door(),
                                );
                            } else if vy
                                .checked_add(3)
                                .is_some_and(|s| frame_at(vx, s, z, doorgen::is_south_frame))
                            {
                                try_place(
                                    Point::new(vx, vy + 1, z),
                                    doorgen::GenFacing::NorthCcw.door(),
                                );
                                try_place(
                                    Point::new(vx, vy + 2, z),
                                    doorgen::GenFacing::SouthCw.door(),
                                );
                            }
                        }
                    }
                }
            }
        }

        let count = placements.len();
        for (closed, open, offset_x, offset_y, position) in placements {
            if let Some(entity) = self.place_decoration(facet, closed, 0, position) {
                self.state.registry.insert(
                    entity,
                    Door {
                        closed,
                        open,
                        offset_x,
                        offset_y,
                        is_open: false,
                        close_at: 0,
                    },
                );
            }
        }
        debug!(facet, count, "generated doors from static frames");
    }

    /// Remove every script-placed decoration — "Clear deco".
    fn clear_decorations(&mut self) {
        let placed: Vec<EntityId> = self
            .state
            .registry
            .query::<Decoration>()
            .map(|(entity, _)| entity)
            .collect();
        for entity in placed {
            let serial = self.state.registry.serial_of(entity);
            let facet = self.state.facet_of(entity);
            if let Some(serial) = serial {
                for watcher in self.state.watchers_of(entity) {
                    self.state.forget(watcher, entity, serial);
                }
            }
            self.state.facet_state_mut(facet).sectors.remove(entity);
            self.state.registry.despawn(entity);
        }
    }

    /// Give every brain due a beat. The deciding is [`ai::think_one`]'s; the world
    /// only applies the one thing a brain cannot do itself — a step — since it
    /// owns movement. A creature that gets a `Combat` from the brain is fought by
    /// `combat::swings` exactly as a player would be.
    fn think(&mut self) {
        let now = self.state.ticks;
        let thinkers: Vec<EntityId> = self
            .state
            .registry
            .query::<Brain>()
            .filter(|(_, brain)| now >= brain.next_think)
            .map(|(entity, _)| entity)
            .collect();
        for creature in thinkers {
            // A script-controlled mobile is driven by its `onTick`, not here; the
            // built-in brain stays out of its way.
            if self.state.registry.has::<Scripted>(creature) {
                continue;
            }
            if let Some(dir) = ai::think_one(&mut self.state, creature) {
                if let Some(serial) = self.state.registry.serial_of(creature) {
                    self.step(serial.raw(), dir);
                }
            }
            if let Some(brain) = self.state.registry.get_mut::<Brain>(creature) {
                brain.next_think = now + AI_THINK_TICKS;
            }
        }
    }

    /// The wire serials of every mobile a script has taken control of. The server
    /// reads this each tick and calls each one's `onTick`.
    #[must_use]
    pub fn scripted(&self) -> Vec<Serial> {
        self.state
            .registry
            .query::<Scripted>()
            .filter_map(|(entity, _)| self.state.registry.serial_of(entity))
            .collect()
    }

    /// Hand a mobile's brain to the script: it stops thinking on its own and its
    /// `onTick` drives it instead. See [`Command::Control`].
    fn control(&mut self, serial: u32) {
        if let Some(entity) = Serial::new(serial).and_then(|s| self.state.registry.entity_of(s)) {
            self.state.registry.insert(entity, Scripted);
        }
    }

    /// A client asked to cast a spell: say so on the bus for a script to act on.
    /// The world does not cast — it does not know what the spell costs or does.
    /// See [`Command::RequestCast`].
    fn request_cast(&mut self, connection: ConnectionId, spell: u16) {
        let Some(&entity) = self.state.players.get(&connection) else {
            return;
        };
        let Some(serial) = self.state.registry.serial_of(entity) else {
            return;
        };
        self.state.bus.send(SpellRequested {
            entity,
            serial,
            spell,
        });
    }

    fn disconnect(&mut self, connection: ConnectionId) {
        // A client that logs out mid-drag would otherwise leave its item nowhere —
        // off the ground and out of any container, on a cursor that is gone. Put
        // it back where it was.
        if let Some(held) = self.state.held.remove(&connection) {
            items::restore(&mut self.state, held);
        }
        // Forget any containers it had open; a gone connection watches nothing.
        self.state.open_containers.retain(|_, watchers| {
            watchers.remove(&connection);
            !watchers.is_empty()
        });

        let Some(entity) = self.state.players.remove(&connection) else {
            return;
        };
        // Forget any targeting cursor it had up: a gone mobile clicks nothing.
        self.state.pending_targets.remove(&entity);
        let serial = self.state.registry.serial_of(entity);
        let facet = self.state.facet_of(entity);

        // Save before despawning, and not by marking it dirty: a `touch` is a
        // promise to read the entity at the next save, and in a moment there
        // will be no entity to read. Logging out is when a save matters most —
        // it is the only moment a player's whole session is at stake — so the
        // record is taken at the one instant it still can be.
        if let Some(record) = Self::record_of(&self.state.registry, entity) {
            // The journal copy is for the store; the departed copy is for the
            // server's in-memory character list, which a re-login reads before the
            // deferred store save has necessarily landed.
            self.departed.push(record.clone());
            // The carried inventory, walked now for the same reason as the record:
            // in a moment the items are despawned with the character and there is
            // nothing left to read. Two copies, for two readers: the journal's for
            // the store, and `pending_inventories` so a re-login *this run* re-equips
            // it — the same fast-relogin path the departed record cache serves, and
            // without it a relog before the next save loses everything carried.
            let items = self.inventory_of(entity);
            self.pending_inventories
                .insert(record.serial, items.clone());
            self.journal.keep_inventory(Inventory {
                owner: record.serial,
                items,
            });
            self.journal.keep(record);
        }

        // Take it off every screen *before* despawning: once the entity is gone
        // its serial is released and there is nothing left to tell anyone about.
        if let Some(serial) = serial {
            for watcher in self.state.watchers_of(entity) {
                self.state.forget(watcher, entity, serial);
            }
        }
        self.state.seen.remove(&entity);
        self.state.facet_state_mut(facet).sectors.remove(entity);
        // The character's worn items — its backpack and whatever is in it — are
        // not saved yet, so they go with it rather than orphaning on a serial that
        // is about to be released and reused.
        if let Some(serial) = serial {
            items::despawn_belongings(&mut self.state, serial);
        }
        self.state.registry.despawn(entity);

        if let Some(serial) = serial {
            self.state.bus.send(PlayerLeft { entity, serial });
            info!(%serial, "left the world");
        }
    }
}

#[cfg(test)]
mod interest_tests;
#[cfg(test)]
mod persistence_tests;
#[cfg(test)]
pub(crate) mod tests;
