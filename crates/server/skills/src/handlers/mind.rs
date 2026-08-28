//! The skills a mobile turns on itself: Meditation and Spirit Speak.
//!
//! Neither raises a cursor — pressing the button *is* the whole use — so they are
//! the other half of [`super`]'s split: a skill that asks a question, and a skill
//! that simply does something to the person using it.
//!
//! Both are the pre-AoS forms. ServUO's AoS Spirit Speak is a different skill
//! wearing the same name (a channelled heal that drains nearby corpses); the
//! classic one contacts the netherworld so the living can hear the dead, which is
//! the version this engine's ghosts are built for.

use openshard_entities::EntityId;
use openshard_protocol::wire::{ClilocId, SoundId};
use openshard_state::components::{HearsGhosts, Hitpoints, Mana, Meditating, Spellbook};
use openshard_state::weapon::{LAYER_ONE_HANDED, LAYER_TWO_HANDED};
use openshard_state::{Skill, TICKS_PER_SECOND, WorldState};

use crate::check::roll_skill_band;

/// "You are busy doing something else and cannot focus."
const BUSY: ClilocId = ClilocId(501_845);
/// "The mind is strong but the body is weak." — under a tenth of your hit points.
const BODY_TOO_WEAK: ClilocId = ClilocId(501_849);
/// "You are at peace." — nothing to meditate for.
const AT_PEACE: ClilocId = ClilocId(501_846);
/// "Your hands must be free to cast spells or meditate."
const HANDS_NOT_FREE: ClilocId = ClilocId(502_626);
/// "You enter a meditative trance."
const TRANCE: ClilocId = ClilocId(501_851);
/// "You cannot focus your concentration."
const NO_FOCUS: ClilocId = ClilocId(501_850);
/// The sound a trance begins with — ServUO's `PlaySound(0xF9)`.
const TRANCE_SOUND: SoundId = SoundId(0x00F9);

/// "You contact the netherworld."
const CONTACT: ClilocId = ClilocId(502_444);
/// "You fail to contact the netherworld."
const NO_CONTACT: ClilocId = ClilocId(502_443);
/// "You feel your contact with the netherworld fading."
const CONTACT_FADING: ClilocId = ClilocId(502_445);
/// The sound of reaching the netherworld — ServUO's `PlaySound(0x24A)`.
const CONTACT_SOUND: SoundId = SoundId(0x024A);
/// The shortest contact with the dead, in seconds, however little skill was used.
const CONTACT_FLOOR_SECONDS: u64 = 15;

/// How long a use of either skill holds the button, in ticks — both of ServUO's
/// return values, kept as the constants they are because a wrong one is a skill
/// that can be held down.
const MEDITATION_DELAY: u64 = 10 * TICKS_PER_SECOND;
/// The shorter hold for a refused meditation, and for Spirit Speak.
const SHORT_DELAY: u64 = 5 * TICKS_PER_SECOND;
/// The hold for hands that are not free — ServUO's two and a half seconds.
const HANDS_DELAY: u64 = 5 * TICKS_PER_SECOND / 2;
/// Spirit Speak's own hold.
const SPIRIT_SPEAK_DELAY: u64 = TICKS_PER_SECOND;

/// Meditation: sit still and get mana back twice as fast.
///
/// ServUO's `Meditation.OnUse`, gates in its order. The trance itself is one
/// marker component and no timer — what ends it is somebody doing something, not a
/// clock, and the mana rate reads the marker where it decides rather than having
/// anything folded into it.
pub(super) fn meditation(state: &mut WorldState, actor: EntityId) {
    let skill = Skill::Meditation;
    // A cursor already up is something else being concentrated on.
    if state.has_target(actor) {
        state.localized_message(actor, BUSY, "");
        crate::set_skill_delay(state, actor, SHORT_DELAY);
        return;
    }
    // Pre-AoS: a body under a tenth of its hit points cannot hold a trance.
    if let Some(&Hitpoints { current, max }) = state.registry.get::<Hitpoints>(actor) {
        if u32::from(current) * 10 < u32::from(max) {
            state.localized_message(actor, BODY_TOO_WEAK, "");
            crate::set_skill_delay(state, actor, SHORT_DELAY);
            return;
        }
    }
    let mana = state
        .registry
        .get::<Mana>(actor)
        .copied()
        .unwrap_or(Mana { current: 0, max: 0 });
    if mana.current >= mana.max {
        state.localized_message(actor, AT_PEACE, "");
        crate::set_skill_delay(state, actor, SHORT_DELAY);
        return;
    }
    if !hands_free(state, actor) {
        state.localized_message(actor, HANDS_NOT_FREE, "");
        crate::set_skill_delay(state, actor, HANDS_DELAY);
        return;
    }

    // `chance = (50 + (skill - (manaMax - mana)) * 2) / 100`, in per-cent: the
    // emptier the pool the better the odds, and a grandmaster with a full-ish pool
    // is nearly certain. Rolled against the world's own generator, not a skill
    // check — ServUO is explicit that this must *bypass* the check so a failed
    // attempt does not train (`CheckSkill` is called only on success, below).
    let value = i32::from(crate::skill_value(state, actor, skill)) / 10;
    let missing = i32::from(mana.max.saturating_sub(mana.current));
    let chance = 50 + (value - missing) * 2;
    let roll = i32::from(crate::roll_u16(&mut state.rng, crate::PERCENT));
    crate::set_skill_delay(state, actor, MEDITATION_DELAY);
    if chance <= roll {
        state.localized_message(actor, NO_FOCUS, "");
        return;
    }
    // The trance took. The skill check here is for the *gain* alone; its result is
    // deliberately ignored, which is what ServUO does with it.
    let _ = roll_skill_band(state, actor, skill, crate::SkillBand::new(0, 1000));
    state.registry.insert(actor, Meditating);
    state.localized_message(actor, TRANCE, "");
    state.play_sound(actor, TRANCE_SOUND);
}

/// Spirit Speak: hear what the dead are saying, for a while.
///
/// The pre-AoS form (`CanHearGhosts`). Contact lasts `base / 50 * 90` seconds with
/// a floor of fifteen — a quarter of an hour of skill buys a minute and a half —
/// and it does **not** persist: like a cast in flight or a field on the ground it
/// is measured in seconds, and a restored one would be a contact whose expiry
/// nobody remembers announcing.
pub(super) fn spirit_speak(state: &mut WorldState, actor: EntityId) {
    let skill = Skill::SpiritSpeak;
    crate::set_skill_delay(state, actor, SPIRIT_SPEAK_DELAY);
    if !roll_skill_band(state, actor, skill, crate::SkillBand::new(0, 1000)) {
        // A failed contact takes away any contact still standing, which is
        // ServUO's `CanHearGhosts = false` on the failure branch.
        state.registry.remove::<HearsGhosts>(actor);
        state.localized_message(actor, NO_CONTACT, "");
        return;
    }
    // An existing contact is not extended — ServUO only starts a timer when there
    // is none, so a second use inside the first says the line and nothing more.
    if !state.registry.has::<HearsGhosts>(actor) {
        let base = u64::from(trained(state, actor, skill)) / 10;
        let seconds = (base * 90 / 50).max(CONTACT_FLOOR_SECONDS);
        let until = state.ticks + seconds * TICKS_PER_SECOND;
        state.registry.insert(actor, HearsGhosts { until });
    }
    state.play_sound(actor, CONTACT_SOUND);
    state.localized_message(actor, CONTACT, "");
}

/// Whether both weapon hands are free enough to meditate — ServUO's
/// `CheckOkayHolding`, which allows a spellbook and nothing else.
///
/// A shield counts as a held thing here (it is on the two-handed layer), which is
/// the pre-AoS rule and the reason a mage carries neither.
fn hands_free(state: &WorldState, actor: EntityId) -> bool {
    let Some(serial) = state.registry.serial_of(actor) else {
        return true;
    };
    !openshard_state::equipped_items(state, serial).any(|(item, worn)| {
        (worn.layer == LAYER_ONE_HANDED || worn.layer == LAYER_TWO_HANDED)
            && !state.registry.has::<Spellbook>(item)
    })
}

/// What a mobile has actually *trained*, in tenths — the base, with no help from
/// its stats. The length of a Spirit Speak contact reads this rather than the
/// effective value, as ServUO's does.
fn trained(state: &WorldState, entity: EntityId, skill: Skill) -> u16 {
    state
        .registry
        .get::<openshard_state::Skills>(entity)
        .map_or(0, |s| s.get(skill))
}

/// Tell a mobile its contact with the dead has lapsed — the line ServUO sends when
/// its timer runs out, said here by the expiry pass rather than by the skill.
pub(super) fn contact_faded(state: &mut WorldState, entity: EntityId) {
    state.localized_message(entity, CONTACT_FADING, "");
}
