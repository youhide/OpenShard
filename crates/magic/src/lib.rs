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
use openshard_items::{count_in_container, take_from_container};
use openshard_state::components::{stat_shift, Hitpoints, Mana, StatMod, StatMods, Stats};
use openshard_state::WorldState;

mod spells;
pub use spells::{
    cast_delay_ticks, difficulty, info, mana, SpellEffect, SpellInfo, SpellTarget, AREA_RADIUS,
    MAGERY,
};

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

/// Everything a cast needs — a plain bundle, so [`cast_spell`] takes one argument
/// and the reagents can ride along by reference.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cast<'a> {
    /// The caster's serial.
    pub serial: u32,
    /// Which spell, by id.
    pub spell: u16,
    /// The target's serial, or zero for a spell that needs none.
    pub target: u32,
    /// The mana it costs.
    pub mana: u16,
    /// The casting difficulty, 0–100.
    pub difficulty: u16,
    /// The skill it rolls (Magery).
    pub skill: u8,
    /// The container reagents come out of, or zero for a spell that needs none.
    pub pack: u32,
    /// The reagents the spell consumes, as `(graphic, count)`.
    pub reagents: &'a [(u16, u16)],
}

/// Cast a spell: check the mana and reagents, spend them, roll the skill, and
/// announce it.
///
/// Two gates before anything is spent, both fizzling the spell without cost if
/// they fail: the caster must have the mana, and its pack must hold every
/// reagent. Reagents are all-or-nothing across the whole list — a spell short one
/// of five consumes none of them — checked first, then consumed once mana is also
/// known good, so a fizzle never eats half a reagent list or the mana.
pub fn cast_spell(state: &mut WorldState, cast: Cast<'_>) {
    let Cast {
        serial,
        spell,
        target,
        mana,
        difficulty,
        skill,
        pack,
        reagents,
    } = cast;
    let Some(serial) = Serial::new(serial) else {
        return;
    };
    let Some(caster) = state.registry.entity_of(serial) else {
        return;
    };

    let fizzle = |state: &mut WorldState| {
        state.bus.send(SpellCast {
            caster,
            serial,
            spell,
            target,
            success: false,
        });
    };

    let have = state.registry.get::<Mana>(caster).map_or(0, |m| m.current);
    // Not enough mana, or the pack is short a reagent — the spell fizzles, and
    // nothing is spent either way.
    if have < mana || !has_reagents(state, pack, reagents) {
        fizzle(state);
        return;
    }

    consume_reagents(state, pack, reagents);
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

/// The core cast path's pay-and-roll: check the mana and reagents, spend them,
/// and roll the casting skill — returning whether the roll passed, or `None` if
/// the spell fizzled short of mana or a reagent (nothing is spent). Unlike
/// [`cast_spell`] it emits no event: the *world* emits [`SpellCast`] once it
/// knows the target — which a targeted spell learns only after the cast — and
/// applies the core effect there.
pub fn pay_and_roll(
    state: &mut WorldState,
    caster: EntityId,
    mana: u16,
    difficulty: u16,
    skill: u8,
    pack: u32,
    reagents: &[(u16, u16)],
) -> Option<bool> {
    let have = state.registry.get::<Mana>(caster).map_or(0, |m| m.current);
    if have < mana || !has_reagents(state, pack, reagents) {
        return None;
    }
    consume_reagents(state, pack, reagents);
    if let Some(&Mana { current, max }) = state.registry.get::<Mana>(caster) {
        state.registry.insert(
            caster,
            Mana {
                current: current - mana,
                max,
            },
        );
    }
    Some(openshard_skills::roll_skill(
        state, caster, skill, difficulty,
    ))
}

/// Whether `pack` holds every reagent the spell needs. A zero pack with any
/// reagent required is short by definition.
fn has_reagents(state: &WorldState, pack: u32, reagents: &[(u16, u16)]) -> bool {
    let Some(pack) = Serial::new(pack) else {
        return reagents.is_empty();
    };
    reagents
        .iter()
        .all(|&(graphic, count)| count_in_container(state, pack, graphic) >= u32::from(count))
}

/// Take every reagent out of the pack. Only called once [`has_reagents`] has
/// confirmed they are all there, so each take succeeds.
fn consume_reagents(state: &mut WorldState, pack: u32, reagents: &[(u16, u16)]) {
    if let Some(pack) = Serial::new(pack) {
        for &(graphic, count) in reagents {
            take_from_container(state, pack, graphic, count);
        }
    }
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

/// A stat shifted by `delta`, floored at 1 and capped at the type maximum.
///
/// The floor keeps a debuff from driving a stat (or a derived maximum) to zero,
/// where a zero max-hits would read as dead. It costs exactness only for a mobile
/// whose stat is already smaller than the modifier — no real character is — so a
/// reversal of that clamped shift restores the base within that rounding.
fn apply_delta(value: u16, delta: i16) -> u16 {
    (i32::from(value) + i32::from(delta)).clamp(1, i32::from(u16::MAX)) as u16
}

/// Fold one stat modifier into (or, with a negated `offset`, back out of) a
/// mobile's live stats and the maxima that hang off them.
///
/// Strength moves the hit-points cap, intelligence the mana cap; dexterity's
/// stamina pool has no component yet, so it moves only the stat. A shrinking
/// maximum clamps the current pool down with it; a growing one leaves the current
/// where it is, to be healed or regenerated into.
fn shift_stats(state: &mut WorldState, entity: EntityId, kind: u8, offset: i16) {
    let (ds, dd, di) = stat_shift(kind, offset);
    if let Some(&Stats {
        strength,
        dexterity,
        intelligence,
    }) = state.registry.get::<Stats>(entity)
    {
        state.registry.insert(
            entity,
            Stats {
                strength: apply_delta(strength, ds),
                dexterity: apply_delta(dexterity, dd),
                intelligence: apply_delta(intelligence, di),
            },
        );
    }
    if ds != 0 {
        if let Some(&Hitpoints { current, max }) = state.registry.get::<Hitpoints>(entity) {
            let max = apply_delta(max, ds);
            state.registry.insert(
                entity,
                Hitpoints {
                    current: current.min(max),
                    max,
                },
            );
        }
    }
    if di != 0 {
        if let Some(&Mana { current, max }) = state.registry.get::<Mana>(entity) {
            let max = apply_delta(max, di);
            state.registry.insert(
                entity,
                Mana {
                    current: current.min(max),
                    max,
                },
            );
        }
    }
}

/// Lay a timed stat modifier on a mobile — the Bless/Curse family.
///
/// The `offset` is signed (a debuff arrives negative), and `expires_at` is the
/// tick it lifts. Re-casting the same `kind` refreshes it: the old entry is first
/// backed out cleanly, then the new one applied, so a Bless recast never stacks a
/// second bonus. The shift folds into the live [`Stats`] at once; the ledger entry
/// remembers how to give it back.
pub fn apply_stat_buff(
    state: &mut WorldState,
    serial: u32,
    kind: u8,
    offset: i16,
    expires_at: u64,
) {
    let Some(entity) = Serial::new(serial).and_then(|s| state.registry.entity_of(s)) else {
        return;
    };
    let mut mods = state
        .registry
        .get::<StatMods>(entity)
        .cloned()
        .unwrap_or_default();
    // A recast backs out the standing entry of this kind before re-applying, so
    // the bonus refreshes rather than doubling.
    if let Some(pos) = mods.active.iter().position(|m| m.kind == kind) {
        let old = mods.active.remove(pos);
        shift_stats(state, entity, old.kind, -old.offset);
    }
    shift_stats(state, entity, kind, offset);
    mods.active.push(StatMod {
        kind,
        offset,
        expires_at,
    });
    state.registry.insert(entity, mods);
}

/// Lift every stat modifier whose tick has come, backing its shift out of the
/// mobile it worked through. Returns whom it touched, so the caller can redraw a
/// player's status bar. Runs on the tick counter, so it replays.
#[must_use]
pub fn expire_buffs(state: &mut WorldState, now: u64) -> Vec<EntityId> {
    let ready: Vec<EntityId> = state
        .registry
        .query::<StatMods>()
        .filter(|(_, mods)| mods.active.iter().any(|m| now >= m.expires_at))
        .map(|(entity, _)| entity)
        .collect();
    for &entity in &ready {
        let Some(mods) = state.registry.get::<StatMods>(entity).cloned() else {
            continue;
        };
        let (expired, kept): (Vec<StatMod>, Vec<StatMod>) =
            mods.active.into_iter().partition(|m| now >= m.expires_at);
        for m in expired {
            shift_stats(state, entity, m.kind, -m.offset);
        }
        if kept.is_empty() {
            state.registry.remove::<StatMods>(entity);
        } else {
            state.registry.insert(entity, StatMods { active: kept });
        }
    }
    ready
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
