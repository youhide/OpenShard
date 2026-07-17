//! Spells, casting, mana, and healing.
//!
//! A gameplay system in its own crate. [`cast_spell`] is the gate every spell
//! passes: it checks the mana, rolls the casting skill (through the very
//! [`roll_skill`](openshard_skills::roll_skill) a mined ore uses, so casting
//! trains Magery), spends the mana, and emits [`SpellCast`]. What the spell
//! *does* — a fireball's damage, a summon's creature — is a script's to decide,
//! read off that event; the casting machinery knows nothing of effects.
//!
//! [`heal`] mends toward the maximum and redraws the bar; [`regen_mana`] trickles
//! mana back on the tick counter, so it needs no clock and stays replayable.

use openshard_entities::{EntityId, Serial};
use openshard_state::components::{Hitpoints, Mana};
use openshard_state::WorldState;

/// How often, in ticks, a mobile with spent mana gets a point back.
pub const MANA_REGEN_TICKS: u64 = 60;

/// A spell was cast: the mana was paid and the skill rolled. What the spell
/// *does* is a script's to decide — this only says who cast what at whom, and
/// whether it took. A fireball's damage, a heal's mending, a summon's creature
/// all hang off this event, none of them known to the casting machinery.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SpellCast {
    /// The caster.
    pub caster: EntityId,
    /// Its wire identity.
    pub serial: Serial,
    /// Which spell, by id.
    pub spell: u16,
    /// The target's serial, or zero for a spell that needs none.
    pub target: u32,
    /// Whether the cast succeeded (mana paid and the skill check passed).
    pub success: bool,
}

/// Cast a spell: pay the mana, roll the skill, announce it.
pub fn cast_spell(
    state: &mut WorldState,
    serial: u32,
    spell: u16,
    target: u32,
    mana: u16,
    difficulty: u16,
    skill: u8,
) {
    let Some(serial) = Serial::new(serial) else {
        return;
    };
    let Some(caster) = state.registry.entity_of(serial) else {
        return;
    };
    let have = state.registry.get::<Mana>(caster).map_or(0, |m| m.current);

    // Not enough mana to pay for it — the spell fizzles, and nothing is spent.
    if have < mana {
        state.bus.send(SpellCast {
            caster,
            serial,
            spell,
            target,
            success: false,
        });
        return;
    }
    if let Some(&Mana { current, max }) = state.registry.get::<Mana>(caster) {
        state.registry.insert(
            caster,
            Mana {
                current: current - mana,
                max,
            },
        );
    }
    let success = openshard_skills::roll_skill(state, caster, skill, difficulty);
    state.bus.send(SpellCast {
        caster,
        serial,
        spell,
        target,
        success,
    });
}

/// Mend a mobile up toward its maximum, and redraw the bar for it and everyone
/// watching.
pub fn heal(state: &mut WorldState, serial: u32, amount: u16) {
    let Some(serial) = Serial::new(serial) else {
        return;
    };
    let Some(entity) = state.registry.entity_of(serial) else {
        return;
    };
    let Some(&Hitpoints { current, max }) = state.registry.get::<Hitpoints>(entity) else {
        return;
    };
    let healed = current.saturating_add(amount).min(max);
    if healed == current {
        return;
    }
    state.registry.insert(
        entity,
        Hitpoints {
            current: healed,
            max,
        },
    );
    state.broadcast_health(entity);
}

/// Trickle mana back for everyone who has any, one point each regen tick. Runs
/// against the tick counter, so it needs no clock and stays replayable.
pub fn regen_mana(state: &mut WorldState) {
    if !state.ticks.is_multiple_of(MANA_REGEN_TICKS) {
        return;
    }
    let thirsty: Vec<EntityId> = state
        .registry
        .query::<Mana>()
        .filter(|(_, mana)| mana.current < mana.max)
        .map(|(entity, _)| entity)
        .collect();
    for entity in thirsty {
        if let Some(&Mana { current, max }) = state.registry.get::<Mana>(entity) {
            state.registry.insert(
                entity,
                Mana {
                    current: (current + 1).min(max),
                    max,
                },
            );
        }
    }
}
