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

use openshard_entities::EntityId;
use openshard_items::{
    count_in_container,
    take_from_container,
};
use openshard_protocol::casting::SpellId;
use openshard_protocol::serial::Serial;
use openshard_protocol::wire::Graphic;
use openshard_skills::SkillBand;
use openshard_state::components::{
    BehaviourBuff,
    BehaviourBuffKind,
    BehaviourBuffs,
    Frozen,
    Hitpoints,
    Mana,
    Meditating,
    Stamina,
    StatEffectKind,
    StatMod,
    StatMods,
    Stats,
    stat_shift,
};
use openshard_state::{
    Skill,
    TICKS_PER_SECOND,
    WorldState,
};

mod spells;
pub use spells::{
    AREA_RADIUS,
    MAGERY,
    MAGERY_SKILL,
    SpellCircle,
    SpellEffect,
    SpellInfo,
    SpellTarget,
    cast_delay_ticks,
    cast_skills,
    info,
    mana,
};

mod resist;
pub use resist::{
    RESIST_SKILL,
    RESISTED_MESSAGE,
    check_resisted,
    resist_chance,
    resisted,
};

mod travel;
pub use travel::{
    PUBLIC_MOONGATES,
    PublicGate,
    TravelKind,
    describe,
    destination_of,
    may_travel,
    public_gate_at,
    standing_at,
};

/// What intelligence a mobile with no stat sheet regenerates as if it had — the
/// same convention the status bar and the lore skills use for a missing stat.
const DEFAULT_INTELLIGENCE: u16 = 100;

/// Meditation's skill id, which sets the mana regen rate.
const MEDITATION_SKILL: Skill = Skill::Meditation;

/// A spell was cast: the mana was paid and the skill rolled. What the spell
/// *does* is a script's to decide — this only says who cast what at whom, and
/// whether it took. A fireball's damage, a heal's mending, a summon's creature
/// all hang off this event, none of them known to the casting machinery.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SpellCast {
    /// The caster.
    pub caster:  EntityId,
    /// Its wire identity.
    pub serial:  Serial,
    /// Which spell, by id.
    pub spell:   SpellId,
    /// The target, or `None` for a spell that needs none.
    pub target:  Option<Serial>,
    /// Whether the cast succeeded (mana paid and the skill check passed).
    pub success: bool,
}

/// A skill identifier carried by a pending cast.
///
/// This is deliberately distinct from other wire-sized values: a cast's
/// skill is resolved as a [`Skill`] at the cast boundary, and must not be
/// confused with mana, a spell id, or an arbitrary byte.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(transparent)]
pub struct SkillId(u8);

impl SkillId {
    /// Wrap the raw skill id received from the command queue.
    #[must_use]
    pub const fn new(raw: u8) -> Self {
        Self(raw)
    }

    /// Return the protocol representation for validation at resolution.
    #[must_use]
    pub const fn raw(self) -> u8 {
        self.0
    }
}

/// Everything a cast needs — a plain bundle, so [`cast_spell`] takes one argument
/// and the reagents can ride along by reference.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cast<'a> {
    /// The caster.
    pub serial:     Serial,
    /// Which spell, by id.
    pub spell:      SpellId,
    /// The target, or `None` for a spell that needs none.
    pub target:     Option<Serial>,
    /// The mana it costs.
    pub mana:       u16,
    /// The skill band it is cast against.
    pub skill_band: SkillBand,
    /// The skill it rolls (Magery).
    pub skill:      SkillId,
    /// The container reagents come out of, or `None` for a spell that needs none.
    pub pack:       Option<Serial>,
    /// The reagents the spell consumes, as `(graphic, count)`.
    pub reagents:   &'a [(Graphic, u16)],
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
        skill_band,
        skill,
        pack,
        reagents,
    } = cast;
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

    // `skill` crossed the command queue unchecked (N3's "the queue is a
    // delivery, not a checkpoint"); this is the seam that owns the roll, so this
    // is where an id past the table is refused — the same shape
    // `skills::set_skill` uses.
    let Some(skill) = Skill::from_id(skill.raw()) else {
        fizzle(state);
        return;
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
        state.set_mana(
            caster,
            Mana {
                current: current - mana,
                max,
            },
        );
    }
    let success = openshard_skills::roll_skill_band(state, caster, skill, skill_band);
    state.bus.send(SpellCast {
        caster,
        serial,
        spell,
        target,
        success,
    });
}

/// The core cast path's pay-and-roll — Sphere's confirmed model: check that the
/// mana and reagents are *there* (fizzling short of them spends nothing), roll the
/// casting skill, and only *then* spend, at resolution once success is known.
/// Returns whether the roll passed, or `None` if the spell could not be cast for
/// want of mana or a reagent.
///
/// A successful cast always spends; a *failed* one spends mana only if
/// `mana_loss_on_fail` and reagents only if `reagent_loss_on_fail` — Sphere's
/// `ManaLossFail`/`ReagentLossFail`, the UO/ServUO original being both on. Reagents
/// are toggled off entirely by the caller passing an empty list. Unlike
/// [`cast_spell`] it emits no event: the *world* emits [`SpellCast`] once it knows
/// the target — which a targeted spell learns only after the cast — and applies
/// the core effect there.
// Each argument is a distinct cast input; a struct would only move the list up.
#[allow(clippy::too_many_arguments)]
pub fn pay_and_roll(
    state: &mut WorldState,
    caster: EntityId,
    mana: u16,
    skill_band: SkillBand,
    skill: Skill,
    pack: Option<Serial>,
    reagents: &[(Graphic, u16)],
    mana_loss_on_fail: bool,
    reagent_loss_on_fail: bool,
) -> Option<bool> {
    let have = state.registry.get::<Mana>(caster).map_or(0, |m| m.current);
    if have < mana || !has_reagents(state, pack, reagents) {
        return None; // cannot cast — nothing is spent
    }
    // Roll before spending, so we know success in time to honour the loss flags.
    let success = openshard_skills::roll_skill_band(state, caster, skill, skill_band);
    if success || mana_loss_on_fail {
        if let Some(&Mana { current, max }) = state.registry.get::<Mana>(caster) {
            state.set_mana(
                caster,
                Mana {
                    current: current.saturating_sub(mana),
                    max,
                },
            );
        }
    }
    if success || reagent_loss_on_fail {
        consume_reagents(state, pack, reagents);
    }
    Some(success)
}

/// Whether `pack` holds every reagent the spell needs. A zero pack with any
/// reagent required is short by definition.
fn has_reagents(state: &WorldState, pack: Option<Serial>, reagents: &[(Graphic, u16)]) -> bool {
    let Some(pack) = pack else {
        return reagents.is_empty();
    };
    reagents
        .iter()
        .all(|&(graphic, count)| count_in_container(state, pack, graphic) >= u32::from(count))
}

/// Take every reagent out of the pack. Only called once [`has_reagents`] has
/// confirmed they are all there, so each take succeeds.
fn consume_reagents(state: &mut WorldState, pack: Option<Serial>, reagents: &[(Graphic, u16)]) {
    if let Some(pack) = pack {
        for &(graphic, count) in reagents {
            take_from_container(state, pack, graphic, u32::from(count));
        }
    }
}

/// Mend a mobile up toward its maximum, and redraw the bar for it and everyone
/// watching.
pub fn heal(state: &mut WorldState, serial: Serial, amount: u16) {
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
    state.registry.insert(entity, Hitpoints { current: healed, max });
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
/// Strength moves the hit-points cap, intelligence the mana cap, and dexterity
/// the stamina cap. A shrinking maximum clamps the current pool down with it; a
/// growing one leaves the current where it is, to be healed or regenerated into.
fn shift_stats(state: &mut WorldState, entity: EntityId, kind: StatEffectKind, offset: i16) {
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
                strength:     apply_delta(strength, ds),
                dexterity:    apply_delta(dexterity, dd),
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
            state.set_mana(
                entity,
                Mana {
                    current: current.min(max),
                    max,
                },
            );
        }
    }
    if dd != 0 {
        if let Some(&Stamina { current, max }) = state.registry.get::<Stamina>(entity) {
            let max = apply_delta(max, dd);
            state.registry.insert(
                entity,
                Stamina {
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
    serial: Serial,
    kind: StatEffectKind,
    offset: i16,
    expires_at: openshard_state::WorldTick,
) {
    let Some(entity) = state.registry.entity_of(serial) else {
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
pub fn expire_buffs(state: &mut WorldState, now: openshard_state::WorldTick) -> Vec<EntityId> {
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

/// Put a timed behaviour buff on a mobile — Night Sight, Protection, Reactive
/// Armor, Magic Reflection. The [`StatMods`] pattern for effects that change a
/// behaviour, not a stat: nothing folds into a number, so a recast of the same
/// `kind` just replaces its entry (refresh, never stack), and there is nothing to
/// back out — expiry only stops the buff being read.
pub fn apply_behaviour_buff(
    state: &mut WorldState,
    serial: Serial,
    kind: BehaviourBuffKind,
    amount: i16,
    expires_at: openshard_state::WorldTick,
) {
    let Some(entity) = state.registry.entity_of(serial) else {
        return;
    };
    let mut buffs = state
        .registry
        .get::<BehaviourBuffs>(entity)
        .cloned()
        .unwrap_or_default();
    buffs.active.retain(|b| b.kind != kind);
    buffs.active.push(BehaviourBuff {
        kind,
        amount,
        expires_at,
    });
    state.registry.insert(entity, buffs);
}

/// Lift every behaviour buff whose tick has come, returning `(entity, kind)` for
/// each so the caller can react — Night Sight, say, must re-send the ambient light
/// when it lifts. Runs on the tick counter, so it replays.
#[must_use]
pub fn expire_behaviour_buffs(
    state: &mut WorldState,
    now: openshard_state::WorldTick,
) -> Vec<(EntityId, BehaviourBuffKind)> {
    let ready: Vec<EntityId> = state
        .registry
        .query::<BehaviourBuffs>()
        .filter(|(_, buffs)| buffs.active.iter().any(|b| now >= b.expires_at))
        .map(|(entity, _)| entity)
        .collect();
    let mut lifted = Vec::new();
    for entity in ready {
        let Some(buffs) = state.registry.get::<BehaviourBuffs>(entity).cloned() else {
            continue;
        };
        let (expired, kept): (Vec<BehaviourBuff>, Vec<BehaviourBuff>) =
            buffs.active.into_iter().partition(|b| now >= b.expires_at);
        for b in expired {
            lifted.push((entity, b.kind));
        }
        if kept.is_empty() {
            state.registry.remove::<BehaviourBuffs>(entity);
        } else {
            state.registry.insert(entity, BehaviourBuffs { active: kept });
        }
    }
    lifted
}

/// Take one behaviour buff off a mobile before its time — Magic Reflection is
/// spent the moment it bounces a spell. Returns whether the buff was there.
pub fn consume_behaviour_buff(state: &mut WorldState, entity: EntityId, kind: BehaviourBuffKind) -> bool {
    let Some(mut buffs) = state.registry.get::<BehaviourBuffs>(entity).cloned() else {
        return false;
    };
    let before = buffs.active.len();
    buffs.active.retain(|b| b.kind != kind);
    if buffs.active.len() == before {
        return false;
    }
    if buffs.active.is_empty() {
        state.registry.remove::<BehaviourBuffs>(entity);
    } else {
        state.registry.insert(entity, buffs);
    }
    true
}

/// Read the magnitude of an active behaviour buff, if the mobile carries it — the
/// Reactive Armor reflect percent, the Protection chance. `None` when absent.
#[must_use]
pub fn behaviour_buff(state: &WorldState, entity: EntityId, kind: BehaviourBuffKind) -> Option<i16> {
    state
        .registry
        .get::<BehaviourBuffs>(entity)?
        .active
        .iter()
        .find(|b| b.kind == kind)
        .map(|b| b.amount)
}

/// Freeze a mobile in place until `until` — the Paralyze spell and Paralyze Field
/// alike. A no-op if it is already frozen, matching ServUO's `Paralyze()`: a fresh
/// cast (or a field pulse over someone already caught) does not extend the hold, so
/// a field cannot pin a target forever.
pub fn apply_paralyze(state: &mut WorldState, serial: Serial, until: openshard_state::WorldTick) {
    let Some(entity) = state.registry.entity_of(serial) else {
        return;
    };
    if state.registry.has::<Frozen>(entity) {
        return;
    }
    state.registry.insert(entity, Frozen { until });
}

/// Lift the paralysis of every mobile whose tick has come, returning whom it
/// thawed so the caller can tell a player it can move again. Runs on the tick
/// counter, so it replays.
#[must_use]
pub fn expire_frozen(state: &mut WorldState, now: openshard_state::WorldTick) -> Vec<EntityId> {
    let thawed: Vec<EntityId> = state
        .registry
        .query::<Frozen>()
        .filter(|(_, frozen)| now >= frozen.until)
        .map(|(entity, _)| entity)
        .collect();
    for &entity in &thawed {
        state.registry.remove::<Frozen>(entity);
    }
    thawed
}

/// How often a mobile gets a point of mana back, in ticks — ServUO's pre-AoS
/// `Mobile_ManaRegenRate`, in fixed point.
///
/// `medPoints = (Int + Meditation) / 2` drives a curve from seven seconds a point
/// down to three quarters of one:
///
/// ```text
/// rate = 7.0                                        medPoints <= 0
///      = 7.0 - 239·mp/2400 + 19·mp²/48000           medPoints <= 100
///      = 1.0                                        medPoints <  120
///      = 0.75                                       otherwise
/// rate += armour offset;  halved while meditating;  clamped to 0.5 ..= 7.0
/// ```
///
/// Two things make this worth reading rather than skimming. The **armour offset**
/// (`combat::armor::meditation_offset`) is added in *seconds*, so a mage in plate
/// regenerates like a warrior however much Meditation they have — that is what the
/// skill's free-hands and armour rules are all about. And the whole thing is
/// hundredths of a second throughout, then ticks, because a floating rate would
/// make the trickle drift between two replays of the same world.
///
/// A read-site derivation: nothing is stored, so taking a helmet off changes the
/// next point of mana with nothing to invalidate.
#[must_use]
pub fn mana_regen_ticks(state: &WorldState, entity: EntityId) -> u64 {
    // Hundredths of a second throughout.
    let intelligence = u32::from(
        state
            .registry
            .get::<Stats>(entity)
            .map_or(DEFAULT_INTELLIGENCE, |stats| stats.intelligence),
    );
    let meditation = u32::from(openshard_skills::skill_value(state, entity, MEDITATION_SKILL)) / 10;
    // `(Int + Meditation) * 0.5`, in tenths of a point so the halving is exact.
    let med_points = (intelligence * 10 + meditation * 10) / 2;
    let mut rate = if med_points == 0 {
        700
    } else if med_points <= 1000 {
        // 7.0 - 239·mp/2400 + 19·mp²/48000, with mp in tenths: the quadratic is
        // evaluated in hundredths of a second and cannot go negative over 0..=100.
        let mp = i64::from(med_points);
        let linear = 239 * mp * 100 / 24_000;
        let quadratic = 19 * mp * mp * 100 / 4_800_000;
        (700 - linear + quadratic).max(0) as u32
    } else if med_points < 1200 {
        100
    } else {
        75
    };
    rate += openshard_combat::armor::meditation_offset(state, entity);
    if state.registry.has::<Meditating>(entity) {
        rate /= 2;
    }
    let rate = rate.clamp(50, 700);
    // Hundredths of a second into ticks, never below one: a rate faster than the
    // tick would otherwise mean "every tick" by accident rather than by rule.
    (u64::from(rate) * TICKS_PER_SECOND / 100).max(1)
}

/// Trickle mana back, at each mobile's own rate. Runs against the tick counter, so
/// it needs no clock and stays replayable.
///
/// The rate is per mobile ([`mana_regen_ticks`]), so the cadence cannot be one
/// global modulus. It is still stateless: a mobile gets a point when the tick
/// counter divides its *own* rate, which needs no per-mobile timer to keep in step
/// and no field to save. Two mobiles with different rates simply fall on different
/// ticks.
pub fn regen_mana(state: &mut WorldState) {
    let thirsty: Vec<EntityId> = state
        .registry
        .query::<Mana>()
        .filter(|(_, mana)| mana.current < mana.max)
        .map(|(entity, _)| entity)
        .collect();
    for entity in thirsty {
        if !state.ticks.is_multiple_of(mana_regen_ticks(state, entity)) {
            continue;
        }
        if let Some(&Mana { current, max }) = state.registry.get::<Mana>(entity) {
            state.set_mana(
                entity,
                Mana {
                    current: (current + 1).min(max),
                    max,
                },
            );
        }
    }
}
