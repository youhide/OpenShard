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
    encode_health, encode_remove, ClientVersion, Equipment, MobileIncoming, MobileMove, Notoriety,
    PlayerUpdate, Point, WorldItem,
};

use crate::components::{
    Amount, Body, Client, Contained, Equipped, Facet, Graphic, Heading, Hitpoints, Movement,
    Position,
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
        }
    }
}

impl Default for Gameplay {
    /// The pre-AoS feel the systems were built with — the values that were
    /// compile-time constants before an operator could tune them.
    fn default() -> Self {
        Self::new(1, 15000, 1000, 20 * 60, 2 * 60, 18, 3, 31)
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
        let Some(packet) = self.draw_packet(other, version) else {
            return;
        };
        self.seen.entry(watcher).or_default().insert(other);
        self.outbox.push(Outbound { connection, packet });
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
