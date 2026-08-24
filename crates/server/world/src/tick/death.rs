use super::*;
use openshard_protocol::containers::GridSlot;
use openshard_protocol::wire::{Graphic, Hue, Layer};
use openshard_state::components::{
    CORPSE_GRAPHIC, CORPSE_GUMP, Corpse, DEATH_SHROUD_GRAPHIC, Decays, creature_name, ghost_body,
};

/// How long a corpse lies before it rots away with its loot — ServUO's default
/// seven minutes, in ticks.
const CORPSE_DECAY_TICKS: u64 = 7 * 60 * TICKS_PER_SECOND;

/// The outer-torso layer the death shroud wears at — ServUO's `Layer.OuterTorso`.
const OUTER_TORSO_LAYER: Layer = Layer(0x16);
/// The outer-torso robe issued to a player brought back to life.
const RESURRECTION_ROBE_GRAPHIC: Graphic = Graphic(0x1F03);
/// The axe carried in the two-handed weapon slot after resurrection.
const RESURRECTION_AXE_GRAPHIC: Graphic = Graphic(0x0F49);

/// The two body graphics that share the skeleton loot table.
const SKELETON_BODIES: [Graphic; 2] = [Graphic(0x0032), Graphic(0x0038)];
/// A skeleton always carries a modest purse; the inclusive upper bound is 45.
const SKELETON_GOLD_MIN: u16 = 20;
const SKELETON_GOLD_SPREAD: u16 = 26;
/// The battered weapons a skeleton may leave behind.
const SKELETON_WEAPONS: [Graphic; 3] = [
    Graphic(0x0F52), // dagger
    Graphic(0x0F5C), // mace
    Graphic(0x0F5E), // broadsword
];
/// A small ration, picked when the corpse is made.
const SKELETON_FOOD: [Graphic; 3] = [
    Graphic(0x09D0), // apple
    Graphic(0x09F2), // ribs
    Graphic(0x103B), // bread
];

impl World {
    /// Dispose of every mobile that died this tick: a creature becomes a corpse
    /// and leaves the world; a player becomes a ghost, leaving a corpse but
    /// staying connected.
    ///
    /// Reads the tick's [`MobileDied`](openshard_combat::MobileDied) events — the
    /// "emit, don't call" seam: combat announces a death, the world disposes of the
    /// body.
    pub(super) fn reap(&mut self) {
        let dead: Vec<(EntityId, Serial, Option<Serial>)> = self
            .state
            .bus
            .read(&mut self.dead)
            .map(|event| (event.entity, event.serial, event.killer))
            .collect();
        for (entity, serial, killer) in dead {
            // Standing first, while the victim's own fame and karma can still be read:
            // ServUO's `BaseCreature.OnDeath` awards from the corpse's owner, and the
            // body is about to be swept.
            self.award_standing(entity, killer);
            // A body already gone — reaped once, or removed another way this tick —
            // is skipped. A ghost that dies again (it cannot, guarded elsewhere) is
            // likewise a no-op.
            if self.state.registry.entity_of(serial).is_none() || self.state.registry.has::<Ghost>(entity) {
                continue;
            }
            // Who struck last, by name and now, while the killer is certainly still
            // in the world: the corpse remembers a name rather than a serial, so
            // Forensics can still answer once the killer has logged out.
            let killer = killer
                .and_then(|s| self.state.registry.entity_of(s))
                .and_then(|k| self.state.registry.get::<Name>(k))
                .map(|name| name.0.clone());
            if self.state.registry.has::<Client>(entity) {
                self.become_ghost(entity, serial, killer);
            } else {
                self.lay_corpse(entity, serial, killer);
            }
        }
    }

    /// Award the killer the victim's fame, and karma by the victim's own sign —
    /// ServUO's `BaseCreature.OnDeath`, which hands `Titles.AwardFame(killer, Fame)`
    /// and `AwardKarma(killer, -Karma)` to whoever struck last.
    ///
    /// The sign is the whole rule: a creature carries *negative* karma when it is evil,
    /// so killing it awards its negation — a positive amount — and killing something
    /// innocent (positive karma) costs the killer. That is why a murderer's karma falls
    /// without anything needing to know what a murder is.
    fn award_standing(&mut self, victim: EntityId, killer: Option<Serial>) {
        let Some(killer) = killer.and_then(|s| self.state.registry.entity_of(s)) else {
            return; // an unattributed death earns nobody anything
        };
        if killer == victim {
            return;
        }
        let fame = self
            .state
            .registry
            .get::<openshard_state::components::Fame>(victim)
            .map_or(0, |f| f.0);
        let karma = self
            .state
            .registry
            .get::<openshard_state::components::Karma>(victim)
            .map_or(0, |k| k.0);
        if fame == 0 && karma == 0 {
            return; // a creature with no standing to give
        }
        let gained_fame = openshard_state::title::award_fame(&mut self.state, killer, fame);
        let gained_karma = openshard_state::title::award_karma(&mut self.state, killer, -karma);
        // Only a player is told; a creature has nobody to tell.
        if self.state.registry.has::<Client>(killer) {
            for line in [
                openshard_state::title::award_message(gained_fame, false),
                openshard_state::title::award_message(gained_karma, true),
            ]
            .into_iter()
            .flatten()
            {
                self.notify_self(killer, line);
            }
        }
    }

    /// Turn a dead player into a ghost: lay a corpse holding their gear (no gold —
    /// that is monster loot), then enter the ghost state, wearing a fresh death
    /// shroud. The player keeps their connection and can walk as a ghost;
    /// resurrection reverses every step of this.
    fn become_ghost(&mut self, entity: EntityId, serial: Serial, killer: Option<String>) {
        // The corpse first, while the gear is still worn — `move_gear_to_corpse`
        // reads the `Equipped` items off the mobile.
        if let Some(&Position(at)) = self.state.registry.get::<Position>(entity) {
            let facet = self.state.facet_of(entity);
            let body = self.state.registry.get::<Body>(entity).copied();
            // The heading the player died with — see `lay_corpse`.
            let facing = self
                .state
                .registry
                .get::<Heading>(entity)
                .map(|Heading(facing)| facing.direction);
            let owner = self
                .state
                .registry
                .get::<Name>(entity)
                .map_or_else(String::new, |n| n.0.clone());
            let name = if owner.is_empty() {
                "a corpse".to_owned()
            } else {
                format!("a corpse of {owner}")
            };
            let story = Corpse {
                owner,
                killer,
                ..Corpse::default()
            };
            if let Some(corpse) = self.spawn_corpse(at, facet, body, facing, name, story) {
                // Everyone but the dying player, who is about to be told by
                // `0x2C` and has a ghost to watch rather than a corpse to pair.
                self.state.announce_death(entity, Some(corpse));
                // A ghost keeps its backpack and bank box — worn containers, not
                // loot — and its mount saddle, which the `Riding` link still points
                // at (sweeping it into the corpse would strand the ridden creature
                // in limbo). Only its armour and weapons fall to the corpse.
                self.move_gear_to_corpse(
                    serial,
                    corpse,
                    &[items::BACKPACK_LAYER, npc::BANK_LAYER, items::MOUNT_LAYER],
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
    pub(super) fn enter_ghost_state(&mut self, entity: EntityId, serial: Serial, equip_shroud: bool) {
        // The living body, remembered so resurrection can restore it exactly —
        // colour and race included.
        let living = self.state.registry.get::<Body>(entity).copied().unwrap_or(Body {
            id: BODY_HUMAN_MALE,
            hue: openshard_protocol::wire::Hue(0),
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
            hue: openshard_protocol::wire::Hue(0),
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
        let Some(&Client { connection, .. }) = self.state.registry.get::<Client>(entity) else {
            return;
        };
        self.state
            .send_packet(connection, &ServerPacket::DeathStatus(DeathStatus { dead }));
        // Death is exactly where this byte changes: a ghost is stopped by
        // nobody, so `stance_of` puts `IGNORE_MOBILES` on it, and this `0x20` is
        // what tells the client keeping its own copy of the body-blocking rule.
        // Without it a ghost's walk home is refused at every body it passes —
        // by this end's own prediction, never by the shard.
        let flags = self.state.stance_of(entity);
        if let (Some(&Position(at)), Some(&Heading(facing))) = (
            self.state.registry.get::<Position>(entity),
            self.state.registry.get::<Heading>(entity),
        ) {
            self.state.send_packet(
                connection,
                &ServerPacket::PlayerUpdate(PlayerUpdate {
                    serial,
                    body: body.id,
                    hue: body.hue,
                    flags,
                    position: at,
                    facing,
                }),
            );
        }
        if let Some(mine) = self.state.mobile_incoming(entity, entity) {
            self.state
                .send_packet(connection, &ServerPacket::MobileIncoming(mine));
        }
    }

    /// Bring a ghost back to life: lift the [`Ghost`] marker, restore the living
    /// body it remembered, strip the death shroud, and tell the client it is alive
    /// again. Nothing happens to a mobile that is not a ghost. The corpse stays
    /// where it lies — a resurrected player walks back to loot it, as in UO.
    ///
    /// `full` decides how many hit points come back. A spell or a bandage — the
    /// price of which was surviving the fight that killed you, or somebody else's
    /// reagents and minutes — gives a tenth of max, ServUO's number, enough not to
    /// re-die on sight. A healer's free resurrection (`full: true`) restores the
    /// full pool instead, ServUO's `BaseHealer.OfferResurrection` — the price
    /// there is walking to a healer in town, not the fight itself.
    pub(super) fn resurrect(&mut self, entity: EntityId, full: bool) {
        let Some(&Ghost { body: living }) = self.state.registry.get::<Ghost>(entity) else {
            debug!(?entity, "resurrect: not a ghost");
            return;
        };
        let Some(serial) = self.state.registry.serial_of(entity) else {
            debug!(?entity, "resurrect: no serial");
            return;
        };
        debug!(?entity, full, "resurrect: bringing back to life");
        self.state.registry.remove::<Ghost>(entity);
        self.state.registry.insert(entity, living);
        self.strip_death_shroud(serial);
        self.equip_resurrection_kit(serial);

        // A healer's offer may still be standing (a bandage or a spell can land
        // while a ghost has not yet answered "wouldst thou like to be
        // resurrected?"). Any path back to life retires it — otherwise the
        // healer field on `Connection` outlives this death, and the next one
        // makes `offer_resurrection_nearby` treat a stale, out-of-range healer
        // as still pending: it closes and redraws the confirm on every step
        // near a healer instead of asking once.
        if let Some(row) = self.state.row_of_mut(entity) {
            row.healer_gump = None;
        }
        self.close_healer_gump(entity);

        // Back on its feet with hit points, not zero. A spell or bandage gives a
        // fraction — ServUO's roughly a tenth of the max, enough not to re-die on
        // sight — a healer's free resurrection gives the whole pool.
        if let Some(hits) = self.state.registry.get::<Hitpoints>(entity).copied() {
            let revived = if full { hits.max } else { (hits.max / 10).max(1) };
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
                        .get::<Drawn>(*item)
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
            Drawn {
                id: DEATH_SHROUD_GRAPHIC,
                hue: Hue(0),
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

    /// Give a resurrected player the shard's minimal fighting kit. Their former
    /// equipment remains on the corpse, so these fresh items are intentionally
    /// separate from it: a robe plus one axe are immediately usable. A weapon
    /// in the two-handed layer excludes the main hand, so issuing a dagger as
    /// well would create an impossible outfit.
    fn equip_resurrection_kit(&mut self, mobile: Serial) {
        let hue = Hue(0);
        let _ = items::equip_worn_item(
            &mut self.state,
            mobile,
            RESURRECTION_ROBE_GRAPHIC,
            hue,
            OUTER_TORSO_LAYER,
        );
        let _ = items::equip_worn_item(
            &mut self.state,
            mobile,
            RESURRECTION_AXE_GRAPHIC,
            hue,
            openshard_state::weapon::LAYER_TWO_HANDED,
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
    /// Roll the shard's own loot table for a body into a corpse.
    ///
    /// Every roll is on the world's seeded generator, so a replayed tick fills the
    /// same corpse the same way — the guarantee the script pack that held these
    /// tables exempted itself from.
    ///
    /// A drop that rolls a range takes its `least` when the two are equal, so a
    /// fixed count costs no roll: the rng's sequence is part of what replays, and
    /// spending a draw on a decision with one outcome would move everything after
    /// it.
    pub(super) fn roll_shipped_loot(&mut self, corpse: Serial, body: Graphic) {
        let Some(drops) = crate::loot::table(body) else {
            return;
        };
        for drop in drops {
            if drop.percent < 100 && self.state.rng.below(100) >= drop.percent {
                continue;
            }
            let amount = if drop.most > drop.least {
                drop.least + self.state.rng.below(u32::from(drop.most - drop.least) + 1) as u16
            } else {
                drop.least
            };
            self.add_loot(corpse, drop.graphic, drop.hue, amount, drop.stackable);
        }
    }

    pub(super) fn add_loot(
        &mut self,
        container: Serial,
        graphic: Graphic,
        hue: Hue,
        amount: u16,
        stackable: bool,
    ) {
        let is_container = self
            .state
            .registry
            .entity_of(container)
            .is_some_and(|entity| self.state.registry.has::<Container>(entity));
        if !is_container || amount == 0 {
            return;
        }
        if stackable {
            let _ = items::give(&mut self.state, container, graphic, hue, u32::from(amount));
        } else {
            let _ = items::place_one(&mut self.state, container, graphic, hue, amount);
        }
    }

    /// Turn one dead creature into a corpse holding its gear and a little gold,
    /// then despawn the creature.
    fn lay_corpse(&mut self, entity: EntityId, serial: Serial, killer: Option<String>) {
        let Some(&Position(at)) = self.state.registry.get::<Position>(entity) else {
            // No position (a mount in limbo, say) — nothing to lay a corpse on.
            self.despawn_creature(entity, serial);
            return;
        };
        let facet = self.state.facet_of(entity);
        let body = self.state.registry.get::<Body>(entity).copied();
        // Which way it fell: the heading it died with, read while the body is
        // still in the world. The run bit is dropped — a corpse does not run,
        // and the client masks it off anyway.
        let facing = self
            .state
            .registry
            .get::<Heading>(entity)
            .map(|Heading(facing)| facing.direction);
        let max_hits = self.state.registry.get::<Hitpoints>(entity).map_or(0, |h| h.max);
        // A creature's own name if the pack gave it one, else its kind's.
        let owner = self
            .state
            .registry
            .get::<Name>(entity)
            .map(|n| n.0.clone())
            .or_else(|| body.and_then(|b| creature_name(b.id)).map(str::to_owned));
        let name = owner
            .as_ref()
            .map_or_else(|| "a corpse".to_owned(), |n| format!("a corpse of {n}"));
        let story = Corpse {
            owner: owner.unwrap_or_default(),
            killer,
            ..Corpse::default()
        };

        let Some(corpse) = self.spawn_corpse(at, facet, body, facing, name, story) else {
            self.despawn_creature(entity, serial);
            return;
        };
        // Which corpse this body became, said while the body is still in the
        // world for its watchers to have it. Without this the client pairs the
        // fall it is playing with a corpse by tile, and two of the same creature
        // dying together swap falls.
        self.state.announce_death(entity, Some(corpse));
        // Its worn gear falls into the corpse. Named creature tables add their
        // own loot; every other creature keeps the core's baseline gold.
        self.move_gear_to_corpse(serial, corpse, &[]);
        self.fill_creature_loot(corpse, body.map(|body| body.id), max_hits);
        // The shard's own table, on top of the baseline. Rolled here rather than
        // off the event below, because content in the tree is part of the tick
        // rather than a listener — see `crate::loot`.
        let dropped_for = body.map_or(Graphic(0), |b| b.id);
        self.roll_shipped_loot(corpse, dropped_for);
        // The loot hook: a pack adds its own table on top, by serial, off this
        // event. Emitted before the creature is despawned so `body` is still
        // readable if a listener wants it live.
        self.state.bus.send(CorpseCreated {
            corpse,
            body: dropped_for,
        });
        self.despawn_creature(entity, serial);
    }

    /// Spawn a corpse item at `at`, drawn as `body` lying `facing` and named
    /// `name`, and return its serial. A container (the loot window) that rots
    /// after a while.
    fn spawn_corpse(
        &mut self,
        at: Point,
        facet: Facet,
        body: Option<Body>,
        facing: Option<Direction>,
        name: String,
        story: Corpse,
    ) -> Option<Serial> {
        let (entity, serial) = self.state.registry.spawn_with_serial(SerialKind::Item).ok()?;
        let hue = body.map_or(Hue(0), |b| b.hue);
        self.state.registry.insert(
            entity,
            Drawn {
                id: CORPSE_GRAPHIC,
                hue,
            },
        );
        // What the corpse is a picture of. The wire puts the body id in `0x1A`'s
        // stack word (it is not a stack size) and the facing in the direction
        // byte, because the client draws the last frame of that body's death
        // group *for a direction* — a body with no direction is half a corpse.
        //
        // The pair is inserted only when the dead mobile had both. That is not a
        // guard against a missing heading so much as the same condition twice: a
        // mobile the client could draw at all has a body and a heading — see
        // `WorldState::mobile_incoming`, which answers `None` without either — so
        // a bodiless death leaves the plain sack it always left.
        if let (Some(body), Some(facing)) = (body, facing) {
            self.state.registry.insert(
                entity,
                openshard_state::components::CorpseBody {
                    body: body.id,
                    facing,
                },
            );
        }
        self.state.registry.insert(entity, Position(at));
        self.state.registry.insert(entity, facet);
        self.state
            .registry
            .insert(entity, Container { gump: CORPSE_GUMP });
        self.state.registry.insert(entity, Name(name));
        // How it came to be a corpse: what Forensic Evaluation reads, and what the
        // save sweeps along with the rest of the item.
        self.state.registry.insert(entity, story);
        // A corpse rots like clutter, but it is a container, so `mark_decay`
        // skips it — the timer is set here directly, and `items::decay` takes the
        // loot down with it.
        self.state.registry.insert(
            entity,
            Decays {
                at_tick: self.state.ticks + CORPSE_DECAY_TICKS,
            },
        );
        // A corpse is an item and always was: it is a container with a body
        // graphic, not a body, so it goes in the list the living are not read
        // out of.
        self.state.place_item(facet, entity, at);
        self.state.reveal(entity);
        Some(serial)
    }

    /// Move every item worn by `mobile` into the corpse `container`, skipping any
    /// layer in `keep`. A creature keeps nothing; a player keeps its backpack and
    /// bank box — those are worn containers it walks away (as a ghost) still
    /// holding, not loot for the corpse. The worn *gear* still drops.
    fn move_gear_to_corpse(&mut self, mobile: Serial, container: Serial, keep: &[Layer]) {
        let worn: Vec<(usize, EntityId)> = self
            .state
            .registry
            .query::<Equipped>()
            .filter(|(_, equipped)| equipped.mobile == mobile && !keep.contains(&equipped.layer))
            .enumerate()
            .map(|(slot, (entity, _))| (slot, entity))
            .collect();
        for (slot, item) in worn {
            self.state.registry.remove::<Equipped>(item);
            self.state.registry.insert(
                item,
                Contained {
                    container,
                    position: GumpPoint::new(40 + i32::try_from(slot).unwrap_or(0) * 12, 60),
                    grid: GridSlot(0),
                },
            );
        }
    }

    /// The core's default corpse gold, scaled from the creature's toughness — a
    /// baseline beneath `loot::table`; a tougher creature carries more. Uses
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

    /// Fill the built-in loot table for a creature corpse. Packs can still add
    /// their own loot through [`CorpseCreated`] below.
    fn fill_creature_loot(&mut self, corpse: Serial, body: Option<Graphic>, max_hits: u16) {
        if body.is_some_and(|body| SKELETON_BODIES.contains(&body)) {
            let gold = SKELETON_GOLD_MIN + self.state.rng.below(u32::from(SKELETON_GOLD_SPREAD)) as u16;
            let weapon = SKELETON_WEAPONS[self.state.rng.below(SKELETON_WEAPONS.len() as u32) as usize];
            let gold = items::give(
                &mut self.state,
                corpse,
                items::GOLD_GRAPHIC,
                Hue(0),
                u32::from(gold),
            );
            let weapon = items::place_one(&mut self.state, corpse, weapon, Hue(0), 1);
            let bandage = items::place_one(
                &mut self.state,
                corpse,
                openshard_skills::BANDAGE_GRAPHIC,
                Hue(0),
                1,
            );
            let food = SKELETON_FOOD[self.state.rng.below(SKELETON_FOOD.len() as u32) as usize];
            let food = items::place_one(&mut self.state, corpse, food, Hue(0), 1);
            for (item, x, y) in [
                (gold, 35, 50),
                (weapon, 85, 50),
                (bandage, 35, 100),
                (food, 85, 100),
            ] {
                if let Some(item) = item {
                    self.state.registry.insert(
                        item,
                        Contained {
                            container: corpse,
                            position: GumpPoint::new(x, y),
                            grid: GridSlot(0),
                        },
                    );
                }
            }
            return;
        }

        let gold = self.corpse_gold(max_hits);
        if gold > 0 {
            let _ = items::give(
                &mut self.state,
                corpse,
                items::GOLD_GRAPHIC,
                Hue(0),
                u32::from(gold),
            );
        }
    }

    /// Take a creature off the world. The disposal half of the old `combat::die`;
    /// the removal itself is the substrate's, shared with anything else that
    /// takes a mobile out (a guard that has done its work).
    fn despawn_creature(&mut self, entity: EntityId, _serial: Serial) {
        self.state.despawn_mobile(entity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openshard_protocol::serial::SerialKind;
    use openshard_state::components::{Amount, Contained, Container, Drawn};

    #[test]
    fn a_skeleton_corpse_has_visible_gold_weapon_and_supplies() {
        let mut world = World::new((1363, 1600));
        let (corpse, corpse_serial) = world
            .state
            .registry
            .spawn_with_serial(SerialKind::Item)
            .expect("an item serial for the corpse");
        world
            .state
            .registry
            .insert(corpse, Container { gump: CORPSE_GUMP });

        world.fill_creature_loot(corpse_serial, Some(SKELETON_BODIES[0]), 0);

        let loot: Vec<(Graphic, u16)> = world
            .state
            .registry
            .query::<Contained>()
            .filter(|(_, held)| held.container == corpse_serial)
            .filter_map(|(entity, _)| {
                world.state.registry.get::<Drawn>(entity).map(|drawn| {
                    (
                        drawn.id,
                        world
                            .state
                            .registry
                            .get::<Amount>(entity)
                            .map_or(1, |amount| amount.0),
                    )
                })
            })
            .collect();
        let gold = loot
            .iter()
            .find_map(|&(graphic, amount)| (graphic == items::GOLD_GRAPHIC).then_some(amount))
            .expect("a skeleton always carries gold");

        assert!(
            (SKELETON_GOLD_MIN..SKELETON_GOLD_MIN + SKELETON_GOLD_SPREAD).contains(&gold),
            "the skeleton's purse stays within its loot table"
        );
        assert!(
            loot.iter().any(|(graphic, _)| SKELETON_WEAPONS.contains(graphic)),
            "a skeleton leaves one of its weapons behind"
        );
        assert!(
            loot.iter()
                .any(|(graphic, _)| *graphic == openshard_skills::BANDAGE_GRAPHIC),
            "a skeleton carries a clean bandage"
        );
        assert!(
            loot.iter().any(|(graphic, _)| SKELETON_FOOD.contains(graphic)),
            "a skeleton carries one random ration"
        );
        let positions: Vec<GumpPoint> = world
            .state
            .registry
            .query::<Contained>()
            .filter(|(_, held)| held.container == corpse_serial)
            .map(|(_, held)| held.position)
            .collect();
        assert_eq!(positions.len(), 4, "all skeleton loot was created");
        assert!(
            positions
                .iter()
                .enumerate()
                .all(|(index, position)| positions[..index].iter().all(|other| other != position)),
            "each loot icon has its own place in the corpse gump"
        );
    }
}
