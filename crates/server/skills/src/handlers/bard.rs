//! The bard skills: Peacemaking, Provocation and Discordance.
//!
//! ServUO's three, over `BaseInstrument`. What makes them a subsystem rather than
//! three skills is everything they share: an instrument in the pack that is spent
//! by every attempt, a **bard range** that widens with the skill, a Musicianship
//! check *before* the skill's own roll, and one difficulty formula
//! ([`base_difficulty`]) computed from what the target is rather than from a fixed
//! band. A bard who cannot play does not get to try.
//!
//! The two lasting effects are components with a tick expiry — `Pacified` and
//! `Discorded` — and neither is folded into anything: a pacified creature is
//! checked where it would swing, and a discorded one is read in `skill_value`,
//! which is the one question everything else already asks about how good somebody
//! is. So Discordance makes a creature hit worse, resist worse *and* cast worse
//! without combat, magic or the AI knowing what a lute is.

use openshard_entities::EntityId;
use openshard_protocol::wire::{ClilocId, CursorId};
use openshard_state::components::{Discorded, Drawn, Hitpoints, Instrument, Mana, Pacified, Skills, Stamina};
use openshard_state::instrument::instrument_data;
use openshard_state::{Skill, TICKS_PER_SECOND, TargetPurpose, WorldState};

use crate::check::roll_skill_band;

/// "You play poorly, and there is no effect." — the Musicianship check failing.
const PLAY_POORLY: ClilocId = ClilocId(500_612);
/// "You must have an instrument to play." — nothing in the pack.
const NEED_AN_INSTRUMENT: ClilocId = ClilocId(500_617);
/// "Whom do you wish to calm?"
const CALM_WHOM: ClilocId = ClilocId(1_049_525);
/// "You attempt to calm everyone, but fail."
const AREA_CALM_FAILED: ClilocId = ClilocId(500_613);
/// "You have pacified the creature."
const PACIFIED_IT: ClilocId = ClilocId(1_049_532);
/// "You attempt to calm your target, but fail."
const CALM_FAILED: ClilocId = ClilocId(1_049_531);
/// "You cannot calm that!"
const CANNOT_CALM: ClilocId = ClilocId(1_049_528);
/// "Whom do you wish to incite?"
const INCITE_WHOM: ClilocId = ClilocId(501_587);
/// "You play your music and your target becomes angered. Whom do you wish them to
/// attack?"
const ANGERED_WHOM: ClilocId = ClilocId(1_008_085);
/// "Your music fails to incite enough anger."
const NOT_ANGRY_ENOUGH: ClilocId = ClilocId(501_599);
/// "Your music succeeds, as you start a fight."
const FIGHT_STARTED: ClilocId = ClilocId(501_602);
/// "You can't tell someone to attack themselves!"
const ATTACK_THEMSELVES: ClilocId = ClilocId(501_593);
/// "You can't incite that!"
const CANNOT_INCITE: ClilocId = ClilocId(501_589);
/// "Whom do you wish to entice?" — Discordance's prompt.
const ENTICE_WHOM: ClilocId = ClilocId(1_049_541);
/// "You play the song suppressing your target's strength."
const SUPPRESSED: ClilocId = ClilocId(1_049_539);
/// "You fail to disrupt your target."
const DISCORD_FAILED: ClilocId = ClilocId(1_049_540);
/// "You hear jarring music, suppressing your strength." — told to the target.
const HEAR_JARRING: ClilocId = ClilocId(1_072_061);

/// The base of a bard's reach, in tiles — ServUO's `8 + value/15`.
const BARD_RANGE_BASE: u32 = 8;
/// How much skill widens it by one tile, in tenths.
const BARD_RANGE_PER_TILE: u16 = 150;
/// How long a pacified creature stays calm, in seconds. ServUO scales this by the
/// bard's skill against the target's difficulty; the classic floor is what a
/// grandmaster gets against something easy, and this is the plain version of it.
const PACIFY_SECONDS: u64 = 30;
/// How long a song of discord holds.
const DISCORD_SECONDS: u64 = 30;
/// The most Discordance takes off a target, as a percentage — ServUO's
/// `max(-28, discord / -4)`.
const DISCORD_MAX_PENALTY: u16 = 28;
/// The divisor under the skill in that formula, over a value in tenths.
const DISCORD_DIVISOR: u16 = 40;
/// How wide the band around a computed difficulty is, in tenths — ServUO's
/// `diff - 25 .. diff + 25`.
const BARD_BAND: i32 = 250;
/// The ceiling on any barding difficulty, in tenths (ServUO's `MaxBardingDifficulty`).
const MAX_DIFFICULTY: i32 = 1600;

/// How far a bard's music carries for `skill` — ServUO's `GetBardRange`.
#[must_use]
pub fn bard_range(state: &WorldState, bard: EntityId, skill: Skill) -> u32 {
    BARD_RANGE_BASE + u32::from(crate::skill_value(state, bard, skill) / BARD_RANGE_PER_TILE)
}

/// How hard a target is to work on, in tenths — ServUO's
/// `BaseInstrument.GetBaseDifficulty`.
///
/// `hitsMax * 1.6 + stamMax + manaMax + skillsTotal/10`, compressed above 700 (a
/// dragon is hard, but not a hundred times harder than a rat), then divided by ten
/// and capped. Everything in tenths so the tick replays.
#[must_use]
pub fn base_difficulty(state: &WorldState, target: EntityId) -> i32 {
    let hits = state.registry.get::<Hitpoints>(target).map_or(0, |h| h.max);
    let stam = state.registry.get::<Stamina>(target).map_or(0, |s| s.max);
    let mana = state.registry.get::<Mana>(target).map_or(0, |m| m.max);
    let skills = state
        .registry
        .get::<Skills>(target)
        .map_or(0, |s| i32::try_from(s.total() / 10).unwrap_or(0));
    // In whole points first, as ServUO computes it.
    let mut value = i32::from(hits) * 16 / 10 + i32::from(stam) + i32::from(mana) + skills / 10;
    if value > 700 {
        // `700 + (val - 700) * 3/11` — the compression that keeps the top of the
        // range reachable at all.
        value = 700 + (value - 700) * 3 / 11;
    }
    // `/10` for the difficulty, then into tenths for the band, which cancels: the
    // value in whole points *is* the difficulty in tenths.
    value.min(MAX_DIFFICULTY)
}

/// The instrument in a bard's pack, if there is one — ServUO's `GetInstrument`,
/// which searches the backpack and refuses if it finds nothing.
fn instrument_in_pack(state: &WorldState, bard: EntityId) -> Option<EntityId> {
    let serial = state.registry.serial_of(bard)?;
    let backpack = openshard_items::backpack_of(state, serial)?;
    openshard_state::contained_items(state, backpack)
        .map(|(entity, _)| entity)
        .find(|&entity| {
            state
                .registry
                .get::<Drawn>(entity)
                .is_some_and(|graphic| instrument_data(graphic.id).is_some())
        })
}

/// Play the instrument, well or badly, and spend a use.
///
/// The last tune takes the instrument with it — ServUO's `ConsumeUse`, which
/// deletes at zero and says so. Returns nothing: what the music *did* is the
/// caller's business.
fn play(state: &mut WorldState, bard: EntityId, item: EntityId, well: bool) {
    let sound = state
        .registry
        .get::<Drawn>(item)
        .and_then(|graphic| instrument_data(graphic.id))
        .map(|data| if well { data.well } else { data.badly });
    if let Some(sound) = sound {
        state.play_sound(bard, sound);
    }
    let left = state
        .registry
        .get::<Instrument>(item)
        .map_or(openshard_state::instrument::INSTRUMENT_MAX_USES, |i| i.uses_left);
    if left <= 1 {
        state.bus.send(InstrumentSpent { item });
    } else {
        state.registry.insert(item, Instrument { uses_left: left - 1 });
    }
}

/// An instrument played its last tune. Emitted rather than deleted here, because
/// removing an item is `items`' door — the tick spends it, like every other intent
/// this crate hands up.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct InstrumentSpent {
    /// The instrument that is done.
    pub item: EntityId,
}

/// The Musicianship check every bard attempt passes first — ServUO's
/// `CheckMusicianship`: it trains Musicianship, and then the raw skill is rolled
/// against a flat draw.
///
/// It is a *separate* roll from the skill's own, which is what makes Musicianship
/// worth training on its own: a bard with a fine Provocation and no Musicianship
/// simply plays poorly, and the second roll never happens.
fn check_musicianship(state: &mut WorldState, bard: EntityId) -> bool {
    let skill = Skill::Musicianship;
    let _ = roll_skill_band(state, bard, skill, crate::SkillBand::new(0, 1200));
    let value = u32::from(crate::skill_value(state, bard, skill));
    value / 10 > state.rng.below(100)
}

/// Start a bard skill: find the instrument, then raise the cursor its target wants.
///
/// Returns whether anything was started — the button's own answer.
pub(super) fn start(state: &mut WorldState, bard: EntityId, skill: Skill) -> bool {
    if instrument_in_pack(state, bard).is_none() {
        state.localized_message(bard, NEED_AN_INSTRUMENT, "");
        return true; // the skill *was* used; it simply had nothing to play
    }
    let prompt = match skill {
        Skill::Peacemaking => CALM_WHOM,
        Skill::Provocation => INCITE_WHOM,
        Skill::Discordance => ENTICE_WHOM,
        _ => return false,
    };
    super::raise_cursor(state, bard, skill, prompt)
}

/// Peacemaking's target: calm one creature, or — targeting yourself — everyone.
pub(super) fn peacemaking(state: &mut WorldState, bard: EntityId, target: EntityId) {
    let Some(item) = instrument_in_pack(state, bard) else {
        state.localized_message(bard, NEED_AN_INSTRUMENT, "");
        return;
    };
    if !check_musicianship(state, bard) {
        state.localized_message(bard, PLAY_POORLY, "");
        play(state, bard, item, false);
        return;
    }
    let skill = Skill::Peacemaking;
    if target == bard {
        // The area form: everyone within the bard's range stops fighting.
        if !roll_skill_band(state, bard, skill, crate::SkillBand::new(0, 1200)) {
            state.localized_message(bard, AREA_CALM_FAILED, "");
            play(state, bard, item, false);
            return;
        }
        play(state, bard, item, true);
        let range = bard_range(state, bard, skill);
        let until = state.ticks + PACIFY_SECONDS * TICKS_PER_SECOND;
        for other in mobiles_near(state, bard, range) {
            if other == bard {
                continue;
            }
            state.registry.insert(other, Pacified { until });
            state.disengage(other);
        }
        return;
    }
    if !state.registry.has::<openshard_state::components::Body>(target) {
        state.localized_message(bard, CANNOT_CALM, "");
        return;
    }
    let difficulty = base_difficulty(state, target);
    if roll_skill_band(
        state,
        bard,
        skill,
        crate::SkillBand::new(difficulty - BARD_BAND, difficulty + BARD_BAND),
    ) {
        play(state, bard, item, true);
        let until = state.ticks + PACIFY_SECONDS * TICKS_PER_SECOND;
        state.registry.insert(target, Pacified { until });
        state.disengage(target);
        state.localized_message(bard, PACIFIED_IT, "");
    } else {
        play(state, bard, item, false);
        state.localized_message(bard, CALM_FAILED, "");
    }
}

/// Provocation's first target: the creature to anger. Raises the second cursor.
pub(super) fn provoke_first(state: &mut WorldState, bard: EntityId, target: EntityId) {
    let Some(item) = instrument_in_pack(state, bard) else {
        state.localized_message(bard, NEED_AN_INSTRUMENT, "");
        return;
    };
    // A player is not a creature to be set on somebody.
    if !state.registry.has::<openshard_state::components::Body>(target)
        || state.registry.has::<openshard_state::components::Client>(target)
    {
        state.localized_message(bard, CANNOT_INCITE, "");
        return;
    }
    play(state, bard, item, true);
    state.localized_message(bard, ANGERED_WHOM, "");
    state.raise_target(
        bard,
        TargetPurpose::SkillSecond {
            skill: Skill::Provocation,
            first: target,
        },
    );
    if let Some((connection, serial)) = super::client_of(state, bard) {
        super::send_object_cursor(state, connection, CursorId(serial.raw()));
    }
}

/// Provocation's second target: whom the angered creature should attack.
pub(super) fn provoke_second(state: &mut WorldState, bard: EntityId, creature: EntityId, victim: EntityId) {
    let Some(item) = instrument_in_pack(state, bard) else {
        state.localized_message(bard, NEED_AN_INSTRUMENT, "");
        return;
    };
    if creature == victim {
        state.localized_message(bard, ATTACK_THEMSELVES, "");
        return;
    }
    if !check_musicianship(state, bard) {
        state.localized_message(bard, PLAY_POORLY, "");
        play(state, bard, item, false);
        return;
    }
    // The pair's difficulty, averaged and eased by five — ServUO's
    // `(diff(a) + diff(b)) * 0.5 - 5`.
    let difficulty = (base_difficulty(state, creature) + base_difficulty(state, victim)) / 2 - 50;
    let skill = Skill::Provocation;
    if !roll_skill_band(
        state,
        bard,
        skill,
        crate::SkillBand::new(difficulty - BARD_BAND, difficulty + BARD_BAND),
    ) {
        state.localized_message(bard, NOT_ANGRY_ENOUGH, "");
        play(state, bard, item, false);
        return;
    }
    play(state, bard, item, true);
    state.localized_message(bard, FIGHT_STARTED, "");
    // The provoked creature simply gets a `Combat` aimed at the victim — the same
    // component the AI drives and `combat::swings` fights with, so there is no
    // second fight loop anywhere.
    let Some(victim_serial) = state.registry.serial_of(victim) else {
        return;
    };
    state.registry.remove::<Pacified>(creature);
    state.registry.insert(
        creature,
        openshard_state::components::Combat::creature_engaged(victim_serial, state.ticks),
    );
}

/// Discordance: put a creature out of tune, so it is worse at everything.
pub(super) fn discordance(state: &mut WorldState, bard: EntityId, target: EntityId) {
    let Some(item) = instrument_in_pack(state, bard) else {
        state.localized_message(bard, NEED_AN_INSTRUMENT, "");
        return;
    };
    if !state.registry.has::<openshard_state::components::Body>(target) {
        state.localized_message(bard, CANNOT_CALM, "");
        return;
    }
    if !check_musicianship(state, bard) {
        state.localized_message(bard, PLAY_POORLY, "");
        play(state, bard, item, false);
        return;
    }
    let skill = Skill::Discordance;
    let difficulty = base_difficulty(state, target);
    if !roll_skill_band(
        state,
        bard,
        skill,
        crate::SkillBand::new(difficulty - BARD_BAND, difficulty + BARD_BAND),
    ) {
        state.localized_message(bard, DISCORD_FAILED, "");
        play(state, bard, item, false);
        return;
    }
    play(state, bard, item, true);
    // `max(-28, discord / -4)` as a positive penalty.
    let penalty = (crate::skill_value(state, bard, skill) / DISCORD_DIVISOR).min(DISCORD_MAX_PENALTY);
    let until = state.ticks + DISCORD_SECONDS * TICKS_PER_SECOND;
    state.registry.insert(target, Discorded { penalty, until });
    state.localized_message(bard, SUPPRESSED, "");
    state.localized_message(target, HEAR_JARRING, "");
}

/// Let a calm and a song of discord wear off. The counterpart of every other
/// expiry the tick runs, on the tick counter for the same reason.
pub fn expire_songs(state: &mut WorldState) {
    let now = state.ticks;
    let calm: Vec<EntityId> = state
        .registry
        .query::<Pacified>()
        .filter(|(_, calm)| now >= calm.until)
        .map(|(entity, _)| entity)
        .collect();
    for entity in calm {
        state.registry.remove::<Pacified>(entity);
    }
    let discorded: Vec<EntityId> = state
        .registry
        .query::<Discorded>()
        .filter(|(_, song)| now >= song.until)
        .map(|(entity, _)| entity)
        .collect();
    for entity in discorded {
        state.registry.remove::<Discorded>(entity);
    }
}

/// Musicianship on its own: playing an instrument for the sake of it, through the
/// `ItemUsed` double-click seam rather than the skill window.
///
/// ServUO's `BaseInstrument.OnDoubleClick`: it plays, it trains, and it spends a
/// use. The one bard skill with no target at all.
pub fn play_instrument(state: &mut WorldState, bard: EntityId, item: EntityId) {
    if matches!(
        openshard_state::item_location(state, item),
        Some(openshard_state::ItemLocation::Settled(
            openshard_state::SettledItemLocation::Equipped(_)
        ))
    ) {
        return; // worn, not carried: not something you can strike up
    }
    let well = check_musicianship(state, bard);
    play(state, bard, item, well);
    if !well {
        state.localized_message(bard, PLAY_POORLY, "");
    }
}

/// Every mobile within `range` of another, itself included — the area a bard's
/// music reaches. Collected before anything is written, since the sector index is
/// borrowed while it is read.
fn mobiles_near(state: &WorldState, centre: EntityId, range: u32) -> Vec<EntityId> {
    let Some(&openshard_state::components::Position(at)) =
        state
            .registry
            .get::<openshard_state::components::Position>(centre)
    else {
        return Vec::new();
    };
    let facet = state.facet_of(centre);
    state
        .facet_state(facet)
        .sectors()
        .mobiles_near(at, range)
        .map(|(entity, _)| entity)
        .collect()
}
