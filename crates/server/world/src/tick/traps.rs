use openshard_protocol::wire::{
    ClilocId,
    Graphic,
    SoundId,
};
use openshard_state::components::{
    Trap,
    TrapKind,
};

use super::*;

/// The sound a sprung magic or explosion trap makes — ServUO's `PlaySound(0x307)`.
const BLAST_SOUND: SoundId = SoundId(0x0307);
/// A dart trap's twang.
const DART_SOUND: SoundId = SoundId(0x0223);
/// The hiss of a poison cloud.
const POISON_SOUND: SoundId = SoundId(0x0231);
/// The explosion's flame, and the dart's — ServUO's `0x36BD` location effect.
const BLAST_GRAPHIC: Graphic = Graphic(0x36BD);
/// The green cloud a poison trap lets out.
const CLOUD_GRAPHIC: Graphic = Graphic(0x113A);
/// "You set off a trap!"
const SET_OFF_A_TRAP: ClilocId = ClilocId(502_999);
/// "Your skin blisters from the heat!"
const SKIN_BLISTERS: ClilocId = ClilocId(503_000);
/// "A dart imbeds itself in your flesh!"
const DART_IN_FLESH: ClilocId = ClilocId(502_998);
/// "You are enveloped in a noxious green cloud!"
const NOXIOUS_CLOUD: ClilocId = ClilocId(503_004);

impl World {
    /// Spring a container's trap on whoever just opened it, if it has one.
    ///
    /// A sprung trap does not bar the lid — ServUO's `ExecuteTrap` hurts you and
    /// the chest opens anyway — so this returns nothing and the click carries on.
    /// Staff open a trapped chest with their godly powers and set nothing off,
    /// which is the same `is_staff` exemption fatigue and the dead use.
    ///
    /// It lives in the tick rather than in `items` because the damage has to go
    /// through `combat::damage` — the one door — and `items` cannot depend on
    /// `combat` without closing the `skills → items → combat → skills` loop. The
    /// tick is what sits above both.
    pub(super) fn spring_trap(&mut self, opener: EntityId, container: EntityId) {
        let Some(&Trap { kind, power, level }) = self.state.registry.get::<Trap>(container) else {
            return;
        };
        if self.state.is_staff(opener) {
            self.notify_self(opener, "That is trapped, but you open it with your godly powers.");
            return;
        }
        // A trap is spent when it goes off, whatever it did.
        self.state.registry.remove::<Trap>(container);
        let Some(serial) = self.state.registry.serial_of(opener) else {
            return;
        };
        let ticks = self.state.ticks;
        // `level` scales the damage when the chest has one; otherwise the raw
        // power is the damage, which is ServUO's older shape and the one a
        // hand-placed trap uses.
        let scaled = |low: u16, high: u16, state: &mut WorldState| -> u16 {
            if level == 0 {
                return power;
            }
            let span = u32::from(high - low) + 1;
            let roll = low + u16::try_from(state.rng.below(span)).unwrap_or(0);
            roll.saturating_mul(u16::from(level))
        };

        match kind {
            TrapKind::Magic => {
                self.state.localized_message(opener, SET_OFF_A_TRAP, "");
                combat::damage(&mut self.state, serial, power, DamageType::Energy, None);
                self.state.play_sound(container, BLAST_SOUND);
                self.location_effect(container, BLAST_GRAPHIC);
            }
            TrapKind::Explosion => {
                self.state.localized_message(opener, SET_OFF_A_TRAP, "");
                let damage = scaled(10, 30, &mut self.state);
                combat::damage(&mut self.state, serial, damage, DamageType::Fire, None);
                self.state.localized_message(opener, SKIN_BLISTERS, "");
                self.state.play_sound(container, BLAST_SOUND);
                self.location_effect(container, BLAST_GRAPHIC);
            }
            TrapKind::Dart => {
                self.state.localized_message(opener, SET_OFF_A_TRAP, "");
                let damage = scaled(5, 15, &mut self.state);
                combat::damage(&mut self.state, serial, damage, DamageType::Physical, None);
                self.state.localized_message(opener, DART_IN_FLESH, "");
                self.state.play_sound(container, DART_SOUND);
            }
            TrapKind::Poison => {
                self.state.localized_message(opener, SET_OFF_A_TRAP, "");
                // ServUO poisons by the chest's level where it has one, and hits
                // for `power` and greater poison where it does not.
                let poison = if level == 0 {
                    combat::damage(&mut self.state, serial, power, DamageType::Poison, None);
                    2 // greater
                } else {
                    level.saturating_sub(1).min(4)
                };
                combat::apply_poison(
                    &mut self.state,
                    serial,
                    openshard_protocol::world::PoisonLevel::new(poison),
                    ticks,
                );
                self.state.localized_message(opener, NOXIOUS_CLOUD, "");
                self.state.play_sound(container, POISON_SOUND);
                self.location_effect(container, CLOUD_GRAPHIC);
            }
        }
    }

    /// A flash, a puff of smoke or a cloud, standing at a thing's own tile — the
    /// `0x70` in its simplest form, seen by everyone who can see the thing.
    ///
    /// Shared with the dispels, whose two outcomes are both one of these.
    pub(super) fn location_effect(&mut self, at: EntityId, graphic: Graphic) {
        let Some(&Position(spot)) = self.state.registry.get::<Position>(at) else {
            return;
        };
        let packet = openshard_protocol::feedback::GraphicalEffect {
            kind:            openshard_protocol::feedback::EffectKind::FixedXyz,
            from:            None,
            to:              None,
            art:             graphic,
            from_point:      spot,
            to_point:        spot,
            speed:           9,
            duration:        20,
            fixed_direction: true,
            explode:         false,
        };
        self.state.broadcast_packet(
            at,
            &openshard_protocol::server_packet::ServerPacket::Effect(packet),
        );
    }
}
