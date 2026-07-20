use super::*;
use openshard_state::components::{creature_name, Decays, CORPSE_GRAPHIC, CORPSE_GUMP};

/// Gold, the core's default corpse loot until the pack owns real loot tables.
const GOLD_GRAPHIC: u16 = 0x0EED;
/// How long a corpse lies before it rots away with its loot — ServUO's default
/// seven minutes, in ticks.
const CORPSE_DECAY_TICKS: u64 = 7 * 60 * TICKS_PER_SECOND;

impl World {
    /// Lay a corpse where each creature that died this tick fell, and take the
    /// creature off the world.
    ///
    /// Reads the tick's [`MobileDied`](openshard_combat::MobileDied) events — the
    /// "emit, don't call" seam: combat announces a death, the world disposes of the
    /// body. Only a bodied non-player becomes a corpse here; a player's death is
    /// left standing at zero hits for now, since the ghost slice is later.
    pub(super) fn reap(&mut self) {
        let dead: Vec<(EntityId, Serial)> = self
            .state
            .bus
            .read(&mut self.dead)
            .map(|event| (event.entity, event.serial))
            .collect();
        for (entity, serial) in dead {
            // Reap only creatures (ghosts are later); and a body already gone —
            // reaped once, or removed another way this tick — is skipped.
            if self.state.registry.has::<Client>(entity)
                || self.state.registry.entity_of(serial).is_none()
            {
                continue;
            }
            self.lay_corpse(entity, serial);
        }
    }

    /// Turn one dead creature into a corpse holding its gear and a little gold,
    /// then despawn the creature.
    fn lay_corpse(&mut self, entity: EntityId, serial: Serial) {
        let Some(&Position(at)) = self.state.registry.get::<Position>(entity) else {
            // No position (a mount in limbo, say) — nothing to lay a corpse on.
            self.despawn_creature(entity, serial);
            return;
        };
        let facet = self.state.facet_of(entity);
        let body = self.state.registry.get::<Body>(entity).copied();
        let max_hits = self
            .state
            .registry
            .get::<Hitpoints>(entity)
            .map_or(0, |h| h.max);
        let name = body
            .and_then(|b| creature_name(b.id))
            .map_or_else(|| "a corpse".to_owned(), |n| format!("a corpse of {n}"));

        let Some(corpse) = self.spawn_corpse(at, facet, body, name) else {
            self.despawn_creature(entity, serial);
            return;
        };
        // Its worn gear falls into the corpse, and a little gold — the core's
        // default until the pack owns loot tables off a corpse event.
        self.move_gear_to_corpse(serial, corpse);
        let gold = self.corpse_gold(max_hits);
        if gold > 0 {
            let _ = items::give(&mut self.state, corpse, GOLD_GRAPHIC, 0, gold);
        }
        self.despawn_creature(entity, serial);
    }

    /// Spawn a corpse item at `at`, drawn as `body` and named `name`, and return
    /// its serial. A container (the loot window) that rots after a while.
    fn spawn_corpse(
        &mut self,
        at: Point,
        facet: u8,
        body: Option<Body>,
        name: String,
    ) -> Option<Serial> {
        let (entity, serial) = self
            .state
            .registry
            .spawn_with_serial(SerialKind::Item)
            .ok()?;
        let hue = body.map_or(0, |b| b.hue);
        self.state.registry.insert(
            entity,
            Graphic {
                id: CORPSE_GRAPHIC,
                hue,
            },
        );
        // Amount = body: the client draws item 0x2006 as this creature's corpse.
        if let Some(body) = body {
            self.state.registry.insert(entity, Amount(body.id));
        }
        self.state.registry.insert(entity, Position(at));
        self.state.registry.insert(entity, Facet(facet));
        self.state
            .registry
            .insert(entity, Container { gump: CORPSE_GUMP });
        self.state.registry.insert(entity, Name(name));
        // A corpse rots like clutter, but it is a container, so `mark_decay`
        // skips it — the timer is set here directly, and `items::decay` takes the
        // loot down with it.
        self.state.registry.insert(
            entity,
            Decays {
                at_tick: self.state.ticks + CORPSE_DECAY_TICKS,
            },
        );
        self.state.facet_state_mut(facet).sectors.insert(entity, at);
        self.state.reveal(entity);
        Some(serial)
    }

    /// Move every item worn by `mobile` into the corpse `container`.
    fn move_gear_to_corpse(&mut self, mobile: Serial, container: Serial) {
        let worn: Vec<EntityId> = self
            .state
            .registry
            .query::<Equipped>()
            .filter(|(_, equipped)| equipped.mobile == mobile)
            .map(|(entity, _)| entity)
            .collect();
        for (slot, item) in worn.into_iter().enumerate() {
            self.state.registry.remove::<Equipped>(item);
            self.state.registry.insert(
                item,
                Contained {
                    container,
                    x: 40 + (slot as u16) * 12,
                    y: 60,
                    grid: 0,
                },
            );
        }
    }

    /// The core's default corpse gold, scaled from the creature's toughness — a
    /// stand-in for the pack's loot tables; a tougher creature carries more. Uses
    /// the tick's seeded rng, so the drop replays.
    fn corpse_gold(&mut self, max_hits: u16) -> u16 {
        if max_hits == 0 {
            return 0;
        }
        // Half its hits, plus up to another half — a jittered handful.
        let base = max_hits / 2;
        let jitter = self.state.rng.below(u32::from(max_hits / 2 + 1)) as u16;
        base + jitter
    }

    /// Take a creature off the world: forget it from every screen, drop it from
    /// the sector grid, despawn it. The disposal half of the old `combat::die`.
    fn despawn_creature(&mut self, entity: EntityId, serial: Serial) {
        let facet = self.state.facet_of(entity);
        for watcher in self.state.watchers_of(entity) {
            self.state.forget(watcher, entity, serial);
        }
        self.state.seen.remove(&entity);
        self.state.facet_state_mut(facet).sectors.remove(entity);
        self.state.registry.despawn(entity);
    }
}
