//! The skill window's button: what happens when a player clicks a skill.
//!
//! Ported from ServUO's `Skills.UseSkill` (`Server/Skills.cs`). The client sends
//! `0x12` with command type `0x24` and the skill's index; this decides whether
//! the use is allowed at all, and hands the ones that are on to whoever owns the
//! skill's effect.
//!
//! **The default for a bare skill is nothing, and saying so.** Thirty-five of the
//! fifty-eight are not usable this way — Tactics is passive, Mining wants a
//! pickaxe, Magery wants a spellbook — and the client has a line for exactly that
//! case, cliloc 500014. It is not an error and it is not a gap: it is the answer.

use openshard_entities::EntityId;
use openshard_protocol::serial::Serial;
use openshard_protocol::wire::{
    ClilocId,
    RawSkillId,
};
use openshard_state::WorldState;
use openshard_state::components::{
    Casting,
    Ghost,
    SkillCooldown,
};
use openshard_state::skill::Skill;

/// "That skill cannot be used directly." — the client's own line for a skill with
/// no button behaviour.
const CANNOT_USE_DIRECTLY: ClilocId = ClilocId(500_014);

/// "You must wait a few moments to use another skill." — ServUO's
/// `Mobile.SendSkillMessage`.
const MUST_WAIT: ClilocId = ClilocId(500_118);

/// A player asked to use a skill from the window.
///
/// Emitted only for a skill that *can* be used that way, and only once every gate
/// has passed — so a reader knows the mobile is alive, off cooldown, and free to
/// act. The engine runs the skills it knows; anything that wants to add one, or
/// answer a skill the engine leaves alone, reads this.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SkillRequested {
    /// Who asked.
    pub entity: EntityId,
    /// Their wire identity.
    pub serial: Serial,
    /// Which skill.
    pub skill:  Skill,
}

/// How long a use of a skill with no handler of its own holds the button, in
/// ticks — ServUO's handlers all return a delay, and the shortest of them is a
/// second. Until each skill has its own, one second is the floor that stops a
/// held button from flooding the tick.
pub const DEFAULT_SKILL_DELAY_TICKS: u64 = openshard_state::TICKS_PER_SECOND;

/// A player pressed a skill's button. Returns whether anything was started.
///
/// The gates are ServUO's, in its order: dead mobiles do nothing at all, a use
/// inside another use's cooldown is refused out loud, and a use during a cast is
/// refused unless the skill is one of the few allowed mid-cast (Spirit Speak, and
/// only it).
pub fn use_skill_button(state: &mut WorldState, entity: EntityId, id: RawSkillId) -> bool {
    // A ghost has no hands. ServUO's `CheckAlive` says nothing either — there is
    // no message for this, the button simply does not work.
    if state.registry.has::<Ghost>(entity) {
        return false;
    }
    let Some(skill) = Skill::from_id(id.0) else {
        return false; // an id past the table: a client sending noise
    };
    let info = skill.info();
    let now = state.ticks;
    let waiting = state
        .registry
        .get::<SkillCooldown>(entity)
        .is_some_and(|cooldown| now < cooldown.until);
    // A cast in flight blocks every skill but the one ServUO marks otherwise.
    let casting = !info.use_while_casting && state.registry.has::<Casting>(entity);
    if waiting || casting {
        state.localized_message(entity, MUST_WAIT, "");
        return false;
    }
    if !info.usable {
        state.localized_message(entity, CANNOT_USE_DIRECTLY, "");
        return false;
    }
    let Some(serial) = state.registry.serial_of(entity) else {
        return false;
    };
    // The button is held for as long as the use takes, whoever ends up running it.
    // Armed *before* the event, so a handler that wants a different delay only has
    // to overwrite it — and a handler that forgets still cannot be spammed.
    state.registry.insert(
        entity,
        SkillCooldown {
            until: now + DEFAULT_SKILL_DELAY_TICKS,
        },
    );
    state.bus.send(SkillRequested {
        entity,
        serial,
        skill,
    });
    // Then the core's own behaviour. The pack has already seen the event, so this
    // is the default and not a competitor: a skill the core has no opinion on
    // simply does nothing here, having announced itself.
    crate::handlers::start(state, entity, skill);
    true
}

/// Hold a mobile's skill button for `ticks` — what a handler calls when its use
/// takes longer than the default.
pub fn set_skill_delay(state: &mut WorldState, entity: EntityId, ticks: u64) {
    let until = state.ticks + ticks;
    state.registry.insert(entity, SkillCooldown { until });
}
