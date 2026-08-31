//! Crossing a region boundary: who noticed, and what it changes.
//!
//! There is no "entered a region" packet and no hook on movement to hang this
//! from — a mobile moves through the player walk, the creature step, a teleport,
//! a resurrection and a login, and a call beside each of those is five places to
//! forget. So the crossing is *found*, once a tick, by comparing where each
//! mobile is against the region it was last seen in ([`InRegion`]). The same
//! shape as the status bar's diff and the ambient light's, and for the same
//! reason.
//!
//! What a crossing does here is deliberately thin: it emits
//! [`RegionChanged`](crate::events::RegionChanged) and starts the region's music.
//! The rules that *care* — guards hunting a murderer who walks into town, the
//! dark of a dungeon — read the region where they are decided, not here.

use openshard_entities::EntityId;
use openshard_protocol::serial::Serial;
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::world::{
    Facet,
    MusicId,
    PlayMusic,
};
use openshard_state::components::{
    Client,
    InRegion,
    Position,
};
use openshard_state::{
    Region,
    RegionId,
};
use tracing::{
    info,
    warn,
};

use super::World;
use crate::events::RegionChanged;

/// One mobile's crossing, gathered before anything is written — the world is
/// borrowed immutably to find these and mutably to act on them.
struct Crossing {
    entity: EntityId,
    serial: Serial,
    facet:  Facet,
    from:   Option<RegionId>,
    to:     Option<RegionId>,
    name:   String,
    music:  Option<u16>,
}

impl World {
    /// Give a facet its named areas, replacing whatever it had.
    ///
    /// Every player's remembered region is dropped with the old set: an id is an
    /// index into the list that was just thrown away, and keeping it would have
    /// people standing in regions that no longer exist. Dropping it makes the
    /// next tick's pass treat everyone as freshly arrived, which is exactly what
    /// they are.
    pub(super) fn register_regions(&mut self, facet: Facet, regions: Vec<openshard_state::Region>) {
        if !self.state.facets.contains_key(&facet) {
            warn!(facet = %facet, "regions for a facet this shard has not loaded");
            return;
        }
        for region in &regions {
            if let Some(light) = region.light {
                if light > 0x1F {
                    warn!(
                        facet = %facet,
                        region = %region.name,
                        light,
                        "region light above 0x1F; the client clamps it to pitch dark rather than complaining"
                    );
                }
            }
        }
        let count = regions.len();
        if let Err(error) = self.state.facet_state_mut(facet).regions.try_set(regions) {
            warn!(facet = %facet, %error, "regions were not registered");
            return;
        }
        self.forget_remembered_regions();
        info!(facet = %facet, count, "regions registered");
    }

    /// Forget a facet's regions.
    pub(super) fn clear_regions(&mut self, facet: Facet) {
        if self.state.facets.contains_key(&facet) {
            self.state.facet_state_mut(facet).regions.clear();
            self.forget_remembered_regions();
        }
    }

    /// The region at a point on a facet, if any — the outside world's way in,
    /// beside [`sectors`](World::sectors).
    #[must_use]
    pub fn region_at(&self, facet: Facet, point: openshard_protocol::world::Point) -> Option<&Region> {
        self.state.region_at(facet, point)
    }

    /// Every facet's regions as saveable records.
    ///
    /// Unlike a spawner there is no live timer to translate: a region is exactly
    /// its data, so this is a straight copy across the record seam.
    pub(super) fn region_records(&self) -> Vec<openshard_persistence::RegionRecord> {
        self.state
            .facets
            .iter()
            .flat_map(|(&facet, state)| {
                state.regions.iter().map(move |region| {
                    openshard_persistence::RegionRecord {
                        // `.0` at the record seam: a saved facet is a SQL column.
                        facet:       facet.0,
                        id:          region.id.0,
                        name:        region.name.clone(),
                        priority:    region.priority,
                        rects:       region
                            .rects
                            .iter()
                            .map(|r| (r.x, r.y, r.width, r.height, r.z_min, r.z_max))
                            .collect(),
                        guarded:     region.flags.guarded,
                        no_teleport: region.flags.no_teleport,
                        no_recall:   region.flags.no_recall,
                        no_housing:  region.flags.no_housing,
                        safe:        region.flags.safe,
                        music:       region.music,
                        light:       region.light,
                    }
                })
            })
            .collect()
    }

    /// Re-create the regions from saved records at boot. Call once, before anyone
    /// connects; a facet the shard no longer loads is dropped with a word, not a
    /// panic.
    pub fn restore_regions(&mut self, records: Vec<openshard_persistence::RegionRecord>) {
        // Keyed by the domain type from the first line in: `record.facet` is a
        // SQL column, and this is the seam where it becomes a facet again.
        let mut by_facet: std::collections::BTreeMap<Facet, Vec<openshard_state::Region>> =
            std::collections::BTreeMap::new();
        for record in records {
            by_facet
                .entry(Facet(record.facet))
                .or_default()
                .push(openshard_state::Region {
                    id:       RegionId(record.id),
                    name:     record.name,
                    priority: record.priority,
                    rects:    record
                        .rects
                        .into_iter()
                        .map(|(x, y, width, height, z_min, z_max)| {
                            openshard_state::RegionRect {
                                x,
                                y,
                                width,
                                height,
                                z_min,
                                z_max,
                            }
                        })
                        .collect(),
                    flags:    openshard_state::RegionFlags {
                        guarded:     record.guarded,
                        no_teleport: record.no_teleport,
                        no_recall:   record.no_recall,
                        no_housing:  record.no_housing,
                        safe:        record.safe,
                    },
                    music:    record.music,
                    light:    record.light,
                });
        }
        for (facet, mut regions) in by_facet {
            if !self.state.facets.contains_key(&facet) {
                warn!(
                    facet = %facet,
                    "saved regions for a facet this shard has not loaded"
                );
                continue;
            }
            // Saved in id order, and `set` renumbers by position — so sorting here
            // is what keeps an id meaning the same thing after a restart.
            regions.sort_by_key(|region| region.id);
            let count = regions.len();
            if let Err(error) = self.state.facet_state_mut(facet).regions.try_set(regions) {
                warn!(facet = %facet, %error, "saved regions were not restored");
                continue;
            }
            info!(facet = %facet, count, "regions restored");
        }
    }

    /// Drop everyone's remembered region, so the next crossing pass reads the
    /// world fresh.
    fn forget_remembered_regions(&mut self) {
        let known: Vec<EntityId> = self
            .state
            .registry
            .query::<InRegion>()
            .map(|(entity, _)| entity)
            .collect();
        for entity in known {
            self.state.registry.remove::<InRegion>(entity);
        }
    }

    /// Notice everyone who has crossed a region boundary since the last tick.
    ///
    /// Players only. A creature's region matters at the moment something asks —
    /// "is this thief in a guarded town" is a point query the guard rule makes
    /// against the criminal's own tile — and tracking it for ten thousand
    /// wandering monsters would buy nothing but a per-tick cost the LOD work
    /// exists to avoid.
    pub(super) fn region_crossings(&mut self) {
        let crossings = self.find_crossings();
        for crossing in crossings {
            self.state.registry.insert(
                crossing.entity,
                InRegion {
                    facet:  crossing.facet,
                    region: crossing.to,
                },
            );
            self.start_music(crossing.entity, crossing.music);
            self.state.bus.send(RegionChanged {
                entity: crossing.entity,
                serial: crossing.serial,
                facet:  crossing.facet,
                from:   crossing.from,
                to:     crossing.to,
                name:   crossing.name,
            });
        }
    }

    /// Everyone standing somewhere other than where their remembered region says.
    fn find_crossings(&self) -> Vec<Crossing> {
        self.state
            .players
            .values()
            .filter_map(|&entity| {
                let position = self.state.registry.get::<Position>(entity)?;
                let facet = self.state.facet_of(entity);
                let region = self.state.region_at(facet, position.0);
                let to = region.map(|region| region.id);
                let seen = self.state.registry.get::<InRegion>(entity);
                let from = seen.and_then(|seen| seen.region);
                // A mobile with no `InRegion` at all has just entered the world;
                // it is only a crossing if it landed somewhere named, or `enter`
                // would fire one for every login into open country.
                let known = seen.is_some();
                // The facet is half the comparison: the same id on two facets is
                // two different places, so a traveller has crossed even when the
                // number has not changed.
                if known && from == to && seen.is_some_and(|seen| seen.facet == facet) {
                    return None;
                }
                if !known && to.is_none() {
                    return None;
                }
                Some(Crossing {
                    entity,
                    serial: self.state.registry.serial_of(entity)?,
                    facet,
                    from,
                    to,
                    name: region.map(|r| r.name.clone()).unwrap_or_default(),
                    music: region.and_then(|r| r.music),
                })
            })
            .collect()
    }

    /// Play a region's track for one player, unless it is already the one
    /// playing.
    ///
    /// The check is not an optimisation. Re-sending `0x6D` with the same id
    /// *restarts* the track, so a player walking back and forth across a town
    /// line would hear the first two seconds of Britain over and over.
    fn start_music(&mut self, entity: EntityId, music: Option<u16>) {
        let Some(&Client { connection, .. }) = self.state.registry.get::<Client>(entity) else {
            return;
        };
        let Some(track) = music.map(MusicId) else {
            // Leaving a region does not stop the music: there is no "silence"
            // track, and the client fading out on its own is what UO does. The
            // remembered track stays, so walking back in is still a no-op.
            return;
        };
        let Some(row) = self.state.connection_mut(connection) else {
            return;
        };
        if row.last_music == Some(track) {
            return;
        }
        row.last_music = Some(track);
        self.state
            .send_packet(connection, &ServerPacket::PlayMusic(PlayMusic { track }));
    }

    /// Set the guards on anyone who has just walked into a town they are not
    /// welcome in — ServUO's `GuardedRegion.OnEnter`.
    ///
    /// Reads this tick's crossings off the bus rather than being called from
    /// inside the crossing pass: the crossing is a fact, and what anyone does
    /// about it is a rule of its own. The rule itself is `npc`'s; this is the
    /// cursor and the call, the shape `ai::retaliate` uses for `MobileDamaged`.
    pub(super) fn guard_crossings(&mut self) {
        let entered: Vec<EntityId> = self
            .state
            .bus
            .read(&mut self.crossed)
            .filter(|crossing| crossing.to.is_some())
            .map(|crossing| crossing.entity)
            .collect();
        for entity in entered {
            openshard_npc::hunt_with_guards(&mut self.state, entity);
        }
    }
}
