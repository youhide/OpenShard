//! Persistent field spells: a row of ground tiles laid at the aimed spot that
//! pulse harm (Fire, Poison) or bar the way (Energy, Stone) until their tick
//! comes. Laid from `apply_spell_effect`, pulsed and expired by [`World::field_tick`]
//! on the tick counter — the [`combat::poison_tick`] shape, so a field replays like
//! decay. Paralyze Field is not here: it freezes whoever crosses it, and there is
//! no freeze mechanic until the Paralyze spell brings one.

use super::*;
use openshard_state::components::{Field, FieldKind, Skills, FIELD_HEIGHT};
use openshard_state::DamageType;

/// The skill a field's duration scales from — Magery, id 25.
const MAGERY_SKILL: u8 = 25;
/// Fire Field's damage per pulse (pre-AoS, era 1).
const FIRE_FIELD_DAMAGE: u16 = 2;
/// Poison Field applies the regular (lowest) poison — the deadlier levels want the
/// Poisoning skill, a later refinement.
const POISON_FIELD_LEVEL: u8 = 0;

impl World {
    /// Lay a field at the aimed spot: a row of tiles perpendicular to the line of
    /// fire, each a ground entity drawn like a dropped item and carrying a
    /// [`Field`]. A wall kind also registers each tile in the obstruction index.
    pub(super) fn lay_field(&mut self, caster: EntityId, kind: FieldKind, target: Point) {
        let Some(caster_serial) = self.state.registry.serial_of(caster) else {
            return;
        };
        let facet = self.state.facet_of(caster);
        let from = self
            .state
            .registry
            .get::<Position>(caster)
            .map_or(target, |p| p.0);
        let magery = self
            .state
            .registry
            .get::<Skills>(caster)
            .map_or(0, |s| s.get(MAGERY_SKILL));
        let now = self.state.ticks;
        let expires_at = now + field_duration_ticks(kind, magery);
        let next_pulse = now + field_pulse_ticks(kind);
        let east_to_west = field_east_to_west(from, target);
        // Five tiles for a hazard, three for a wall — centre on the aimed spot,
        // running along the axis perpendicular to the line of fire.
        let reach: i32 = if matches!(kind, FieldKind::Stone) {
            1
        } else {
            2
        };
        let graphic = field_graphic(kind, east_to_west);
        let field = Field {
            kind,
            caster: caster_serial,
            next_pulse,
            expires_at,
            blocks: kind.blocks(),
        };
        for offset in -reach..=reach {
            let (x, y) = if east_to_west {
                (i32::from(target.x) + offset, i32::from(target.y))
            } else {
                (i32::from(target.x), i32::from(target.y) + offset)
            };
            if !(0..=i32::from(u16::MAX)).contains(&x) || !(0..=i32::from(u16::MAX)).contains(&y) {
                continue; // off the world edge
            }
            let pos = Point::new(x as u16, y as u16, target.z);
            self.spawn_field_tile(graphic, pos, facet, field);
        }
    }

    /// Put one field tile on the ground — the drawn-item path (`Graphic`, `Position`,
    /// `Facet`, the sector grid, `reveal`), plus the [`Field`] and, for a wall, an
    /// obstruction. No `Decays`: a field owns its own lifetime.
    fn spawn_field_tile(&mut self, graphic: u16, pos: Point, facet: u8, field: Field) {
        let Ok((entity, _serial)) = self.state.registry.spawn_with_serial(SerialKind::Item) else {
            warn!("out of item serials; not laying a field tile");
            return;
        };
        self.state.registry.insert(
            entity,
            Graphic {
                id: graphic,
                hue: 0,
            },
        );
        self.state.registry.insert(entity, Position(pos));
        self.state.registry.insert(entity, Facet(facet));
        self.state.registry.insert(entity, field);
        self.state
            .facet_state_mut(facet)
            .sectors
            .insert(entity, pos);
        if field.blocks {
            self.state.facet_state_mut(facet).obstructions.block(
                pos.x,
                pos.y,
                entity,
                false, // a wall, not a door: nothing routes through it
                pos.z,
                FIELD_HEIGHT,
            );
        }
        self.state.reveal(entity);
    }

    /// Pulse the fields that harm and lift the fields whose time is up — the tick
    /// counter throughout, like decay and poison. Runs before `reap`, so a field
    /// kill lays its corpse the same tick.
    pub(super) fn field_tick(&mut self) {
        let now = self.state.ticks;
        // Pulse the hazards: harm every mobile standing on a due tile.
        let due: Vec<(EntityId, FieldKind, Serial)> = self
            .state
            .registry
            .query::<Field>()
            .filter(|(_, field)| field.kind.pulses() && now >= field.next_pulse)
            .map(|(entity, field)| (entity, field.kind, field.caster))
            .collect();
        for (entity, kind, caster) in due {
            let Some(&Position(pos)) = self.state.registry.get::<Position>(entity) else {
                continue;
            };
            let facet = self.state.facet_of(entity);
            // Range 0 is the tile itself (Chebyshev); a field only harms who stands on it.
            let victims: Vec<u32> = self
                .state
                .facet_state(facet)
                .sectors
                .nearby(pos, 0)
                .filter(|(entity, _)| self.state.registry.has::<Body>(*entity))
                .filter_map(|(entity, _)| self.state.registry.serial_of(entity).map(|s| s.raw()))
                .collect();
            for victim in victims {
                match kind {
                    FieldKind::Fire => combat::damage(
                        &mut self.state,
                        victim,
                        FIRE_FIELD_DAMAGE,
                        DamageType::Fire,
                        Some(caster),
                    ),
                    FieldKind::Poison => {
                        combat::apply_poison(&mut self.state, victim, POISON_FIELD_LEVEL, now);
                    }
                    FieldKind::Energy | FieldKind::Stone => {}
                }
            }
            // Reschedule, if the tile did not vanish under the blow.
            if let Some(mut field) = self.state.registry.get::<Field>(entity).copied() {
                field.next_pulse = now + field_pulse_ticks(kind);
                self.state.registry.insert(entity, field);
            }
        }
        // Lift the fields whose time is up.
        let expired: Vec<EntityId> = self
            .state
            .registry
            .query::<Field>()
            .filter(|(_, field)| now >= field.expires_at)
            .map(|(entity, _)| entity)
            .collect();
        for entity in expired {
            self.remove_field(entity);
        }
    }

    /// Take a field tile off the world — free its obstruction if it was a wall,
    /// forget it on every screen (`0x1D`), off the sector grid, out of the registry.
    /// The `clear_decorations` shape.
    fn remove_field(&mut self, entity: EntityId) {
        let Some(serial) = self.state.registry.serial_of(entity) else {
            return;
        };
        let facet = self.state.facet_of(entity);
        if let Some(&Position(at)) = self.state.registry.get::<Position>(entity) {
            if self
                .state
                .registry
                .get::<Field>(entity)
                .is_some_and(|field| field.blocks)
            {
                self.state
                    .facet_state_mut(facet)
                    .obstructions
                    .unblock(at.x, at.y, entity);
            }
        }
        for watcher in self.state.watchers_of(entity) {
            self.state.forget(watcher, entity, serial);
        }
        self.state.facet_state_mut(facet).sectors.remove(entity);
        self.state.registry.despawn(entity);
    }
}

/// Whether a field's row runs along the X axis (east–west) rather than Y, from the
/// isometric projection of the caster→target vector — ServUO's `eastToWest`, so the
/// row lies perpendicular to the line of fire.
fn field_east_to_west(from: Point, to: Point) -> bool {
    let dx = i32::from(from.x) - i32::from(to.x);
    let dy = i32::from(from.y) - i32::from(to.y);
    let rx = (dx - dy) * 44;
    let ry = (dx + dy) * 44;
    // ServUO's cascade reduces to: the row runs east–west exactly when the two
    // projected signs differ.
    (rx >= 0) != (ry >= 0)
}

/// The tile graphic for a field kind and orientation — ServUO's per-field art, an
/// east–west and a north–south variant (Wall of Stone is one graphic either way).
fn field_graphic(kind: FieldKind, east_to_west: bool) -> u16 {
    match kind {
        FieldKind::Fire => {
            if east_to_west {
                0x398C
            } else {
                0x3996
            }
        }
        FieldKind::Poison => {
            if east_to_west {
                0x3915
            } else {
                0x3922
            }
        }
        FieldKind::Energy => {
            if east_to_west {
                0x3946
            } else {
                0x3956
            }
        }
        FieldKind::Stone => 0x0082,
    }
}

/// How long a field lasts, in ticks — Magery-scaled (in tenths, grandmaster
/// `1000`), ServUO's pre-AoS shape.
fn field_duration_ticks(kind: FieldKind, magery: u16) -> u64 {
    let m = u64::from(magery);
    let seconds = match kind {
        FieldKind::Fire => 4 + m / 20,          // 4s + mag/2, grandmaster ~54s
        FieldKind::Poison => 3 + m / 25,        // 3s + mag*0.4, grandmaster ~43s
        FieldKind::Energy => 2 + m * 28 / 1000, // 2s + mag*0.28, grandmaster ~30s
        FieldKind::Stone => 10,                 // fixed
    };
    seconds * TICKS_PER_SECOND
}

/// The ticks between a hazard field's pulses (`0` for a wall, which never pulses).
fn field_pulse_ticks(kind: FieldKind) -> u64 {
    match kind {
        FieldKind::Fire => TICKS_PER_SECOND,           // 1s
        FieldKind::Poison => TICKS_PER_SECOND * 3 / 2, // 1.5s
        FieldKind::Energy | FieldKind::Stone => 0,
    }
}
