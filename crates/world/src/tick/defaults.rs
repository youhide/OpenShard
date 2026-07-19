use super::*;

/// How often the world ticks.
///
/// 20Hz. Fast enough that a 200ms walk step lands within a tick of when the
/// client expects it, and slow enough to leave room for everything a tick will
/// eventually do. Not a protocol constant — the client does not know or care.
pub const TICK_INTERVAL: Duration = Duration::from_millis(50);

/// A human male body.
pub(super) const BODY_HUMAN_MALE: u16 = 0x0190;
/// The graphic, container gump, and paperdoll layer of a starting backpack.
/// Layer 0x15 is UO's `Layer.Backpack`; the gump `0x003C` is the bag window the
/// client draws when it is opened.
pub(super) const BACKPACK_GRAPHIC: u16 = 0x0E75;
pub(super) const BACKPACK_GUMP: u16 = 0x003C;
pub(super) const BACKPACK_LAYER: u8 = 0x15;
/// How far an idle banker may drift from its post before it heads back — a couple
/// of tiles of shuffling near the counter, not a stroll out the door.
pub(super) const BANKER_WANDER: u8 = 2;
/// The skin hue a character gets when nothing else chose one — the same one
/// Sphere hands a body with no stored colour.
pub(super) const DEFAULT_HUE: u16 = 0x83EA;
/// Full daylight. The scale runs backwards: 0 is brightest, 0x1F pitch dark.
pub(super) const LIGHT_DAY: u8 = 0;
/// The facet a new character spawns on, and the world's fallback for a facet it
/// has not loaded. Zero is Felucca.
pub(super) const DEFAULT_FACET: u8 = 0;
/// The height to use when there is no map to ask. Only the tests still name it;
/// the world reads the flat default through [`WorldState::start_position`].
#[cfg(test)]
pub(super) const Z_WITHOUT_A_MAP: i8 = 0;
/// Notoriety 0x01 is "innocent" — the blue health bar.
pub(super) const NOTORIETY_INNOCENT: u8 = 0x01;
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
/// A body's own weight in stones, before anything it carries — Sphere's and
/// ServUO's `BodyWeight`. Sent on the status bar; kept well under the carry cap so
/// the client never thinks it is overloaded and refuses to run.
pub(super) const BODY_WEIGHT: u16 = 14;
/// The sum of the three stats a character may train to — the classic 225.
pub(super) const STAT_CAP: u16 = 225;
/// How many pets may follow a character. Only the shape matters until pets do.
pub(super) const MAX_FOLLOWERS: u8 = 5;

/// The weight a character can carry before it is overloaded, from its strength.
///
/// UO's `40 + floor(3.5 * str)`. Only the *ceiling* is sent on the status bar,
/// and only so the client can see it is not over it; nothing enforces it yet.
pub(super) const fn max_weight(strength: u16) -> u16 {
    40 + strength * 7 / 2
}
/// Ticks between a brain's beats — half a second at [`TICK_INTERVAL`]. Creatures
/// think in beats, not every tick: it paces their walk and spares the loop from
/// re-deciding a thousand times a second what has not changed.
pub(super) const AI_THINK_TICKS: u64 = 10;
/// The seed the world's roll generator starts from.
///
/// Fixed, so a fresh world's rolls are reproducible in a test and a replay. A
/// live shard that wanted unpredictable rolls would seed from the clock at
/// startup and save the seed with the world; that is an additive change, and one
/// value, not a redesign.
pub(super) const DEFAULT_SEED: u64 = 0x0DEE_5340_0000_0001;

/// How often the world offers a snapshot to persistence, in ticks.
///
/// Twenty seconds at [`TICK_INTERVAL`]. Sphere's default world save is ten
/// minutes, which is ten minutes of play a crash can cost; that number is from
/// an era when a save walked the entire world and blocked while it did. This one
/// writes what changed, on another task, so it can afford to be frequent.
///
/// In ticks and not a `Duration` on purpose. A shard that has fallen behind
/// should save less often, not spend its shortfall on the disk.
pub const SAVE_EVERY_TICKS: u64 = 400;
