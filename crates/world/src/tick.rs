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
use openshard_movement::{step_from, Terrain, Walk, Walker};
use openshard_persistence::{
    CharacterRecord, DecorationRecord, DoorState, Inventory, ItemLocation, ItemRecord, Journal,
    MobileRecord, Snapshot, SCHEMA_VERSION,
};
use openshard_protocol::{
    encode_context_menu, encode_light_level, encode_login_complete, encode_map_change,
    encode_message, encode_supported_features, encode_walk_ack, encode_walk_reject, AccessLevel,
    ClientVersion, Direction, Facing, Feature, MobileStatus, Notoriety, PlayerStart, PlayerUpdate,
    Point, WalkRequest, AOS_FEATURE_FLAGS, DEFAULT_MAP_HEIGHT, DEFAULT_MAP_WIDTH, LABEL_MODE,
};
use tracing::{debug, info, warn};

use openshard_state::components::{
    Access, Account, Amount, Body, Brain, Client, Combat, Contained, Container, DamageType,
    Decoration, Door, Equipped, Facet, Ghost, Graphic, Heading, Hitpoints, Mana, MeleeDamage,
    Movement, Name, Position, Resistance, Ridden, Riding, Scripted, SpawnedBy, Spellbook,
    Stackable, Stats, Vendor,
};
use openshard_state::rng::Rng;
use openshard_state::sectors::Sectors;
use openshard_state::{
    FacetState, Gameplay, Obstructions, Outbound, TooltipMode, WorldState, TICKS_PER_SECOND,
};

use openshard_ai as ai;
use openshard_chat as chat;
use openshard_combat as combat;
use openshard_items as items;
use openshard_magic as magic;
use openshard_npc as npc;
use openshard_skills as skills;

use crate::doorgen;
use crate::events::{
    AdminMenuAction, CorpseCreated, MobileMoved, MobileTurned, PlayerEntered, PlayerLeft,
    RefusedReason, StepRefused,
};
use crate::gm;
use crate::terrain::MapTerrain;

mod command;
mod context;
mod death;
mod decor;
mod defaults;
mod enter;
mod motion;
mod persist;
mod skills_wire;
mod spawners;
mod speech;
mod spells;
mod staff;

pub use command::{Appearance, CharacterSheet, Command, DecorContainer, DecorDoor};
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
    sheet: Option<CharacterSheet>,
    access: AccessLevel,
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
    /// What combat reported hit, for the AI's retaliation.
    damaged: Cursor<openshard_combat::MobileDamaged>,
    /// Read to find out what to mark dirty. See `mark_dirty`.
    turned: Cursor<MobileTurned>,
    /// Skill gains this tick, to push the single-line `0x3A` update to the owner.
    raised: Cursor<openshard_skills::SkillRaised>,
    /// Damage this tick, read to disturb a spell mid-cast (the `spell_disturb`
    /// rule); a separate cursor from `damaged`, which the AI reads for its own.
    disturbed: Cursor<openshard_combat::MobileDamaged>,
    /// Deaths this tick, read by `reap` to lay a corpse where a creature fell.
    dead: Cursor<openshard_combat::MobileDied>,
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
                obstructions: Obstructions::default(),
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
            damaged: Cursor::default(),
            turned: Cursor::default(),
            raised: Cursor::default(),
            disturbed: Cursor::default(),
            dead: Cursor::default(),
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
                obstructions: Obstructions::default(),
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
        combat::volleys(&mut self.state);
        combat::expire_criminality(&mut self.state);
        combat::decay_murders(&mut self.state);
        combat::poison_tick(&mut self.state);
        // Lift the stat buffs whose time is up, and redraw the bar for any player
        // whose stats just changed back — the decide-then-apply split again.
        let now = self.state.ticks;
        for entity in magic::expire_buffs(&mut self.state, now) {
            if let Some(serial) = self.state.registry.serial_of(entity) {
                self.refresh_status_of(serial.raw());
            }
        }
        magic::regen_mana(&mut self.state);
        // Finish or break the ServUO-style casts whose delay is up or whose
        // caster was struck; the Sphere style resolves in `begin_cast` and never
        // reaches here.
        self.advance_casts();
        // Lay a corpse where each creature fell this tick — after every source of
        // death (a swing, a volley, poison, a spell, a command) has had its turn.
        self.reap();
        items::decay(&mut self.state);
        items::close_doors(&mut self.state);
        self.maintain_spawners();

        // Follow this tick's skill gains on any open window. Before `update`
        // retires the events, like `mark_dirty`.
        self.send_skill_updates();
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
                sheet,
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
                sheet,
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
            Command::RequestSkills { connection } => {
                if let Some(&entity) = self.state.players.get(&connection) {
                    self.send_skills(connection, entity);
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
                aggression,
                beat,
                wander,
                ranged,
                ranged_kind,
                position,
                facet,
                name,
                banker,
                vendor,
                equipment,
            } => {
                npc::spawn(
                    &mut self.state,
                    npc::SpawnSpec {
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
                        wander,
                        ranged,
                        ranged_kind,
                        position,
                        facet,
                        name,
                        banker,
                        vendor,
                        equipment,
                    },
                );
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
            Command::SetSkillLock {
                connection,
                skill,
                lock,
            } => self.set_skill_lock(connection, skill, lock),
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
                debug!(
                    serial = format!("0x{serial:08X}"),
                    paperdoll = serial & 0x8000_0000 != 0,
                    "double-click"
                );
                // Bit 31 is the client's *paperdoll request* — the login-time
                // paperdoll open, the paperdoll macro — and it is only that:
                // ServUO's `UseReq` routes it straight to `OnPaperdollRequest`,
                // never to `Use`. A raw double-click carries the bare serial.
                // Stripping the bit and treating both alike was the bug where
                // relogging mounted dismounted you a breath later: the client's
                // paperdoll-open read as a self-double-click.
                if serial & 0x8000_0000 != 0 {
                    items::paperdoll_request(&mut self.state, connection, serial & 0x7FFF_FFFF);
                } else if !npc::open_shop(&mut self.state, connection, serial) {
                    // A vendor's shop first: if the click was a shopkeeper in
                    // range the buy gump answers it; anything else is the
                    // ordinary use rule.
                    items::double_click(&mut self.state, connection, serial);
                }
            }
            Command::SingleClick { connection, serial } => self.single_click(connection, serial),
            Command::QueryProperties {
                connection,
                serials,
            } => self.query_properties(connection, &serials),
            Command::ContextMenuRequest { connection, serial } => {
                self.context_menu_request(connection, serial);
            }
            Command::ContextMenuSelect {
                connection,
                serial,
                index,
            } => self.context_menu_select(connection, serial, index),
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
            Command::RequestCast { connection, spell } => self.begin_cast(connection, spell),
            Command::StockVendor { serial, stock } => {
                npc::stock(&mut self.state, serial, stock);
            }
            Command::AddLoot {
                container,
                graphic,
                hue,
                amount,
                stackable,
            } => self.add_loot(container, graphic, hue, amount, stackable),
            Command::ConsumeItem { serial, amount } => {
                if let Some(serial) = Serial::new(serial) {
                    items::consume(&mut self.state, serial, amount);
                }
            }
            Command::Buy {
                connection,
                vendor,
                purchases,
            } => npc::buy(&mut self.state, connection, vendor, &purchases),
            Command::Sell {
                connection,
                vendor,
                sales,
            } => npc::sell(&mut self.state, connection, vendor, &sales),
        }
    }

    /// Give every brain due a beat. The deciding is [`ai::think_one`]'s; the world
    /// only applies the one thing a brain cannot do itself — a step — since it
    /// owns movement. A creature that gets a `Combat` from the brain is fought by
    /// `combat::swings` exactly as a player would be.
    fn think(&mut self) {
        // Violence answered first: whoever was struck since the last tick turns
        // on its attacker (or turns tail), before any beat is spent.
        let blows: Vec<openshard_combat::MobileDamaged> =
            self.state.bus.read(&mut self.damaged).copied().collect();
        if !blows.is_empty() {
            ai::retaliate(&mut self.state, &blows);
        }
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
            // A ridden mount is out of the world; its legs are the rider's.
            if self.state.registry.has::<Ridden>(creature) {
                continue;
            }
            if let Some(dir) = ai::think_one(&mut self.state, creature) {
                if let Some(serial) = self.state.registry.serial_of(creature) {
                    self.step(serial.raw(), dir);
                }
            }
            // A hunter re-beats at its own pace (or the shard's); idle life
            // ambles at half speed. Engagement is read after the think, so the
            // beat that acquired a target already quickens.
            let engaged = self
                .state
                .registry
                .get::<Combat>(creature)
                .and_then(|c| c.target)
                .is_some();
            let default_beat = self.state.gameplay.creature_step_ticks.max(1);
            if let Some(brain) = self.state.registry.get_mut::<Brain>(creature) {
                let base = if brain.beat_ticks > 0 {
                    brain.beat_ticks
                } else {
                    default_beat
                };
                brain.next_think = now + if engaged { base } else { base * 2 };
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
        // A rider logs out *still mounted*: the ride persists. The saddle rides
        // along in the saved inventory below, and `restore_inventory` rebuilds the
        // ridden creature from it on relogin, so the character comes back on
        // horseback where every other emulator would have dropped them on foot.
        // The transient creature itself is despawned once the inventory has
        // captured the saddle that stands for it (below).
        // Forget any targeting cursor it had up: a gone mobile clicks nothing.
        self.state.pending_targets.remove(&entity);
        let serial = self.state.registry.serial_of(entity);
        let facet = self.state.facet_of(entity);

        // Save before despawning, and not by marking it dirty: a `touch` is a
        // promise to read the entity at the next save, and in a moment there
        // will be no entity to read. Logging out is when a save matters most —
        // it is the only moment a player's whole session is at stake — so the
        // record is taken at the one instant it still can be.
        if let Some(record) = Self::record_of(&self.state.registry, entity, self.state.ticks) {
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

        // The ridden creature lived only in limbo; the saddle that rebuilds it is
        // now safely in the saved inventory, so drop the creature (the saddle item
        // itself goes with the character's belongings below).
        if let Some(&Riding { mount, .. }) = self.state.registry.get::<Riding>(entity) {
            self.state.registry.despawn(mount);
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
