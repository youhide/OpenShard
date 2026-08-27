use super::*;
use openshard_protocol::wire::Graphic;

/// How often the world ticks.
///
/// 40Hz. Fast enough that an interactive command waits no more than 25ms for
/// the next authoritative tick, while a 200ms walk step still lands on a tick.
/// client expects it, and slow enough to leave room for everything a tick will
/// eventually do. Not a protocol constant — the client does not know or care.
pub const TICK_INTERVAL: Duration = Duration::from_millis(25);

/// The two halves of one fact, welded.
///
/// [`TICK_INTERVAL`] is how long a tick lasts and
/// [`TICKS_PER_SECOND`](openshard_state::TICKS_PER_SECOND) is how many of them a
/// second holds, and they live in different crates because the world defines the
/// loop while `state` is what every timer counts in. Nothing made them agree
/// until now, and they did not: the interval moved from 50ms to 25ms and thirteen
/// constants across combat, the AI, the NPCs, the quests and this file went on
/// being written in twentieths of a second — a swing at half its era's speed, a
/// murder count fading in four hours instead of eight, a world saving twice as
/// often as its own comment claimed.
///
/// A compile error is the only place that can be caught, because every one of
/// those numbers is *arithmetically fine* at either rate; what changes is what
/// they mean.
const _: () = assert!(
    TICK_INTERVAL.as_millis() as u64 * openshard_state::TICKS_PER_SECOND == 1_000,
    "TICK_INTERVAL and TICKS_PER_SECOND are the same tick counted two ways: \
     change one and change the other."
);

/// A human male body.
pub(super) const BODY_HUMAN_MALE: Graphic = Graphic(0x0190);
/// The graphic and gump of a starting backpack.  They belong to `items`, where
/// staff-created backpacks use the same pair.
pub(super) use openshard_items::{BACKPACK_GRAPHIC, BACKPACK_GUMP};
/// The skin hue a character gets when nothing else chose one — the same one
/// Sphere hands a body with no stored colour.
pub(super) const DEFAULT_HUE: u16 = 0x83EA;
/// The colour overhead text takes when nothing else chose one — the client's
/// grey name label, [`Hue::LABEL`]. Only an *item*'s single click reaches it: a
/// mobile's label is coloured by its standing instead.
pub(super) const TEXT_HUE: Hue = Hue::LABEL;
/// Full daylight. The scale runs backwards: 0 is brightest, 0x1F pitch dark.
pub(super) const LIGHT_DAY: Light = Light(0);
/// Full night — ServUO's `LightCycle.NightLevel`. Dark enough to want a lantern,
/// not dark enough to be unplayable; the two-hour ramps either side of it are in
/// `tick/ambient.rs`.
pub(super) const LIGHT_NIGHT: Light = Light(12);
/// The light a Night Sight caster sees by — the brightest level, so the dark
/// lifts wherever they are. Distinct name from [`LIGHT_DAY`] though both are 0:
/// one is what time it is, the other is a buff beating it.
pub(super) const LIGHT_NIGHTSIGHT: Light = Light(0);
/// The facet a new character spawns on, and the world's fallback for a facet it
/// has not loaded. Zero is Felucca.
pub(super) const DEFAULT_FACET: u8 = 0;
/// The height to use when there is no map to ask. Only the tests still name it;
/// the world reads the flat default through [`WorldState::start_position`].
#[cfg(test)]
pub(super) const Z_WITHOUT_A_MAP: i8 = 0;
/// The facet size used when there is no map. Big enough for anywhere a test
/// puts something; the grid is a `Vec` of empty buckets and costs nothing.
pub(super) const FACET_WITHOUT_A_MAP: (u32, u32) = (7168, 4096);
/// The strength a character starts with, and so — hit points deriving from
/// strength — its starting hit points. A placeholder for what character creation
/// will set.
pub(super) const DEFAULT_HITPOINTS: u16 = 100;
/// The intelligence a character starts with, and so its starting mana.
pub(super) const DEFAULT_MANA: u16 = 100;
/// The dexterity a character starts with.
pub(super) const DEFAULT_DEXTERITY: u16 = 100;
/// A body's own weight in stones. Lives in `items` beside the walk that sums
/// what is on top of it, and beside the cap it is compared against.
pub(super) use openshard_items::BODY_WEIGHT;
/// The sum of the three stats a character may train to — the classic 225.
pub(super) const STAT_CAP: u16 = 225;
/// How many pets may follow a character. Only the shape matters until pets do.
/// How many followers a character may keep at once — the same number the taming
/// gate reads, so the bar and the refusal can never disagree.
pub(super) const MAX_FOLLOWERS: u8 = openshard_skills::MAX_FOLLOWERS;

/// The weight a character can carry before it is overloaded. In `items` for the
/// reason [`BODY_WEIGHT`] is: three rules read it now, and two copies of
/// `40 + 3.5 * str` is a shard where a mule can walk but cannot recall.
pub(super) use openshard_items::max_weight;
/// The seed a world's roll generator starts from when nothing says otherwise.
///
/// Fixed, so a fresh world's rolls are reproducible in a test and a replay. An
/// operator overrides it with `world.seed`
/// ([`World::with_seed`](super::World::with_seed)), which is the only way the
/// value here is ever replaced — a live shard does not re-seed at boot, it
/// *resumes*: the save carries where the stream got to and
/// [`World::with_rng_state`](super::World::with_rng_state) picks it up. Seeding a
/// restored world would deal the previous run's rolls a second time.
pub(super) const DEFAULT_SEED: u64 = 0x0DEE_5340_0000_0001;

/// How often the world offers a snapshot to persistence, in ticks.
///
/// Twenty seconds at [`TICK_INTERVAL`]. Sphere's default world save is ten
/// minutes, which is ten minutes of play a crash can cost; that number is from
/// an era when a save walked the entire world and blocked while it did. This one
/// writes what changed, on another task, so it can afford to be frequent.
///
/// In ticks and not a `Duration` on purpose. A shard that has fallen behind
/// should save less often, not spend its shortfall on the disk. Derived from the
/// tick rate all the same: twenty seconds is the decision, and as a bare `400` it
/// became ten of them the day the tick halved.
pub const SAVE_EVERY_TICKS: u64 = 20 * TICKS_PER_SECOND;
