//! The world's runtime state: the data a tick reads and writes.
//!
//! [`WorldState`] gathers everything a gameplay system touches — the registry,
//! the event bus, the spatial index, the seeded generator, who is on each
//! client's screen — into one value that lives *below* the systems that act on
//! it. That is what lets a system be a function in its own crate
//! (`combat::swings(&mut WorldState)`) rather than a method on a single
//! ever-growing world object.
//!
//! What is deliberately *not* here: the tick itself, the persistence journal,
//! and the client's map files. Those sit above, in `openshard-world`, which owns
//! a `WorldState` and drives it. This crate knows the shape of world state and
//! nothing about when it changes or how it is saved.

use std::collections::{BTreeMap, HashMap, HashSet};

use openshard_entities::{EntityId, Registry, Serial};
use openshard_events::EventBus;
use openshard_gateway::ConnectionId;
use openshard_movement::Terrain;
use openshard_protocol::{
    encode_action, encode_health, encode_new_action, encode_opl_info, encode_play_sound,
    encode_remove, AccessLevel, ClientVersion, Equipment, Feature, MobileIncoming, MobileMove,
    Notoriety, PlayerUpdate, Point, PropertyList, WorldItem,
};

use crate::components::{
    body_opens_doors, Access, Amount, Body, Client, Contained, Equipped, Facet, Ghost, Graphic,
    Heading, Hitpoints, Movement, Name, Position,
};
use crate::obstruct::{LiveTerrain, Obstructions};
use crate::rng::Rng;
use crate::sectors::{Sectors, VIEW_RANGE};

/// A character's height above the ground when the facet has no map to ask.
const Z_WITHOUT_A_MAP: i8 = 0;

/// Ticks in one second — the reciprocal of the world's 50ms tick interval. The
/// world defines the interval; this is the whole-number rate config uses to turn
/// operator-facing seconds into the tick counts timers run on. If one moves, the
/// other must.
pub const TICKS_PER_SECOND: u64 = 20;

/// The gameplay rules an operator tuned, in the form the systems read them: the
/// [`GameplayConfig`](../../openshard_config) knobs, with the second-valued ones
/// already converted to ticks. A plain value the [`WorldState`] carries so any
/// system can reach the number it needs — combat the swing era, chat the speech
/// ranges, items the decay timer — without a config crate below them.
#[derive(Clone, Copy, Debug)]
pub struct Gameplay {
    /// Which swing-speed formula combat uses (Sphere's `CombatEra`, 0–4).
    pub combat_era: u8,
    /// The swing formula's numerator (Sphere's `SpeedScaleFactor`).
    pub speed_scale_factor: u64,
    /// The ceiling any one skill trains to, in tenths.
    pub skill_cap: u16,
    /// How long an item lies on the ground before it rots, in ticks.
    pub decay_ticks: u64,
    /// How long a criminal flag lasts, in ticks.
    pub criminal_ticks: u64,
    /// How far normal speech carries, in tiles.
    pub distance_talk: u32,
    /// How far a whisper carries, in tiles.
    pub distance_whisper: u32,
    /// How far a yell carries, in tiles.
    pub distance_yell: u32,
    /// Ticks between a hunting creature's steps. 8 (0.4s) is the references'
    /// base-monster pace — slower than a running player on purpose; 5 (0.25s)
    /// matches a runner, for shards that want monsters to catch people. Idle
    /// creatures amble at twice this.
    pub creature_step_ticks: u64,
    /// How a spell is cast — Sphere's cast-while-walking, or the UO/ServUO
    /// stop-to-cast with the target after.
    pub cast_style: CastStyle,
    /// Whether taking damage while casting disturbs the spell (UO's fizzle). Only
    /// meaningful in [`CastStyle::Stop`], where there is a cast to disturb.
    pub spell_disturb: bool,
    /// How AoS object tooltips are served — Sphere's `TOOLTIPMODE`, plus an off
    /// gate. Read by the interest substrate to decide what to send when a thing is
    /// drawn, and by the world when the client asks for a full list.
    pub tooltip_mode: TooltipMode,
    /// Whether the server answers a context-menu request with a popup.
    pub context_menus: bool,
    /// Whether spells require and consume reagents at all (classic UO on; a
    /// no-reagent shard off).
    pub reagents: bool,
    /// Whether a failed cast still spends mana — Sphere's `ManaLossFail`. Spent at
    /// resolution once success is known; a successful cast always spends.
    pub mana_loss_on_fail: bool,
    /// Whether a failed cast still consumes reagents — Sphere's `ReagentLossFail`.
    pub reagent_loss_on_fail: bool,
}

/// How AoS object tooltips (the "cliloc" hover names) are served — Sphere's
/// `TOOLTIPMODE`, with an added off state.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum TooltipMode {
    /// No tooltips, and AoS is not advertised — a modern client falls back to the
    /// classic single-click name label.
    Off,
    /// Send only a revision (`0xDC`) when a thing is drawn and wait for the client
    /// to request the full list (`0xD6`). Sphere's `TOOLTIPMODE_SENDVERSION`, the
    /// bandwidth-cheap standard.
    #[default]
    SendVersion,
    /// Send the whole tooltip (`0xD6`) up front. Sphere's `TOOLTIPMODE_SENDFULL`.
    SendFull,
}

impl TooltipMode {
    /// Parse the operator's `tooltips` string. `"off"` disables them, `"full"`
    /// sends the whole list up front; anything else is the send-version default.
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "false" => Self::Off,
            "full" | "sendfull" => Self::SendFull,
            _ => Self::SendVersion,
        }
    }
}

/// How a spell is cast — the choice both reference emulators make differently.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum CastStyle {
    /// The UO/ServUO original: the caster stops, says the words over a cast
    /// delay, and only then does the target cursor appear (after which it may
    /// move again). Damage during the delay can disturb it.
    #[default]
    Stop,
    /// Sphere's feel: the spell resolves as it is cast, with no rooting delay —
    /// the caster keeps walking, and a target cursor (if any) comes up at once.
    Walk,
}

impl CastStyle {
    /// Parse the operator's `cast_style` string. `"sphere"`/`"walk"` is the
    /// walking cast; anything else is the stop-to-cast default.
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "sphere" | "walk" | "walking" => Self::Walk,
            _ => Self::Stop,
        }
    }
}

impl Gameplay {
    /// Build the runtime rules from operator values, converting the two
    /// second-valued timers to ticks. The defaults an operator does not override
    /// are what [`Default`] gives — the numbers the systems used as constants.
    // One argument past clippy's limit, and every one is a distinct config knob;
    // a struct would only move the same list one call up.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        combat_era: u8,
        speed_scale_factor: u64,
        skill_cap: u16,
        decay_seconds: u64,
        criminal_seconds: u64,
        distance_talk: u32,
        distance_whisper: u32,
        distance_yell: u32,
        creature_step_ms: u64,
        cast_style: CastStyle,
        spell_disturb: bool,
        tooltip_mode: TooltipMode,
        context_menus: bool,
        reagents: bool,
        mana_loss_on_fail: bool,
        reagent_loss_on_fail: bool,
    ) -> Self {
        Self {
            combat_era,
            speed_scale_factor,
            skill_cap,
            decay_ticks: decay_seconds * TICKS_PER_SECOND,
            criminal_ticks: criminal_seconds * TICKS_PER_SECOND,
            distance_talk,
            distance_whisper,
            distance_yell,
            // 50ms per tick; anything under one tick is one tick.
            creature_step_ticks: (creature_step_ms / 50).max(1),
            cast_style,
            spell_disturb,
            tooltip_mode,
            context_menus,
            reagents,
            mana_loss_on_fail,
            reagent_loss_on_fail,
        }
    }
}

impl Default for Gameplay {
    /// The pre-AoS feel the systems were built with — the values that were
    /// compile-time constants before an operator could tune them.
    fn default() -> Self {
        Self::new(
            1,
            15000,
            1000,
            20 * 60,
            2 * 60,
            18,
            3,
            31,
            400,
            CastStyle::Stop,
            true,
            TooltipMode::SendVersion,
            true,
            true, // reagents
            true, // mana_loss_on_fail
            true, // reagent_loss_on_fail
        )
    }
}

/// Bytes for a connection, produced by a tick.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Outbound {
    /// Who to send to.
    pub connection: ConnectionId,
    /// What to send.
    pub packet: Vec<u8>,
}

/// One facet: its ground, and who is near what on it.
///
/// The world keeps one of these per loaded facet. Two mobiles on different
/// facets never share a sector grid, so they never see each other and never
/// block each other — the isolation is a property of the data structure, not a
/// check anyone has to remember to write.
///
/// The ground is a [`Terrain`] trait object, not a concrete map: this crate sits
/// below the client-file parsers, so it holds the *abstraction* of terrain and
/// the world hands it the real thing (a `MapTerrain`) boxed. A facet with no map
/// carries `None` and every step is allowed.
pub struct FacetState {
    /// The floor, if this facet has a map loaded.
    pub terrain: Option<Box<dyn Terrain + Send + Sync>>,
    /// Who is near what, on this facet.
    pub sectors: Sectors,
    /// What the live world has put in the way: closed doors, placed decoration.
    pub obstructions: Obstructions,
}

impl FacetState {
    /// The terrain every movement decision actually checks: the map with the
    /// live obstacles laid over it. Works with no map too — an open world with
    /// doors in it still has doors.
    #[must_use]
    pub fn live_terrain(&self) -> LiveTerrain<'_> {
        LiveTerrain::new(self.terrain.as_deref(), &self.obstructions, false)
    }

    /// The same terrain as a door-opener plans over: closed doors do not block,
    /// because the mobile walking the route opens them on arrival.
    #[must_use]
    pub fn planning_terrain(&self, through_doors: bool) -> LiveTerrain<'_> {
        LiveTerrain::new(self.terrain.as_deref(), &self.obstructions, through_doors)
    }
}

/// An item on a cursor: the entity, and where it was lifted from.
///
/// The origin is the whole reason to remember more than the entity. A drag that
/// is refused — dropped out of reach, into nothing — has to put the item back
/// exactly where it was, and by then it is off the ground (and out of any
/// container) with no place of its own to return to.
#[derive(Clone, Copy, Debug)]
pub struct HeldItem {
    /// The lifted item.
    pub entity: EntityId,
    /// Where it was, so a cancelled drag can undo cleanly.
    pub origin: Origin,
}

impl std::fmt::Debug for FacetState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FacetState")
            .field("has_terrain", &self.terrain.is_some())
            .field("sectors", &self.sectors.len())
            .finish()
    }
}

/// Where a held item came from, so a cancelled drag can put it back.
#[derive(Clone, Copy, Debug)]
pub enum Origin {
    /// It was on the ground.
    Ground {
        /// Where it lay.
        position: Point,
        /// On which facet.
        facet: u8,
    },
    /// It was inside a container.
    Container(Contained),
    /// It was worn by a mobile.
    Worn(Equipped),
}

/// The world's runtime state — the data every gameplay system operates on.
///
/// A plain value with public fields: it is a data carrier, not an encapsulation
/// boundary. The boundary that matters is the event bus (systems emit, they do
/// not call), not field privacy. Nothing here is a static; a test builds as many
/// as it likes.
pub struct WorldState {
    /// Everything in the world.
    pub registry: Registry,
    /// What happened, for anyone to read: the client, persistence, scripts.
    pub bus: EventBus,
    /// The loaded facets, each with its own ground and interest grid, keyed by
    /// facet number. There is always at least the default one.
    pub facets: BTreeMap<u8, FacetState>,
    /// The facet a new character spawns on, and the one anything asking for a
    /// facet it does not have falls back to.
    pub default_facet: u8,
    /// Which entity a connection is driving.
    pub players: HashMap<ConnectionId, EntityId>,
    /// What each player's client currently has on screen.
    ///
    /// The server has to remember, because the client never says. There is no
    /// "what can you see" packet — only "draw this" and "forget that" — so the
    /// only way to send a mobile exactly once is to know what was sent before.
    pub seen: HashMap<EntityId, HashSet<EntityId>>,
    /// The item each connection is dragging on its cursor, and where it was so a
    /// cancelled drag can put it back. An item here is off the ground and out of
    /// everyone's [`seen`](Self::seen) — in limbo until a `0x08` lands it.
    pub held: HashMap<ConnectionId, HeldItem>,
    /// Where new characters appear. The height comes from the map.
    pub start: (u16, u16),
    /// The generator behind every roll — a swing landing, a skill gaining. Part
    /// of the state so replay is exact; advanced only inside the tick.
    pub rng: Rng,
    /// How many ticks have run.
    pub ticks: u64,
    /// Packets the last tick produced.
    pub outbox: Vec<Outbound>,
    /// Which connections have each container open, so a change to its contents —
    /// an item consumed as a reagent, one decaying inside — can be pushed to the
    /// clients looking at it. A connection's opens are cleared on logout.
    pub open_containers: HashMap<Serial, HashSet<ConnectionId>>,
    /// Mobiles that have a targeting cursor up, and what the click is for. A `.tele`
    /// raises one; the `0x6C` answer looks here to know what to do with the spot.
    pub pending_targets: HashMap<EntityId, TargetPurpose>,
    /// The tunable rules — swing era, speech ranges, timers — the systems read.
    pub gameplay: Gameplay,
    /// Set by a staff `.save` to ask the tick for an immediate snapshot. The world
    /// clears it once taken — a request, not the save itself, because taking the
    /// snapshot is the `World`'s to do, not a system's.
    pub save_requested: bool,
}

/// What a raised targeting cursor is waiting to do with the click.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TargetPurpose {
    /// Teleport the targeter to the clicked spot — the cursor `.tele`.
    Teleport,
    /// A targeted spell waiting for its aim — the cursor a spell puts up once
    /// the cast resolves. `success` is the skill roll already made, carried here
    /// so a fumbled cast that still raises a cursor simply lands no effect.
    Spell {
        /// Which spell, by id.
        spell: u16,
        /// Whether the cast's skill roll passed.
        success: bool,
    },
}

impl WorldState {
    /// Which facet an entity is on: its [`Facet`] component, or the world default
    /// so callers can index [`facets`](Self::facets) with the result.
    #[must_use]
    pub fn facet_of(&self, entity: EntityId) -> u8 {
        self.registry
            .get::<Facet>(entity)
            .map_or(self.default_facet, |facet| facet.0)
    }

    /// The state of a facet the world is known to have.
    #[must_use]
    pub fn facet_state(&self, facet: u8) -> &FacetState {
        &self.facets[&facet]
    }

    /// The same, mutably. Panics only on a facet no entity should carry —
    /// `facet_of` and `enter` keep every live entity on a loaded facet.
    pub fn facet_state_mut(&mut self, facet: u8) -> &mut FacetState {
        self.facets
            .get_mut(&facet)
            .expect("an entity's facet is always loaded")
    }

    /// Where a character appears on `facet`: the configured x and y, at that
    /// facet's height.
    ///
    /// The `z` is read from the map rather than configured. A second source of
    /// truth that disagrees by three units leaves a character unable to take a
    /// single step — every one is more than a two-unit climb — with nothing in
    /// the log to explain it.
    #[must_use]
    pub fn start_position(&self, facet: u8) -> Point {
        let (x, y) = self.start;
        let z = self
            .facets
            .get(&facet)
            .and_then(|state| state.terrain.as_ref())
            .and_then(|terrain| terrain.ground_z(x, y))
            .unwrap_or(Z_WITHOUT_A_MAP);
        Point::new(x, y, z)
    }

    /// Everyone who currently has `entity` on their screen — the mobiles whose
    /// `seen` set holds it. The audience for a redraw: a health bar, a step, a
    /// change of colour.
    #[must_use]
    pub fn watchers_of(&self, entity: EntityId) -> Vec<EntityId> {
        self.seen
            .iter()
            .filter(|(watcher, seen)| **watcher != entity && seen.contains(&entity))
            .map(|(watcher, _)| *watcher)
            .collect()
    }

    /// Redraw `entity`'s health bar: the real numbers to itself, a 0–100 scale to
    /// everyone watching. The `0xA1` a blow or a heal sends.
    pub fn broadcast_health(&mut self, entity: EntityId) {
        let Some(&Hitpoints { current, max }) = self.registry.get::<Hitpoints>(entity) else {
            return;
        };
        let Some(serial) = self.registry.serial_of(entity) else {
            return;
        };
        if let Some(&Client { connection, .. }) = self.registry.get::<Client>(entity) {
            self.outbox.push(Outbound {
                connection,
                packet: encode_health(serial.raw(), max, current, true),
            });
        }
        let scaled = encode_health(serial.raw(), max, current, false);
        for watcher in self.watchers_of(entity) {
            if let Some(&Client { connection, .. }) = self.registry.get::<Client>(watcher) {
                self.outbox.push(Outbound {
                    connection,
                    packet: scaled.clone(),
                });
            }
        }
    }

    /// Send one prebuilt, version-independent packet to every player within
    /// view range of `source` — its own client included.
    ///
    /// The audience for a sound or a graphical effect is who is *near*, not the
    /// `seen` set a health redraw uses: a door never enters anyone's `seen` (it is
    /// decoration, redrawn by `reveal`, not tracked as an interest), yet its creak
    /// must still be heard — so this asks the spatial index for neighbours the way
    /// `reveal` does, and keeps the ones with a client. There is no self-vs-others
    /// split: a sound and an effect are the same bytes for everyone, so a caller
    /// builds the packet once and this fans it out. The feedback seam every
    /// gameplay system reaches for — a swing, a spell, a door — so the world is
    /// *felt*, not merely correct.
    pub fn broadcast_from(&mut self, source: EntityId, packet: Vec<u8>) {
        let facet = self.facet_of(source);
        let sectors = &self.facet_state(facet).sectors;
        let Some(centre) = sectors.position_of(source) else {
            return;
        };
        // Collected before the mutation so the sectors borrow is dropped.
        let audience: Vec<EntityId> = sectors
            .nearby(centre, VIEW_RANGE)
            .map(|(id, _)| id)
            .collect();
        for entity in audience {
            if let Some(&Client { connection, .. }) = self.registry.get::<Client>(entity) {
                self.outbox.push(Outbound {
                    connection,
                    packet: packet.clone(),
                });
            }
        }
    }

    /// Play `sound` at `source`'s position, heard by everyone who can see it.
    ///
    /// A no-op for a source with no `Position` (a contained item) — its holder's
    /// tile is where such a sound belongs, and that is the caller's to place. The
    /// `0x54` is placed in 3D so the client attenuates it by distance.
    pub fn play_sound(&mut self, source: EntityId, sound: u16) {
        let Some(&Position(at)) = self.registry.get::<Position>(source) else {
            return;
        };
        let packet = encode_play_sound(sound, at.x, at.y, at.z);
        self.broadcast_from(source, packet);
    }

    /// Animate `mobile` performing `action` — a swing, a death throe, a cast
    /// gesture — for everyone who can see it.
    ///
    /// The wire is per-client, not per-packet: a modern client (7.0.0.0+) gets the
    /// `0xE2` new-animation packet, where the server names a body-agnostic
    /// [`AnimationType`](Action) and the client picks the frames for that body —
    /// which is why a swing needs no body table there. An older client gets the
    /// `0x6E` classic packet, whose action id *is* body-specific, so it is chosen
    /// off a coarse humanoid-vs-creature split (the same `body_opens_doors` line
    /// the door AI uses). The split is deliberately rough: exact per-weapon,
    /// per-body actions want the animation tables the references key off body id,
    /// and the modern path — the one the test clients take — does not need them.
    pub fn animate(&mut self, mobile: EntityId, action: Action) {
        let Some(serial) = self.registry.serial_of(mobile) else {
            return;
        };
        let humanoid = self
            .registry
            .get::<Body>(mobile)
            .is_some_and(|body| body_opens_doors(body.id));
        // Built once each; the per-recipient choice is only which to send.
        let new_packet = encode_new_action(serial.raw(), action.animation_type(), 0, 0);
        let (old_action, frames) = action.classic_action(humanoid);
        let old_packet = encode_action(serial.raw(), old_action, frames, 1, true, false, 0);

        let facet = self.facet_of(mobile);
        let sectors = &self.facet_state(facet).sectors;
        let Some(centre) = sectors.position_of(mobile) else {
            return;
        };
        let audience: Vec<EntityId> = sectors
            .nearby(centre, VIEW_RANGE)
            .map(|(id, _)| id)
            .collect();
        for entity in audience {
            if let Some(&Client {
                connection,
                version,
            }) = self.registry.get::<Client>(entity)
            {
                let packet = if version.supports(Feature::NewMobileAnimation) {
                    new_packet.clone()
                } else {
                    old_packet.clone()
                };
                self.outbox.push(Outbound { connection, packet });
            }
        }
    }
}

/// A mobile action worth animating — the semantic the caller names, which
/// [`WorldState::animate`] turns into the wire animation each client understands.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    /// A melee or ranged swing.
    Attack,
    /// A death throe.
    Die,
    /// A spellcasting gesture.
    Cast,
}

impl Action {
    /// The `0xE2` [`AnimationType`](Action) — ServUO's enum: Attack 0, Die 3,
    /// Spell 11. The client maps it to the right frames for whatever body it is,
    /// so no body table is needed on this path.
    const fn animation_type(self) -> u16 {
        match self {
            Self::Attack => 0, // Attack
            Self::Die => 3,    // Die
            Self::Cast => 11,  // Spell
        }
    }

    /// The `0x6E` classic action id and frame count, which *are* body-specific.
    /// The humanoid ids are ServUO's people-animation values (Wrestle 31, human
    /// die 21, human directed-cast 16); the creature ids its monster-group values
    /// (attack 4, die 2, cast 12). A coarse split until weapon and body tables
    /// land — good enough for the old 2D client, which is the minority path.
    const fn classic_action(self, humanoid: bool) -> (u16, u16) {
        match (self, humanoid) {
            (Self::Attack, true) => (31, 7), // WeaponAnimation.Wrestle
            (Self::Attack, false) => (4, 4), // monster attack1
            (Self::Die, true) => (21, 6),    // human die
            (Self::Die, false) => (2, 4),    // monster die
            (Self::Cast, true) => (16, 7),   // human directed-cast
            (Self::Cast, false) => (12, 7),  // monster cast
        }
    }
}

/// Interest management: the machinery that keeps each client's screen in sync
/// with the world — who to draw, who to forget, who to redraw on a move. Shared
/// by every system that changes what a mobile looks like or where it stands.
impl WorldState {
    /// Move a mobile to `to` at once — a teleport, not a walk. Sets its position
    /// everywhere the world tracks it, tells its own client to jump there, and
    /// refreshes what it and everyone around it can see.
    ///
    /// The own-client `0x20` is the part a plain position write forgets: without
    /// it the client keeps drawing its character at the old tile while the new
    /// neighbours appear around where it used to stand — the "teleport did not
    /// refresh" bug. A walk does not need this because the client predicts its own
    /// step; a decree does, because the client was not expecting to move.
    pub fn teleport(&mut self, entity: EntityId, to: Point) {
        let facet = self.facet_of(entity);
        self.registry.insert(entity, Position(to));
        // Keep the walker's own copy in step, or the next walk starts from the old
        // tile.
        if let Some(Movement(mut walker)) = self.registry.get::<Movement>(entity).copied() {
            walker.position = to;
            self.registry.insert(entity, Movement(walker));
        }
        self.facet_state_mut(facet).sectors.insert(entity, to);

        if let Some(&Client { connection, .. }) = self.registry.get::<Client>(entity) {
            let serial = self.registry.serial_of(entity).map_or(0, |s| s.raw());
            let body = self.registry.get::<Body>(entity);
            let facing = self.registry.get::<Heading>(entity).map(|h| h.0);
            if let (Some(body), Some(facing)) = (body, facing) {
                self.send(
                    connection,
                    PlayerUpdate {
                        serial,
                        body: body.id,
                        hue: body.hue,
                        flags: 0,
                        position: to,
                        facing,
                    }
                    .encode(),
                );
            }
        }
        self.refresh_around(entity);
    }

    /// Bring `entity`'s neighbourhood up to date, both ways.
    ///
    /// Whoever it can see, and whoever can see it. Both, because visibility is
    /// symmetric here and doing one direction leaves the other end with a mobile
    /// that walked away and never left the screen.
    pub fn refresh_around(&mut self, entity: EntityId) {
        // Only this entity's facet: two mobiles on different facets share no
        // sector grid, so a lookup here never turns up anyone on another one.
        let facet = self.facet_of(entity);
        let sectors = &self.facet_state(facet).sectors;
        let Some(centre) = sectors.position_of(entity) else {
            return;
        };

        // Collect first. The lookup borrows the index and the sends borrow `self`
        // mutably, and more importantly a `Vec` here is what makes the set of
        // neighbours a snapshot rather than something that shifts while it is
        // walked.
        let neighbours: Vec<EntityId> = sectors
            .nearby(centre, VIEW_RANGE)
            .map(|(id, _)| id)
            .filter(|id| *id != entity)
            .collect();

        for other in &neighbours {
            self.show(entity, *other);
            self.show(*other, entity);
        }

        // Anything this one used to see and no longer can. `nearby` says who is
        // close; only the remembered set says who *was*.
        let gone: Vec<EntityId> = self
            .seen
            .get(&entity)
            .map(|seen| {
                seen.iter()
                    .filter(|id| !neighbours.contains(id))
                    .copied()
                    .collect()
            })
            .unwrap_or_default();
        for other in gone {
            if let Some(serial) = self.registry.serial_of(other) {
                self.forget(entity, other, serial);
            }
        }

        // And anyone who used to see this one and no longer can.
        for watcher in self.watchers_of(entity) {
            if !neighbours.contains(&watcher) {
                if let Some(serial) = self.registry.serial_of(entity) {
                    self.forget(watcher, entity, serial);
                }
            }
        }

        self.broadcast_move(entity);
    }

    /// Tell everyone already watching `entity` that it moved.
    ///
    /// Only those who already have it: someone seeing it for the first time gets
    /// a `0x78` from [`show`](Self::show), and a `0x77` for a mobile the client
    /// has never heard of is ignored.
    pub fn broadcast_move(&mut self, entity: EntityId) {
        let Some(packet) = self.mobile_move(entity) else {
            return;
        };
        for watcher in self.watchers_of(entity) {
            let Some(&Client {
                connection,
                version,
            }) = self.registry.get::<Client>(watcher)
            else {
                continue;
            };
            self.outbox.push(Outbound {
                connection,
                packet: packet.encode(version),
            });
        }
    }

    /// Draw `other` for `watcher`, if it is not already on screen.
    pub fn show(&mut self, watcher: EntityId, other: EntityId) {
        // Only players have screens. An NPC "seeing" someone is an AI question,
        // and it does not belong in the packet path.
        let Some(&Client {
            connection,
            version,
        }) = self.registry.get::<Client>(watcher)
        else {
            return;
        };
        if self
            .seen
            .get(&watcher)
            .is_some_and(|seen| seen.contains(&other))
        {
            return;
        }
        // The living cannot see the dead: a ghost is drawn only to another ghost
        // or to staff. Skip it here, before it ever enters `seen`, so a living
        // watcher never has it on screen to move or forget.
        if !self.can_see_mobile(watcher, other) {
            return;
        }
        let Some(packet) = self.draw_packet(other, version) else {
            return;
        };
        self.seen.entry(watcher).or_default().insert(other);
        self.outbox.push(Outbound { connection, packet });
        // The health bar rides along with the draw. There is no "what is its
        // health" packet the client can count on us answering — it opens the bar
        // from what it was last told — so a mobile whose bar is never sent shows an
        // empty frame until the first blow moves it. Send the scaled bar on sight
        // and it reads full from the moment you see it, like every other client.
        if let Some(&Hitpoints { current, max }) = self.registry.get::<Hitpoints>(other) {
            if let Some(serial) = self.registry.serial_of(other) {
                self.outbox.push(Outbound {
                    connection,
                    packet: encode_health(serial.raw(), max, current, false),
                });
            }
        }
        // AoS tooltip: the drawn thing's property revision rides along, so the
        // client knows its cached tooltip is stale and can ask for a fresh one.
        if let Some(tooltip) = self.tooltip_packet(other, version) {
            self.outbox.push(Outbound {
                connection,
                packet: tooltip,
            });
        }
    }

    /// The tooltip packet to send *alongside* a draw, or `None` when tooltips are
    /// off, the client is too old for them, or the object has no properties.
    ///
    /// In send-version mode a client new enough for revision hashes ([`0xDC`],
    /// [`Feature::TooltipHash`]) gets just the revision and asks for the list on
    /// hover; an older AoS client, or send-full mode, gets the whole list up front
    /// — it cannot request one it was never told a revision for. Sphere's
    /// `TOOLTIPMODE`.
    fn tooltip_packet(&self, entity: EntityId, version: ClientVersion) -> Option<Vec<u8>> {
        if self.gameplay.tooltip_mode == TooltipMode::Off || !version.supports(Feature::Tooltips) {
            return None;
        }
        let (full, hash) = self.object_properties(entity)?;
        let send_version = self.gameplay.tooltip_mode == TooltipMode::SendVersion
            && version.supports(Feature::TooltipHash);
        if send_version {
            let serial = self.registry.serial_of(entity)?.raw();
            Some(encode_opl_info(serial, hash))
        } else {
            Some(full)
        }
    }

    /// The `0xD6` property list for an object and its revision hash, or `None` for
    /// something with no name to show. Name-only for now: a mobile is cliloc
    /// `1050045` (`~1_PREFIX~~2_NAME~~3_SUFFIX~`) with its [`Name`]; an item is
    /// cliloc `1020000 + graphic` — the client's own tiledata-name range, so no
    /// string is sent — pluralised through cliloc `1050039` when it is a stack.
    /// The item-vs-mobile split is [`draw_packet`](Self::draw_packet)'s, read for
    /// a tooltip rather than a draw. Ported from ServUO's `AddNameProperties` /
    /// `Item.AddNameProperty`.
    #[must_use]
    pub fn object_properties(&self, entity: EntityId) -> Option<(Vec<u8>, u32)> {
        let serial = self.registry.serial_of(entity)?.raw();
        let mut list = PropertyList::new(serial);
        if let Some(Name(name)) = self.registry.get::<Name>(entity) {
            list.add_args(1_050_045, &format!(" \t{name}\t "));
        } else if let Some(&Graphic { id, .. }) = self.registry.get::<Graphic>(entity) {
            let cliloc = 1_020_000 + u32::from(id);
            match self.registry.get::<Amount>(entity) {
                Some(Amount(amount)) if *amount > 1 => {
                    list.add_args(1_050_039, &format!("{amount}\t#{cliloc}"));
                }
                _ => list.add(cliloc),
            }
        } else {
            return None;
        }
        Some(list.finish())
    }

    /// Send `entity`'s full `0xD6` property list to one connection — the answer to
    /// a client's tooltip request. Nothing is sent for an object with no name.
    pub fn send_property_list(&mut self, connection: ConnectionId, entity: EntityId) {
        if let Some((packet, _)) = self.object_properties(entity) {
            self.outbox.push(Outbound { connection, packet });
        }
    }

    /// The packet that draws `entity` on a client, or `None` for something not
    /// drawable. A mobile is a `0x78`, an item a `0x1A` — the interest system does
    /// not care which, only that there is one packet per thing on screen.
    #[must_use]
    pub fn draw_packet(&self, entity: EntityId, version: ClientVersion) -> Option<Vec<u8>> {
        if self.registry.has::<Body>(entity) {
            Some(self.mobile_incoming(entity)?.encode(version))
        } else if self.registry.has::<Graphic>(entity) {
            Some(self.world_item(entity)?.encode())
        } else {
            None
        }
    }

    /// Build a `0x1A` for an entity, if it is a drawable item.
    #[must_use]
    pub fn world_item(&self, entity: EntityId) -> Option<WorldItem> {
        let serial = self.registry.serial_of(entity)?;
        let Graphic { id, hue } = *self.registry.get::<Graphic>(entity)?;
        let Position(position) = *self.registry.get::<Position>(entity)?;
        // No `Amount` means a single. The encoder treats 1 and absent the same.
        let amount = self.registry.get::<Amount>(entity).map_or(1, |a| a.0);
        Some(WorldItem {
            serial: serial.raw(),
            graphic: id,
            amount,
            position,
            hue,
        })
    }

    /// Take `other` off `watcher`'s screen.
    pub fn forget(&mut self, watcher: EntityId, other: EntityId, serial: Serial) {
        if let Some(seen) = self.seen.get_mut(&watcher) {
            if !seen.remove(&other) {
                return;
            }
        } else {
            return;
        }
        if let Some(&Client { connection, .. }) = self.registry.get::<Client>(watcher) {
            self.outbox.push(Outbound {
                connection,
                packet: encode_remove(serial.raw()),
            });
        }
    }

    /// Whether `watcher` has staff authority — a GameMaster or above. Staff see
    /// the dead and are held to none of the interest rules the living obey.
    #[must_use]
    pub fn is_staff(&self, watcher: EntityId) -> bool {
        self.registry
            .get::<Access>(watcher)
            .is_some_and(|access| access.0 >= AccessLevel::GameMaster)
    }

    /// Whether `watcher` may see mobile `other`. The living cannot see the dead: a
    /// ghost is drawn only to itself, to another ghost, or to staff — ServUO's
    /// `CanSee(Mobile)` (`this == m || m.Alive || !Alive || IsStaff`). Every other
    /// mobile in range is visible to everyone; an item is never a ghost, so this
    /// bites only mobiles.
    #[must_use]
    fn can_see_mobile(&self, watcher: EntityId, other: EntityId) -> bool {
        if !self.registry.has::<Ghost>(other) {
            return true;
        }
        watcher == other || self.registry.has::<Ghost>(watcher) || self.is_staff(watcher)
    }

    /// A mobile's standing — the colour of its health bar. Absent reads as
    /// [`Notoriety::Innocent`], a blue bar, the safe default.
    #[must_use]
    pub fn notoriety_of(&self, entity: EntityId) -> Notoriety {
        self.registry
            .get::<Notoriety>(entity)
            .copied()
            .unwrap_or(Notoriety::Innocent)
    }

    /// Build a `0x78` for an entity, if it is a drawable mobile.
    #[must_use]
    pub fn mobile_incoming(&self, entity: EntityId) -> Option<MobileIncoming> {
        let serial = self.registry.serial_of(entity)?;
        let Position(position) = *self.registry.get::<Position>(entity)?;
        let Heading(facing) = *self.registry.get::<Heading>(entity)?;
        let body = *self.registry.get::<Body>(entity)?;
        Some(MobileIncoming {
            serial: serial.raw(),
            body: body.id,
            position,
            facing,
            hue: body.hue,
            flags: 0,
            notoriety: self.notoriety_of(entity),
            equipment: self.equipment_of(serial),
        })
    }

    /// What a mobile is wearing, as the `0x78` equipment list.
    #[must_use]
    pub fn equipment_of(&self, mobile: Serial) -> Vec<Equipment> {
        self.registry
            .query::<Equipped>()
            .filter(|(_, worn)| worn.mobile == mobile)
            .filter_map(|(item, worn)| {
                let serial = self.registry.serial_of(item)?;
                let Graphic { id, hue } = *self.registry.get::<Graphic>(item)?;
                Some(Equipment {
                    serial: serial.raw(),
                    graphic: id,
                    layer: worn.layer,
                    hue,
                })
            })
            .collect()
    }

    /// Build a `0x77` for an entity.
    #[must_use]
    pub fn mobile_move(&self, entity: EntityId) -> Option<MobileMove> {
        let serial = self.registry.serial_of(entity)?;
        let Position(position) = *self.registry.get::<Position>(entity)?;
        let Heading(facing) = *self.registry.get::<Heading>(entity)?;
        let body = *self.registry.get::<Body>(entity)?;
        Some(MobileMove {
            serial: serial.raw(),
            body: body.id,
            position,
            facing,
            hue: body.hue,
            flags: 0,
            notoriety: self.notoriety_of(entity),
        })
    }

    /// Queue a raw packet for a connection.
    pub fn send(&mut self, connection: ConnectionId, packet: Vec<u8>) {
        self.outbox.push(Outbound { connection, packet });
    }

    /// Draw a newly placed or changed `entity` to everyone in range who does not
    /// already have it — a fresh item, a spawned creature, an equipped mobile.
    pub fn reveal(&mut self, entity: EntityId) {
        let facet = self.facet_of(entity);
        let sectors = &self.facet_state(facet).sectors;
        let Some(centre) = sectors.position_of(entity) else {
            return;
        };
        let watchers: Vec<EntityId> = sectors
            .nearby(centre, VIEW_RANGE)
            .map(|(id, _)| id)
            .filter(|id| *id != entity)
            .collect();
        for watcher in watchers {
            self.show(watcher, entity);
        }
    }
}

impl std::fmt::Debug for WorldState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorldState")
            .field("ticks", &self.ticks)
            .field("entities", &self.registry.len())
            .field("players", &self.players.len())
            .field("facets", &self.facets.len())
            .finish()
    }
}
