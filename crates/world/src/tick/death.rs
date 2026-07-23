use super::*;
use openshard_protocol::encode_death_status;
use openshard_state::components::{
    creature_name, ghost_body, Decays, CORPSE_GRAPHIC, CORPSE_GUMP, DEATH_SHROUD_GRAPHIC,
};

/// Gold, the core's default corpse loot until the pack owns real loot tables.
const GOLD_GRAPHIC: u16 = 0x0EED;
/// How long a corpse lies before it rots away with its loot — ServUO's default
/// seven minutes, in ticks.
const CORPSE_DECAY_TICKS: u64 = 7 * 60 * TICKS_PER_SECOND;

/// The outer-torso layer the death shroud wears at — ServUO's `Layer.OuterTorso`.
const OUTER_TORSO_LAYER: u8 = 0x16;

impl World {
    /// Dispose of every mobile that died this tick: a creature becomes a corpse
    /// and leaves the world; a player becomes a ghost, leaving a corpse but
    /// staying connected.
    ///
    /// Reads the tick's [`MobileDied`](openshard_combat::MobileDied) events — the
    /// "emit, don't call" seam: combat announces a death, the world disposes of the
    /// body.
    pub(super) fn reap(&mut self) {
        let dead: Vec<(EntityId, Serial)> = self
            .state
            .bus
            .read(&mut self.dead)
            .map(|event| (event.entity, event.serial))
            .collect();
        for (entity, serial) in dead {
            // A body already gone — reaped once, or removed another way this tick —
            // is skipped. A ghost that dies again (it cannot, guarded elsewhere) is
            // likewise a no-op.
            if self.state.registry.entity_of(serial).is_none()
                || self.state.registry.has::<Ghost>(entity)
            {
                continue;
            }
            if self.state.registry.has::<Client>(entity) {
                self.become_ghost(entity, serial);
            } else {
                self.lay_corpse(entity, serial);
            }
        }
    }

    /// Turn a dead player into a ghost: lay a corpse holding their gear (no gold —
    /// that is monster loot), then enter the ghost state, wearing a fresh death
    /// shroud. The player keeps their connection and can walk as a ghost;
    /// resurrection reverses every step of this.
    fn become_ghost(&mut self, entity: EntityId, serial: Serial) {
        // The corpse first, while the gear is still worn — `move_gear_to_corpse`
        // reads the `Equipped` items off the mobile.
        if let Some(&Position(at)) = self.state.registry.get::<Position>(entity) {
            let facet = self.state.facet_of(entity);
            let body = self.state.registry.get::<Body>(entity).copied();
            let name = self
                .state
                .registry
                .get::<Name>(entity)
                .map_or_else(|| "a corpse".to_owned(), |n| format!("a corpse of {}", n.0));
            if let Some(corpse) = self.spawn_corpse(at, facet, body, name) {
                // A ghost keeps its backpack and bank box — worn containers, not
                // loot — and its mount saddle, which the `Riding` link still points
                // at (sweeping it into the corpse would strand the ridden creature
                // in limbo). Only its armour and weapons fall to the corpse.
                self.move_gear_to_corpse(
                    serial,
                    corpse,
                    &[BACKPACK_LAYER, npc::BANK_LAYER, items::MOUNT_LAYER],
                );
            }
        }
        self.enter_ghost_state(entity, serial, true);
    }

    /// Put a player into the ghost state: grey the body, remember the living one
    /// on the [`Ghost`] marker, drop war and target, tell the client it is dead,
    /// and rebuild every screen. Shared by a fresh death (`equip_shroud` true,
    /// which puts a new shroud on) and a relog of an already-dead character
    /// (`equip_shroud` false — its saved shroud came back with its inventory).
    pub(super) fn enter_ghost_state(
        &mut self,
        entity: EntityId,
        serial: Serial,
        equip_shroud: bool,
    ) {
        // The living body, remembered so resurrection can restore it exactly —
        // colour and race included.
        let living = self
            .state
            .registry
            .get::<Body>(entity)
            .copied()
            .unwrap_or(Body {
                id: BODY_HUMAN_MALE,
                hue: 0,
            });

        // War is over, and a ghost holds no target. Clearing `Combat` also stops
        // `swings` from striking on with a dead body.
        self.state
            .registry
            .remove::<openshard_state::components::Combat>(entity);
        self.state.registry.insert(entity, Ghost { body: living });
        // Rise in the ghost body.
        let ghost = Body {
            id: ghost_body(living.id),
            hue: 0,
        };
        self.state.registry.insert(entity, ghost);
        if equip_shroud {
            self.equip_death_shroud(serial);
        }

        // Tell the client it is dead (greys the world, gives the ghost walk),
        // redraw its own greyed body, and refresh its paperdoll (armour gone to
        // the corpse, a shroud in its place). Then rebuild every screen — the
        // living forget the ghost, ghosts and staff see it in its new body.
        self.tell_own_client_body(entity, serial, true, ghost);
        self.redraw_after_body_change(entity, serial);
    }

    /// Tell a player's own client its body just changed: the death status
    /// (`0x2C`), a fresh `0x20` that redraws its own avatar, and its own `0x78` so
    /// the paperdoll shows the right body and worn items. [`reveal`] never draws a
    /// mobile to itself, so this is the only place the player hears about its own
    /// change — the death-and-resurrection counterpart of what `enter` sends once.
    ///
    /// [`reveal`]: openshard_state::WorldState::reveal
    fn tell_own_client_body(&mut self, entity: EntityId, serial: Serial, dead: bool, body: Body) {
        let Some(&Client {
            connection,
            version,
        }) = self.state.registry.get::<Client>(entity)
        else {
            return;
        };
        self.state.send(connection, encode_death_status(dead));
        if let (Some(&Position(at)), Some(&Heading(facing))) = (
            self.state.registry.get::<Position>(entity),
            self.state.registry.get::<Heading>(entity),
        ) {
            self.state.send(
                connection,
                PlayerUpdate {
                    serial: serial.raw(),
                    body: body.id,
                    hue: body.hue,
                    flags: 0,
                    position: at,
                    facing,
                }
                .encode(),
            );
        }
        if let Some(mine) = self.state.mobile_incoming(entity) {
            self.state.send(connection, mine.encode(version));
        }
    }

    /// Bring a ghost back to life: lift the [`Ghost`] marker, restore the living
    /// body it remembered, strip the death shroud, and tell the client it is alive
    /// again. Restores a share of hit points so the raised player is not standing
    /// at zero, one blow from dying again. Nothing happens to a mobile that is not
    /// a ghost. The corpse stays where it lies — a resurrected player walks back to
    /// loot it, as in UO.
    pub(super) fn resurrect(&mut self, entity: EntityId) {
        let Some(&Ghost { body: living }) = self.state.registry.get::<Ghost>(entity) else {
            return;
        };
        let Some(serial) = self.state.registry.serial_of(entity) else {
            return;
        };
        self.state.registry.remove::<Ghost>(entity);
        self.state.registry.insert(entity, living);
        self.strip_death_shroud(serial);

        // Back on its feet with a fraction of its hit points, not zero — ServUO
        // resurrects to roughly a tenth of the max, enough to not re-die on sight.
        if let Some(hits) = self.state.registry.get::<Hitpoints>(entity).copied() {
            let revived = (hits.max / 10).max(1);
            self.state.registry.insert(
                entity,
                Hitpoints {
                    current: revived,
                    max: hits.max,
                },
            );
        }

        // Tell its own client it is alive again, then let the living see it once
        // more: forget the ghost body everywhere, reveal the living one. The
        // refreshed health bar rides the fresh `0x78` draw.
        self.tell_own_client_body(entity, serial, false, living);
        self.redraw_after_body_change(entity, serial);
    }

    /// Despawn the death shroud a ghost wears, if any. The mobile's fresh `0x78`
    /// in [`redraw_after_body_change`](Self::redraw_after_body_change) is what tells
    /// watchers it is no longer worn — a worn item rides the mobile's equipment
    /// list, not the `seen` set, so despawning it here and redrawing the mobile is
    /// the whole of taking it off.
    fn strip_death_shroud(&mut self, mobile: Serial) {
        let shroud: Option<EntityId> = self
            .state
            .registry
            .query::<Equipped>()
            .find(|(item, worn)| {
                worn.mobile == mobile
                    && self
                        .state
                        .registry
                        .get::<Graphic>(*item)
                        .is_some_and(|g| g.id == DEATH_SHROUD_GRAPHIC)
            })
            .map(|(item, _)| item);
        if let Some(item) = shroud {
            self.state.registry.despawn(item);
        }
    }

    /// Equip a fresh death shroud on a ghost, at the outer-torso layer — the grey
    /// robe a dead player rises in. Resurrection strips it.
    fn equip_death_shroud(&mut self, mobile: Serial) {
        let Ok((item, _)) = self.state.registry.spawn_with_serial(SerialKind::Item) else {
            return;
        };
        self.state.registry.insert(
            item,
            Graphic {
                id: DEATH_SHROUD_GRAPHIC,
                hue: 0,
            },
        );
        self.state.registry.insert(
            item,
            Equipped {
                mobile,
                layer: OUTER_TORSO_LAYER,
            },
        );
    }

    /// Forget a mobile whose body just changed from every screen, then reveal it
    /// afresh — the only way to restyle a mobile the client already drew, since
    /// there is no "change body" packet for someone else's mobile. The visibility
    /// gate in [`show`](openshard_state::WorldState::show) decides who gets the new
    /// draw: for a fresh ghost, the living do not.
    fn redraw_after_body_change(&mut self, entity: EntityId, serial: Serial) {
        for watcher in self.state.watchers_of(entity) {
            self.state.forget(watcher, entity, serial);
        }
        self.state.reveal(entity);
    }

    /// Put a loot item into a container by serial — the pack filling a corpse off
    /// a [`CorpseCreated`](crate::events::CorpseCreated) event. Guarded on the
    /// target being a real container, so a stray or stale serial adds nothing
    /// rather than conjuring a floating item. A stackable merges (gold, reagents);
    /// a discrete piece (a weapon) is placed whole.
    pub(super) fn add_loot(
        &mut self,
        container: u32,
        graphic: u16,
        hue: u16,
        amount: u16,
        stackable: bool,
    ) {
        let Some(container) = Serial::new(container) else {
            return;
        };
        let is_container = self
            .state
            .registry
            .entity_of(container)
            .is_some_and(|entity| self.state.registry.has::<Container>(entity));
        if !is_container || amount == 0 {
            return;
        }
        if stackable {
            let _ = items::give(&mut self.state, container, graphic, hue, amount);
        } else {
            let _ = items::place_one(&mut self.state, container, graphic, hue, amount);
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
        // Its worn gear falls into the corpse, and a flat baseline of gold — the
        // core's default so a bare shard still loots.
        self.move_gear_to_corpse(serial, corpse, &[]);
        let gold = self.corpse_gold(max_hits);
        if gold > 0 {
            let _ = items::give(&mut self.state, corpse, GOLD_GRAPHIC, 0, gold);
        }
        // The loot hook: a pack adds the real per-creature table on top of the
        // baseline, by serial, off this event. Emitted before the creature is
        // despawned so `body` is still readable if a listener wants it live.
        self.state.bus.send(CorpseCreated {
            corpse,
            body: body.map_or(0, |b| b.id),
        });
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

    /// Move every item worn by `mobile` into the corpse `container`, skipping any
    /// layer in `keep`. A creature keeps nothing; a player keeps its backpack and
    /// bank box — those are worn containers it walks away (as a ghost) still
    /// holding, not loot for the corpse. The worn *gear* still drops.
    fn move_gear_to_corpse(&mut self, mobile: Serial, container: Serial, keep: &[u8]) {
        let worn: Vec<EntityId> = self
            .state
            .registry
            .query::<Equipped>()
            .filter(|(_, equipped)| equipped.mobile == mobile && !keep.contains(&equipped.layer))
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
