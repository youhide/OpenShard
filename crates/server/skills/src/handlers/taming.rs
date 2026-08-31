//! Animal Taming — the skill that turns a creature into somebody's.
//!
//! ServUO's `AnimalTaming`, minus its three-to-five-beat timer: the attempt
//! resolves at once and the button is held forty seconds either way, which is the
//! same trade Poisoning makes. What is kept is every *gate*, in ServUO's order,
//! because each of them is a rule a player will meet: not tamable, already tame,
//! too many followers, no chance at all — and the anger roll, which is what makes
//! taming a bear a decision rather than a formality.
//!
//! Where the creature becomes a pet is not here. `npc` owns what a creature *is*,
//! so this returns the intent and the tick spends it — the split every other
//! handler in this crate uses.

use openshard_entities::EntityId;
use openshard_protocol::wire::ClilocId;
use openshard_protocol::world::FollowerSlots;
use openshard_state::components::{
    Body,
    Client,
    Ghost,
    Pet,
    Tamable,
};
use openshard_state::{
    Skill,
    WorldState,
};

use crate::check::roll_skill_band;

/// "That creature cannot be tamed."
const NOT_TAMABLE: ClilocId = ClilocId(1_049_655);
/// "That animal looks tame already."
const ALREADY_TAME: ClilocId = ClilocId(502_804);
/// "You have too many followers to tame that creature."
const TOO_MANY_FOLLOWERS: ClilocId = ClilocId(1_049_611);
/// "You have no chance of taming this creature."
const NO_CHANCE: ClilocId = ClilocId(502_806);
/// "You seem to anger the beast!"
const ANGERED: ClilocId = ClilocId(502_805);
/// "You start to tame the creature."
const START_TAMING: ClilocId = ClilocId(1_010_597);
/// "It seems to accept you as its master." — ServUO's success line.
const ACCEPTS_YOU: ClilocId = ClilocId(502_799);
/// "That being cannot be tamed." — a person.
const CANNOT_TAME_PERSON: ClilocId = ClilocId(502_469);
/// How far a tamer can reach — ServUO's `Target(2, …)` outside AoS.
pub(super) const TAME_RANGE: u32 = 3;
/// How many followers a tamer may keep — ServUO's `FollowersMax`.
pub const MAX_FOLLOWERS: u8 = 5;
/// The chance in a hundred that a beast angers instead — ServUO rolls
/// `CanAngerOnTame && 0.95 >= RandomDouble()`, so most of the time it is angered
/// *unless* the creature is one that never angers. Only the harder animals do.
const ANGER_CHANCE: u32 = 20;
/// The skill above which nothing angers any more, in tenths: a grandmaster's
/// approach does not startle a deer.
const ANGER_STOPS_AT: u16 = 900;

/// Animal Taming's cursor came back with something.
pub(super) fn taming(state: &mut WorldState, tamer: EntityId, target: EntityId) -> Option<Tamed> {
    if state.registry.has::<Client>(target) {
        state.private_overhead_cliloc(tamer, target, CANNOT_TAME_PERSON, "");
        return None;
    }
    let Some(&Body { id: body, .. }) = state.registry.get::<Body>(target) else {
        state.private_overhead_cliloc(tamer, target, NOT_TAMABLE, "");
        return None;
    };
    if state.registry.has::<Ghost>(target) {
        return None;
    }
    // The creature's own numbers if a spawn pinned them, else the core table's for
    // its body — the same "the engine's answer first" precedence a
    // weapon's damage has.
    let Some(what) = state
        .registry
        .get::<Tamable>(target)
        .copied()
        .or_else(|| openshard_state::tame::tamable(body))
    else {
        state.private_overhead_cliloc(tamer, target, NOT_TAMABLE, "");
        return None;
    };
    if state.registry.has::<Pet>(target) {
        state.private_overhead_cliloc(tamer, target, ALREADY_TAME, "");
        return None;
    }
    if followers_of(state, tamer) + what.slots.get() > MAX_FOLLOWERS {
        state.localized_message(tamer, TOO_MANY_FOLLOWERS, "");
        return None;
    }
    let skill = Skill::AnimalTaming;
    let value = crate::skill_value(state, tamer, skill);
    if value < what.min_skill {
        state.private_overhead_cliloc(tamer, target, NO_CHANCE, "");
        return None;
    }
    // The anger roll: a beast that is not simply given to you may turn instead, and
    // then it is a fight rather than a taming. Rolled on the world's own generator,
    // so a bad approach replays.
    if value < ANGER_STOPS_AT && state.rng.below(100) < ANGER_CHANCE {
        state.private_overhead_cliloc(tamer, target, ANGERED, "");
        return Some(Tamed {
            creature: target,
            tamer,
            slots: what.slots,
            angered: true,
        });
    }
    state.localized_message(tamer, START_TAMING, "");
    // The band is the creature's own difficulty: `min_skill` to a little past it,
    // so an animal at the edge of your ability teaches most — the same band shape
    // every other roll in the engine uses.
    let min = i32::from(what.min_skill);
    if !roll_skill_band(state, tamer, skill, crate::SkillBand::new(min, min + 400)) {
        state.private_overhead_cliloc(tamer, target, NO_CHANCE, "");
        return None;
    }
    state.private_overhead_cliloc(tamer, target, ACCEPTS_YOU, "");
    Some(Tamed {
        creature: target,
        tamer,
        slots: what.slots,
        angered: false,
    })
}

/// A taming resolved. `npc` owns what a creature *is*, so this crate says what
/// happened and the tick makes it so — the `ai::think_one` split.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Tamed {
    /// The animal.
    pub creature: EntityId,
    /// Who was trying.
    pub tamer:    EntityId,
    /// How many follower slots it fills.
    pub slots:    FollowerSlots,
    /// Whether it turned on the tamer instead of accepting them.
    pub angered:  bool,
}

/// How many follower slots a mobile has already spent — the sum over its pets, plus
/// the mount under it.
///
/// A **read-site derivation**: nothing is stored on the owner, so a pet that dies,
/// is released or is left behind stops counting the instant it does, with nothing
/// to keep in step. The status bar reads the same function.
#[must_use]
pub fn followers_of(state: &WorldState, owner: EntityId) -> u8 {
    let Some(serial) = state.registry.serial_of(owner) else {
        return 0;
    };
    let pets: u8 = state
        .registry
        .query::<Pet>()
        .filter(|(_, pet)| pet.owner == serial)
        .map(|(_, pet)| pet.slots.get())
        .sum();
    let mount = u8::from(state.registry.has::<openshard_state::components::Riding>(owner));
    pets.saturating_add(mount)
}
