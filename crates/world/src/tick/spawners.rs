use super::*;

impl World {
    /// Keep every spawn region at its ceiling. Once per tick, but cheap: a region
    /// not yet due to respawn is a single counter check, and only a due one that
    /// is short a creature does the work of counting and spawning. One creature
    /// per region per pass, so a wiped region refills at its own pace rather than
    /// snapping back full in a tick. Deterministic — the picks draw on the world's
    /// seeded rng, so a replay repopulates the same.
    pub(super) fn maintain_spawners(&mut self) {
        let now = self.state.ticks;
        // Nothing due this tick? Then skip the whole pass — no counting, no
        // proximity checks. The common case once a facet has settled.
        if !self.spawners.iter().any(|s| now >= s.next_spawn) {
            return;
        }
        // Count every region's live members in one sweep, keyed by owner id,
        // rather than re-scanning all creatures once per region. That turned the
        // cost from O(regions × creatures) — millions of comparisons a tick on a
        // full facet, the freeze a staff Populate caused — into O(regions +
        // creatures). The key is the same `index as u32` stored in `SpawnedBy`.
        let mut live_counts: HashMap<u32, u16> = HashMap::new();
        for (_, owner) in self.state.registry.query::<SpawnedBy>() {
            *live_counts.entry(owner.0).or_default() += 1;
        }
        let lod = self.state.gameplay.lod;
        let lod_radius = self.state.gameplay.lod_radius;
        for index in 0..self.spawners.len() {
            if now < self.spawners[index].next_spawn {
                continue;
            }
            let id = index as u32;
            let live = live_counts.get(&id).copied().unwrap_or(0);
            let spawner = &self.spawners[index];
            if spawner.creatures.is_empty() || live >= spawner.max_count {
                continue;
            }
            // Level of detail: a region no player is near need not be kept
            // populated — nobody sees it. It stays dormant (its timer held, not
            // advanced) until a player comes within range, then fills. The range
            // is the radius plus the region's own reach, so a player anywhere the
            // region could put a creature counts. Opt-in with the AI's `lod`.
            if lod {
                let area = spawner.area;
                let centre = Point::new(
                    area.x.wrapping_add(area.width / 2),
                    area.y.wrapping_add(area.height / 2),
                    0,
                );
                let reach = lod_radius + u32::from(area.width.max(area.height));
                if !self.state.any_player_near(centre, reach, area.facet) {
                    continue;
                }
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

            if let Some(entity) = npc::spawn(
                &mut self.state,
                npc::SpawnSpec {
                    body: creature.body,
                    hue: creature.hue,
                    hits: creature.hits,
                    notoriety: creature.notoriety,
                    damage: creature.damage,
                    resistance: creature.resistance,
                    swing: creature.swing,
                    sight: creature.sight,
                    aggression: creature.aggression,
                    beat: creature.beat,
                    ranged: creature.ranged,
                    ranged_kind: creature.ranged_kind,
                    wander: creature.wander,
                    position: Point::new(x, y, z),
                    facet,
                    // A maintained spawn is a monster or an animal, never a named
                    // townsperson; those are placed once, not respawned.
                    name: None,
                    banker: false,
                    vendor: false,
                    equipment: Vec::new(),
                    skills: creature.skills.clone(),
                },
            ) {
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
    pub(super) fn register_spawner(&mut self, mut spawner: crate::spawner::Spawner) {
        // A region already standing over this box wins, and keeps its timer. That
        // timer may have come from the database with hours still to wait, and the
        // boot re-populate (or a second staff click) must not reset it — a hard
        // reset is Clear, then Populate. This is also what lets the pack's
        // `populate` run on every boot, to re-place the townsfolk it does not save,
        // without stacking a second spawner or resetting the restored ones.
        if self.spawners.iter().any(|s| s.area == spawner.area) {
            return;
        }
        spawner.id = self.next_spawner_id;
        self.next_spawner_id += 1;
        // Stagger the first spawn across the respawn window. Populating a whole
        // facet registers hundreds of regions in one tick; without this they are
        // all due at once and fire together, a thundering herd that spikes the
        // tick the moment a staff member presses Populate. A jittered start
        // spreads that first fill over the respawn window instead. Only a fresh
        // register jitters — a restore from the save keeps its saved timer, set
        // by the caller after this returns.
        let delay = spawner.respawn_delay;
        if delay > 1 {
            let jitter = self.state.rng.below(delay.min(u64::from(u32::MAX)) as u32);
            spawner.next_spawn = self.state.ticks + u64::from(jitter);
        }
        self.spawners.push(spawner);
    }

    /// The spawn regions as saveable records. The live timer is a tick count; it is
    /// saved as the *seconds still to wait*, so it means the same after a restart
    /// resets the tick counter — a rare spawn killed with hours left comes back with
    /// those hours ahead of it, and downtime does not spend them.
    pub(super) fn spawner_records(&self) -> Vec<openshard_persistence::SpawnerRecord> {
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
                        aggression: c.aggression,
                        beat: c.beat,
                        ranged: c.ranged,
                        ranged_kind: c.ranged_kind,
                        wander: c.wander,
                        skills: c.skills.clone(),
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
                    aggression: c.aggression,
                    beat: c.beat,
                    ranged: c.ranged,
                    ranged_kind: c.ranged_kind,
                    wander: c.wander,
                    skills: c.skills,
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

    /// "Clear spawns" — the full reset the admin menu pairs with "Populate".
    ///
    /// Drops every spawn region and despawns every NPC mobile: a body, no client
    /// (players have one), and not a ridden mount (whose rider is a live player we
    /// must not strand on a phantom horse). This is both the spawner-maintained
    /// animals — tagged [`SpawnedBy`] — *and* the named townsfolk, bankers and
    /// vendors the pack places once via `op_spawn_mobile`, which carry no
    /// `SpawnedBy` and so used to survive a clear, reading as "clear did nothing".
    /// Each mobile takes its worn gear (and a vendor's stock crate and its wares)
    /// with it, and is taken off every screen before it goes.
    pub(super) fn clear_spawners(&mut self) {
        self.spawners.clear();
        let mobiles: Vec<EntityId> = self
            .state
            .registry
            .query::<Body>()
            .filter(|(entity, _)| {
                !self.state.registry.has::<Client>(*entity)
                    && !self.state.registry.has::<Ridden>(*entity)
            })
            .map(|(entity, _)| entity)
            .collect();
        for entity in mobiles {
            self.despawn_mobile(entity);
        }
    }

    /// Despawn one NPC mobile with everything it wears (and everything nested in
    /// what it wears), taking it off every watcher's screen first.
    fn despawn_mobile(&mut self, entity: EntityId) {
        if let Some(serial) = self.state.registry.serial_of(entity) {
            let worn: Vec<EntityId> = self
                .state
                .registry
                .query::<Equipped>()
                .filter(|(_, worn)| worn.mobile == serial)
                .map(|(item, _)| item)
                .collect();
            for item in worn {
                self.despawn_item_tree(item);
            }
            for watcher in self.state.watchers_of(entity) {
                self.state.forget(watcher, entity, serial);
            }
        }
        let facet = self.state.facet_of(entity);
        self.state.facet_state_mut(facet).sectors.remove(entity);
        self.state.registry.despawn(entity);
    }

    /// Despawn an item and, if it is a container, everything inside it, to any
    /// depth. Worn and contained items are drawn as part of their holder, never
    /// on their own, so no `0x1D` is owed — the holder's removal took them.
    fn despawn_item_tree(&mut self, item: EntityId) {
        if let Some(serial) = self.state.registry.serial_of(item) {
            let contents: Vec<EntityId> = self
                .state
                .registry
                .query::<Contained>()
                .filter(|(_, held)| held.container == serial)
                .map(|(child, _)| child)
                .collect();
            for child in contents {
                self.despawn_item_tree(child);
            }
        }
        self.state.registry.despawn(item);
    }
}
