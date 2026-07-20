//! The cast sequence: the wire and timing around a spell, over the core
//! spell table in `magic`.
//!
//! Two shapes, chosen by `gameplay.cast_style`:
//!
//! - **`Walk` (Sphere)** — a cast resolves the instant it is asked: mana and
//!   reagents are spent, the skill rolled, and the effect (or its target cursor)
//!   comes up at once, with no rooting. The caster keeps walking.
//! - **`Stop` (ServUO/UO)** — the caster is committed to a [`Casting`] over a
//!   cast delay; moving breaks it, and taking damage disturbs it when the shard
//!   runs `spell_disturb`. Only when the delay runs out does it resolve, and only
//!   then does a targeted spell raise its cursor.
//!
//! The *effect* is the core's default (damage, heal, teleport for the spells the
//! engine can run today; the rest are `Scripted` and left to the pack, which
//! reads the same [`SpellCast`] event). This module never decides what a spell
//! *does* beyond dispatching on the table's archetype.

use super::*;
use openshard_magic::{SpellEffect, SpellTarget};
use openshard_protocol::encode_target_cursor;
use openshard_state::components::{Casting, Skills};
use openshard_state::{CastStyle, TargetPurpose};

/// The skill a spell rolls — Magery, id 25.
const MAGERY_SKILL: u8 = 25;
/// The layer a backpack rides on, where reagents are kept.
const BACKPACK_LAYER: u8 = 0x15;

impl World {
    /// A client asked to cast a spell (`0xBF`). Begin it: right away in the
    /// Sphere style, or as a rooted [`Casting`] with a cast delay in the ServUO
    /// style. An unknown spell id or a dead caster is ignored.
    pub(super) fn begin_cast(&mut self, connection: ConnectionId, spell: u16) {
        let Some(&caster) = self.state.players.get(&connection) else {
            return;
        };
        let Some(info) = magic::info(spell) else {
            return; // past the eighth circle; not a spell
        };
        if self
            .state
            .registry
            .get::<Hitpoints>(caster)
            .is_some_and(|h| h.current == 0)
        {
            return; // the dead do not cast
        }
        match self.state.gameplay.cast_style {
            CastStyle::Walk => self.resolve_cast(caster, spell),
            CastStyle::Stop => {
                if self.state.registry.has::<Casting>(caster) {
                    return; // already mid-cast — one at a time
                }
                let delay = magic::cast_delay_ticks(info, TICKS_PER_SECOND);
                self.state.registry.insert(
                    caster,
                    Casting {
                        spell,
                        complete_at: self.state.ticks + delay,
                    },
                );
            }
        }
    }

    /// Advance the ServUO-style casts once per tick: break any the caster took a
    /// disturbing blow to, then resolve those whose delay has run out.
    pub(super) fn advance_casts(&mut self) {
        // Disturb first, so a cast hit *and* due this tick is broken, not cast.
        if self.state.gameplay.spell_disturb {
            let hurt: Vec<EntityId> = self
                .state
                .bus
                .read(&mut self.disturbed)
                .map(|event| event.entity)
                .collect();
            for entity in hurt {
                if self.state.registry.remove::<Casting>(entity).is_some() {
                    self.notify_self(entity, "Your concentration is broken.");
                }
            }
        }
        // Then the casts whose delay is up.
        let now = self.state.ticks;
        let ready: Vec<(EntityId, u16)> = self
            .state
            .registry
            .query::<Casting>()
            .filter(|(_, casting)| now >= casting.complete_at)
            .map(|(entity, casting)| (entity, casting.spell))
            .collect();
        for (caster, spell) in ready {
            self.state.registry.remove::<Casting>(caster);
            self.resolve_cast(caster, spell);
        }
    }

    /// Pay for a cast and roll it, then either land a self-cast now or raise the
    /// target cursor a targeted spell waits on. A fizzle (short mana or a
    /// reagent) says so and stops.
    fn resolve_cast(&mut self, caster: EntityId, spell: u16) {
        let Some(info) = magic::info(spell) else {
            return;
        };
        let Some(serial) = self.state.registry.serial_of(caster) else {
            return;
        };
        let reagents: Vec<(u16, u16)> = info.reagents.iter().map(|&graphic| (graphic, 1)).collect();
        let pack = self.caster_pack(serial);
        let Some(success) = magic::pay_and_roll(
            &mut self.state,
            caster,
            magic::mana(info),
            magic::difficulty(info),
            MAGERY_SKILL,
            pack,
            &reagents,
        ) else {
            self.state.bus.send(magic::SpellCast {
                caster,
                serial,
                spell,
                target: 0,
                success: false,
            });
            self.notify_self(caster, "You lack the mana or reagents to cast that.");
            return;
        };

        match info.target {
            SpellTarget::SelfCast => {
                // No cursor: it lands on the caster or the ground around them.
                self.state.bus.send(magic::SpellCast {
                    caster,
                    serial,
                    spell,
                    target: 0,
                    success,
                });
                if success {
                    let at = self.caster_position(caster);
                    self.apply_spell_effect(caster, spell, 0, at);
                }
            }
            SpellTarget::Mobile | SpellTarget::Location => {
                // Raise the cursor; the effect and the `SpellCast` wait for the
                // aim (see `handle_target`). A creature with no client cannot aim,
                // so its targeted cast simply lapses.
                if let Some(&Client { connection, .. }) = self.state.registry.get::<Client>(caster)
                {
                    self.state
                        .pending_targets
                        .insert(caster, TargetPurpose::Spell { spell, success });
                    self.state
                        .send(connection, encode_target_cursor(serial.raw()));
                }
            }
        }
    }

    /// Run a spell's core effect on its aim. Called immediately for a self-cast,
    /// and from the target cursor's answer for a targeted one. `Scripted`
    /// archetypes do nothing here — the pack owns them off the `SpellCast` event.
    pub(super) fn apply_spell_effect(
        &mut self,
        caster: EntityId,
        spell: u16,
        target_serial: u32,
        target_location: Point,
    ) {
        let Some(info) = magic::info(spell) else {
            return;
        };
        let by = self.state.registry.serial_of(caster);
        match info.effect {
            SpellEffect::Damage(kind, base) => {
                if target_serial != 0 {
                    combat::damage(&mut self.state, target_serial, base, kind, by);
                }
            }
            SpellEffect::AreaDamage(kind, base) => {
                // Centre on the caster for a self-cast (Earthquake), on the aimed
                // spot otherwise (Chain Lightning, Meteor Swarm).
                let centre = if matches!(info.target, SpellTarget::SelfCast) {
                    self.caster_position(caster)
                } else {
                    target_location
                };
                let facet = self.state.facet_of(caster);
                let victims: Vec<u32> = self
                    .state
                    .facet_state(facet)
                    .sectors
                    .nearby(centre, magic::AREA_RADIUS)
                    .filter(|(entity, _)| {
                        *entity != caster && self.state.registry.has::<Body>(*entity)
                    })
                    .filter_map(|(entity, _)| {
                        self.state.registry.serial_of(entity).map(|s| s.raw())
                    })
                    .collect();
                for victim in victims {
                    combat::damage(&mut self.state, victim, base, kind, by);
                }
            }
            SpellEffect::Heal(amount) => {
                let who = if target_serial != 0 {
                    target_serial
                } else {
                    by.map_or(0, |s| s.raw())
                };
                magic::heal(&mut self.state, who, amount);
            }
            SpellEffect::Poison => {
                if target_serial != 0 {
                    // The dose scales with the caster's Magery — a novice lands a
                    // lesser poison, a master a greater one (Poisoning, the
                    // deadlier levels, is a later skill).
                    let magery = self
                        .state
                        .registry
                        .get::<Skills>(caster)
                        .map_or(0, |s| s.get(25));
                    let level = ((magery / 300) as u8).min(2);
                    let now = self.state.ticks;
                    combat::apply_poison(&mut self.state, target_serial, level, now);
                }
            }
            SpellEffect::Cure => {
                let who = if target_serial != 0 {
                    target_serial
                } else {
                    by.map_or(0, |s| s.raw())
                };
                combat::cure_poison(&mut self.state, who);
            }
            SpellEffect::AreaCure => {
                let facet = self.state.facet_of(caster);
                let healed: Vec<u32> = self
                    .state
                    .facet_state(facet)
                    .sectors
                    .nearby(target_location, magic::AREA_RADIUS)
                    .filter(|(entity, _)| self.state.registry.has::<Body>(*entity))
                    .filter_map(|(entity, _)| {
                        self.state.registry.serial_of(entity).map(|s| s.raw())
                    })
                    .collect();
                for mobile in healed {
                    combat::cure_poison(&mut self.state, mobile);
                }
            }
            SpellEffect::Teleport => {
                self.state.teleport(caster, target_location);
                self.state.broadcast_move(caster);
            }
            SpellEffect::StatMod(kind) => {
                // A Mobile-target spell, so it lands on the aimed mobile — or on
                // the caster for a self-cast that answered its own cursor.
                let who = if target_serial != 0 {
                    target_serial
                } else {
                    by.map_or(0, |s| s.raw())
                };
                if who != 0 {
                    let (offset, expires_at) = self.stat_buff_terms(caster, kind);
                    magic::apply_stat_buff(&mut self.state, who, kind, offset, expires_at);
                    self.refresh_status_of(who);
                }
            }
            SpellEffect::Scripted => {} // the pack's, off SpellCast
        }
    }

    /// How strong a stat buff the caster lands, and the tick it lifts.
    ///
    /// Both scale from the caster's Magery, ServUO's shape: the magnitude rises to
    /// `+10` at grandmaster, the duration to a couple of minutes. A debuff kind
    /// takes the same magnitude with the sign flipped — the negation the `magic`
    /// crate then folds in and, later, backs out.
    fn stat_buff_terms(&self, caster: EntityId, kind: u8) -> (i16, u64) {
        let magery = self
            .state
            .registry
            .get::<Skills>(caster)
            .map_or(0, |s| s.get(MAGERY_SKILL));
        let magnitude = (magery / 100).clamp(1, 10) as i16;
        let offset = if openshard_state::is_debuff(kind) {
            -magnitude
        } else {
            magnitude
        };
        let seconds = u64::from(magery / 10).clamp(10, 120);
        (offset, self.state.ticks + seconds * TICKS_PER_SECOND)
    }

    /// The backpack serial reagents come out of, or `0` if the caster wears none.
    fn caster_pack(&self, caster: Serial) -> u32 {
        self.state
            .registry
            .query::<Equipped>()
            .find(|(_, worn)| worn.mobile == caster && worn.layer == BACKPACK_LAYER)
            .and_then(|(item, _)| self.state.registry.serial_of(item))
            .map_or(0, |s| s.raw())
    }

    /// Where a caster stands, or the origin if it somehow has no position.
    fn caster_position(&self, caster: EntityId) -> Point {
        self.state
            .registry
            .get::<Position>(caster)
            .map_or(Point::new(0, 0, 0), |p| p.0)
    }

    /// A private system line to a player, if it is one. A creature hears nothing.
    pub(super) fn notify_self(&mut self, entity: EntityId, text: &str) {
        if let Some(&Client { connection, .. }) = self.state.registry.get::<Client>(entity) {
            let packet = encode_message(
                openshard_protocol::SYSTEM_SERIAL,
                openshard_protocol::NO_GRAPHIC,
                0,
                0x03B2,
                3,
                "System",
                text,
            );
            self.state.send(connection, packet);
        }
    }
}
