//! Stamina expenditure and passive hit-point and stamina recovery.

use openshard_entities::EntityId;
use openshard_state::WorldState;
use openshard_state::components::{Ghost, Hitpoints, Poisoned, Stamina, Steps};

/// How far over its carry cap a mobile may be before fatigue begins.
pub const OVERLOAD_ALLOWANCE: u16 = 4;
/// On-foot steps between baseline stamina expenditure.
pub const STEPS_PER_STAMINA: u32 = 16;
/// Mounted steps between baseline stamina expenditure.
pub const MOUNTED_STEPS_PER_STAMINA: u32 = 48;
const WINDED_PERCENT: u16 = 10;

/// Spend what one step costs in stamina, returning a refusal message if unable.
///
/// A mobile without a stamina pool never tires. Overweight, low-stamina, and
/// periodic walking costs follow the same ordered calculation as before.
pub fn spend_step_stamina(
    state: &mut WorldState,
    mobile: EntityId,
    running: bool,
    mounted: bool,
    over_weight: u16,
) -> Option<&'static str> {
    let &Stamina { current, max } = state.registry.get::<Stamina>(mobile)?;
    let mut left = current;
    let spend = |cost: u16, left: &mut u16| *left = left.saturating_sub(cost);

    if over_weight > 0 {
        let mut loss = 5 + over_weight / 25;
        if mounted {
            loss /= 3;
        }
        if running {
            loss *= 2;
        }
        spend(loss, &mut left);
        if left == 0 {
            store_stamina(state, mobile, left, max);
            return Some("You are too fatigued to move, because you are carrying too much weight!");
        }
    }
    if max > 0 && (u32::from(left) * 100 / u32::from(max)) < u32::from(WINDED_PERCENT) {
        spend(1, &mut left);
    }
    if left == 0 {
        store_stamina(state, mobile, left, max);
        return Some(if mounted {
            "Your mount is too fatigued to move."
        } else {
            "You are too fatigued to move."
        });
    }
    let every = if mounted {
        MOUNTED_STEPS_PER_STAMINA
    } else {
        STEPS_PER_STAMINA
    };
    let steps = state.registry.get::<Steps>(mobile).map_or(0, |s| s.0) + 1;
    state.registry.insert(mobile, Steps(steps));
    if steps.is_multiple_of(every) {
        spend(1, &mut left);
    }
    store_stamina(state, mobile, left, max);
    None
}

fn store_stamina(state: &mut WorldState, mobile: EntityId, current: u16, max: u16) {
    state.registry.insert(mobile, Stamina { current, max });
}

/// Ticks between one-point health regeneration pulses.
pub const HITS_REGEN_TICKS: u64 = 220;

/// Heal living, non-poisoned wounded mobiles once per regeneration pulse.
pub fn regen_hits(state: &mut WorldState) {
    if !state.ticks.is_multiple_of(HITS_REGEN_TICKS) {
        return;
    }
    let wounded: Vec<EntityId> = state
        .registry
        .query::<Hitpoints>()
        .filter(|(_, hits)| hits.current > 0 && hits.current < hits.max)
        .map(|(entity, _)| entity)
        .filter(|&entity| !state.registry.has::<Poisoned>(entity) && !state.registry.has::<Ghost>(entity))
        .collect();
    for entity in wounded {
        if let Some(&Hitpoints { current, max }) = state.registry.get::<Hitpoints>(entity) {
            state.registry.insert(
                entity,
                Hitpoints {
                    current: (current + 1).min(max),
                    max,
                },
            );
            state.broadcast_health(entity);
        }
    }
}

/// Ticks between one-point stamina regeneration pulses.
pub const STAMINA_REGEN_TICKS: u64 = 30;

/// Restore one stamina point to each winded mobile on a regeneration pulse.
pub fn regen_stamina(state: &mut WorldState) {
    if !state.ticks.is_multiple_of(STAMINA_REGEN_TICKS) {
        return;
    }
    let winded: Vec<EntityId> = state
        .registry
        .query::<Stamina>()
        .filter(|(_, stamina)| stamina.current < stamina.max)
        .map(|(entity, _)| entity)
        .collect();
    for entity in winded {
        if let Some(&Stamina { current, max }) = state.registry.get::<Stamina>(entity) {
            state.registry.insert(
                entity,
                Stamina {
                    current: (current + 1).min(max),
                    max,
                },
            );
        }
    }
}
