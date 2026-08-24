//! The townsfolk beat: greeting, facing, barking and wandering.
//!
//! # The AI, and its seam
//!
//! [`live`] is the per-tick beat. It does everything it can directly on the world
//! — greet, turn to face, bark — and returns the one thing it cannot: the *steps*
//! it wants to take, because stepping is bound to the terrain and the walk
//! machinery the tick owns. That is the same decide-then-apply split the creature
//! brain uses (`ai::think_one` returns a direction, the tick calls `step`).
//!
//! # Why a random heading does not make an NPC walk
//!
//! The motion path implements turn-as-step: a step in a direction you are not
//! already facing only *turns* you (`world/tick/motion.rs`). So an idle NPC that
//! picks a fresh random heading every beat spends seven beats in eight pirouetting
//! and one actually moving, which reads exactly like standing still — and that is
//! what this did. `ai::think_one` already guards against it for creatures; the
//! fix here is the reference's own: ServUO's `BaseAI.WalkRandom(iChanceToNotMove,
//! iChanceToDir, iSteps)`, called as `WalkRandomInHome(2, 2, 1)`, which keeps the
//! current heading unless a one-in-`iChanceToDir` roll says otherwise. Most beats
//! it walks on, so it walks.
//!
//! # And a shopkeeper serving a customer stands still
//!
//! ServUO's `VendorAI.DoActionInteract` turns the vendor to face whoever it is
//! dealing with and takes no step at all. Without that the shopkeeper wanders off
//! mid-transaction, which is the other half of "the vendors feel dead": they were
//! not only silent, they were walking away.

use openshard_entities::EntityId;
use openshard_map::overlay::Doors;
use openshard_protocol::direction::{Direction, Facing};
use openshard_protocol::serial::Serial;
use openshard_protocol::world::{Facet, Point};
use openshard_state::components::{Heading, Npc, Position};
use openshard_state::sectors::in_range;
use openshard_state::{Rng, WorldState, WorldTick};

use crate::speech::{bark_line, greeting_for};

/// How long between an NPC's beats, in ticks (~2s at 20Hz).
pub const BEAT_TICKS: u64 = 40;
/// What fraction of a beat's interval that beat is spread over: one in this many.
///
/// Sphere re-rolls an idle NPC's timer at the end of *every* beat —
/// `_SetTimeoutS(1 + g_Rand.GetValFast(2))`, the last line of `NPC_Act_Idle` —
/// rather than re-arming it to a constant. The difference is not cosmetic. A
/// fixed interval preserves whatever phase two NPCs happen to share, forever:
/// jittering only the first beat sets the offsets once and then defends them.
/// Anything that puts two townsfolk on the same tick — a restore, a shared doze —
/// welds them together for the life of the shard, and a street of shopkeepers
/// greeting, turning and wandering in unison is what that looks like from the
/// client.
///
/// It is a *fraction*, not a fixed number of ticks, because the same helper arms
/// beats three orders of magnitude apart: a townsperson's two seconds, a
/// creature's four hundred milliseconds, a dozing mobile's sixteen. A flat spread
/// wide enough to matter to the first swamps the second — a 400 ms monster
/// arriving anywhere in 400–1400 ms is not pacing, it is noise, and it would make
/// `creature_step_ms` mean nothing. A quarter of the interval de-synchronises a
/// crowd within a few beats (the offsets random-walk apart and never re-converge)
/// while leaving every pace knob legible.
pub const BEAT_JITTER_FRACTION: u64 = 4;
/// How near a player has to come for a townsperson to greet them. ServUO's
/// `VendorAI.HandlesOnSpeech` uses the same four tiles.
pub(crate) const GREET_RANGE: u32 = 4;
/// How long a townsperson waits between greetings — long enough not to natter at
/// someone standing at the counter.
const GREET_COOLDOWN: u64 = 15 * 20;
/// And how far that wait is spread. The beats themselves are staggered, so two
/// NPCs greet on different ticks to begin with; without this they would still come
/// off cooldown together and re-converge every fifteen seconds.
const GREET_COOLDOWN_JITTER: u32 = 5 * 20;
/// How long between two of an NPC's own idle remarks. Much longer than a greeting:
/// a bark is atmosphere, and a street of shopkeepers each shouting every fifteen
/// seconds is worse than silence.
const BARK_COOLDOWN: u64 = 60 * 20;
/// The chance, in a hundred, that an idle NPC with nobody near says something to
/// itself this beat.
const BARK_CHANCE: u32 = 6;

/// When a mobile beating every `interval` ticks should next have its turn.
///
/// The one place a beat is armed, so the jitter cannot be forgotten at one of
/// them — and it was, at four: the restore path, the beat itself, the LOD doze
/// and the creature brain. Spends the world's seeded `rng`, so a shard still
/// replays; jitter is randomness *inside* the tick, which is exactly what
/// `WorldState::rng` is for.
#[must_use]
pub fn next_beat(rng: &mut Rng, now: WorldTick, interval: u64) -> WorldTick {
    now + interval + u64::from(rng.below(beat_jitter(interval)))
}

/// The spread applied to a beat of `interval` ticks, as a bound for `Rng::below`.
/// At least 1, so the call is always well formed and a one-tick beat is simply
/// never spread.
#[must_use]
pub fn beat_jitter(interval: u64) -> u32 {
    u32::try_from(interval / BEAT_JITTER_FRACTION)
        .unwrap_or(u32::MAX)
        .max(1)
}

/// When a mobile that has just *arrived* — spawned, restored, or woken — should
/// take its first turn: somewhere inside the next `interval`, never `now`.
///
/// Sphere's `CChar::_GoAwake`, whose comment says the whole thing: *"make it tick
/// randomly in the next sector, so all awaken NPCs get a different tick time."*
/// The distinction from [`next_beat`] is that this spreads across the *whole*
/// interval rather than adding to it — an arrival should be prompt as well as
/// staggered, and a mobile made to wait a full beat before its first is a mobile
/// that visibly starts up around the player.
#[must_use]
pub fn first_beat(rng: &mut Rng, now: WorldTick, interval: u64) -> WorldTick {
    now + u64::from(rng.below(u32::try_from(interval).unwrap_or(u32::MAX)))
}

/// One tick of townsfolk life. Returns the steps the NPCs want —
/// `(serial, direction)` — for the tick to apply through its own terrain-checked
/// `step`. Everything else is done here on the world.
///
/// The world's hour rides on `state` (`WorldState::hour`, refreshed once a tick
/// by `world/tick/ambient.rs`); it is only read when `gameplay.npc_schedule` is
/// on.
#[must_use]
pub fn live(state: &mut WorldState) -> Vec<(Serial, Direction)> {
    let now = state.ticks;
    let due: Vec<EntityId> = state
        .registry
        .query::<Npc>()
        .filter(|(_, npc)| now >= npc.next_beat)
        .map(|(entity, _)| entity)
        .collect();

    let mut steps = Vec::new();
    for npc in due {
        // Space out the next beat first, so an early return below still paces it.
        let armed = next_beat(&mut state.rng, now, BEAT_TICKS);
        if let Some(mut n) = state.registry.get::<Npc>(npc).copied() {
            n.next_beat = armed;
            state.registry.insert(npc, n);
        }
        let Some(&Position(at)) = state.registry.get::<Position>(npc) else {
            continue;
        };
        let facet = state.facet_of(npc);

        // An NPC nobody could see or hear need not think. The same `lod` gate the
        // creature brains sit behind (`world/tick.rs`), and for the same reason:
        // a full Felucca is thousands of mobiles, and the ones alone in a field
        // are exactly the ones whose beat nobody can tell was skipped.
        if state.gameplay.lod && !state.any_player_near(at, state.gameplay.lod_radius, facet) {
            let doze = BEAT_TICKS * state.gameplay.lod_idle_factor.max(1);
            let armed = next_beat(&mut state.rng, now, doze);
            if let Some(mut n) = state.registry.get::<Npc>(npc).copied() {
                n.next_beat = armed;
                state.registry.insert(npc, n);
            }
            continue;
        }

        // Someone close? Face them, greet them if it is time, and stand still this
        // beat — you do not wander off mid-hello, and you certainly do not wander
        // off mid-sale.
        if let Some((visitor, visitor_at)) = nearest_player(state, facet, at, GREET_RANGE) {
            attend(state, npc, at, visitor, visitor_at, now);
            continue;
        }

        // Nobody near: a remark to itself now and then, and a drift near home.
        bark(state, npc, now);
        if let Some(dir) = wander_step(state, npc, at) {
            if let Some(serial) = state.registry.serial_of(npc) {
                steps.push((serial, dir));
            }
        }
    }
    steps
}

/// Attend to a visitor: turn to face them (ServUO's `VendorAI.DoActionInteract`)
/// and greet them if the cooldown has passed. Every trade greets, not only the
/// bankers — the greeting line itself comes from the trade's speech table.
fn attend(
    state: &mut WorldState,
    npc: EntityId,
    at: Point,
    visitor: EntityId,
    visitor_at: Point,
    now: WorldTick,
) {
    // Turn to face them, and let watchers see the turn.
    if let Some(dir) = openshard_movement::direction_toward(at, visitor_at) {
        let facing = Facing::walking(dir);
        if state.registry.get::<Heading>(npc).map(|h| h.0) != Some(facing) {
            state.registry.insert(npc, Heading(facing));
            state.broadcast_move(npc);
        }
    }

    let Some(npc_state) = state.registry.get::<Npc>(npc).copied() else {
        return;
    };
    if now < npc_state.next_greet {
        return;
    }
    let Some(line) = greeting_for(state, npc, visitor) else {
        return;
    };
    crate::say(state, npc, &line);
    // Jitter the cooldown too, so two NPCs that did happen to greet on one tick do
    // not come off cooldown on one tick either.
    let cooldown = GREET_COOLDOWN + u64::from(state.rng.below(GREET_COOLDOWN_JITTER));
    state.registry.insert(
        npc,
        Npc {
            next_greet: now + cooldown,
            ..npc_state
        },
    );
}

/// An idle remark, when nobody is within greeting range. Silent unless the trade's
/// table supplies a line, so a bare shard's townsfolk do not chatter nonsense.
fn bark(state: &mut WorldState, npc: EntityId, now: WorldTick) {
    let Some(npc_state) = state.registry.get::<Npc>(npc).copied() else {
        return;
    };
    if now < npc_state.next_greet || state.rng.below(100) >= BARK_CHANCE {
        return;
    }
    let Some(line) = bark_line(state, npc) else {
        return;
    };
    crate::say(state, npc, &line);
    state.registry.insert(
        npc,
        Npc {
            next_greet: now + BARK_COOLDOWN,
            ..npc_state
        },
    );
}

/// An idle step for an NPC: head to its post when it has strayed, else drift.
/// `None` means stand still this beat. The tile is not checked here — the tick's
/// `step` validates it against the terrain, and a step into a wall simply turns
/// the NPC.
fn wander_step(state: &mut WorldState, npc: EntityId, at: Point) -> Option<Direction> {
    let Npc { home, wander, .. } = *state.registry.get::<Npc>(npc)?;
    if wander == 0 {
        return None;
    }
    let post = post_at_hour(state, npc, home);

    // ServUO's `WalkRandomInHome`: past the home range, walk back; inside it,
    // `WalkRandom`.
    if chebyshev(at, post) > u32::from(wander) {
        return walk_home(state, npc, at, post);
    }

    // `WalkRandom(2, 2, 1)`: one chance in two of not moving at all, and one in
    // two of picking a new heading rather than continuing on the current one.
    // Reusing the heading is what makes the step translate instead of turn.
    if state.rng.below(2) == 0 {
        return None;
    }
    if state.rng.below(2) == 0 {
        return Some(Direction::from_bits(state.rng.below(8) as u8));
    }
    state
        .registry
        .get::<Heading>(npc)
        .map(|h| h.0.direction)
        .or_else(|| Some(Direction::from_bits(state.rng.below(8) as u8)))
}

/// A step back toward the post — pathed around the counter, not into it.
///
/// A townsperson is human, so it walks on [`Doors::AllOpen`]: a shut door on
/// the way is planned through and opened when it is reached, rather than being
/// an obstacle to route round (the auto-close swings it shut again behind
/// them). The opening itself is `ai`'s, on whichever step meets the door —
/// this used to re-derive the door out of the obstruction index for the *first*
/// step only, which was a second reading of a rule `ai` already applied to
/// every other step of the same route.
///
/// The post does not move, so the route to it is planned once and walked
/// ([`openshard_ai::Goal::Fixed`]). That matters here more than anywhere else:
/// this is the caller whose beat is [`BEAT_TICKS`], and a route with the old
/// time window on it was stale on every beat this function ever read one —
/// see `Goal` for the arithmetic.
fn walk_home(state: &mut WorldState, npc: EntityId, at: Point, post: Point) -> Option<Direction> {
    let facet = state.facet_of(npc);
    openshard_ai::step_body_toward(
        state,
        npc,
        facet,
        at,
        post,
        Doors::AllOpen,
        openshard_ai::Goal::Fixed,
    )
}

/// Where this NPC should be at this hour.
///
/// Off by default and beyond both references — ServUO's nearest equivalent is a
/// hand-placed `WayPoint` chain, which is not tied to the clock at all. With
/// `gameplay.npc_schedule` on, a townsperson with a `night_home` walks to it
/// outside working hours and back to its post inside them. Without the setting, or
/// without a `night_home` in the pack's data, this is the post and nothing changes.
fn post_at_hour(state: &WorldState, npc: EntityId, home: Point) -> Point {
    if working_hours(state) {
        return home;
    }
    state
        .registry
        .get::<openshard_state::components::NightHome>(npc)
        .map_or(home, |h| h.0)
}

/// Whether the town is at work: inside the shard's working hours, or always if
/// the schedule is off.
///
/// The one predicate the whole routine turns on — where a townsperson stands,
/// whether its shop serves, and which greeting it gives. Three rules reading one
/// answer, rather than three comparisons drifting apart.
#[must_use]
pub fn working_hours(state: &WorldState) -> bool {
    if !state.gameplay.npc_schedule {
        return true;
    }
    let work = u64::from(state.gameplay.npc_work_hour);
    let rest = u64::from(state.gameplay.npc_home_hour);
    // A working day that does not wrap midnight is the only shape the setting
    // allows; `config` rejects the rest, so this comparison is enough.
    state.hour >= work && state.hour < rest
}

/// The nearest player to `at` within `range` on `facet`, and where it stands.
fn nearest_player(state: &WorldState, facet: Facet, at: Point, range: u32) -> Option<(EntityId, Point)> {
    state
        .players
        .values()
        .filter_map(|&entity| {
            let pos = state.registry.get::<Position>(entity)?.0;
            (state.facet_of(entity) == facet && in_range(pos, at, range)).then_some((entity, pos))
        })
        .min_by_key(|(_, pos)| squared_distance(*pos, at))
}

/// Chebyshev distance — the square UO measures range in.
pub(crate) fn chebyshev(a: Point, b: Point) -> u32 {
    let dx = i32::from(a.x).abs_diff(i32::from(b.x));
    let dy = i32::from(a.y).abs_diff(i32::from(b.y));
    dx.max(dy)
}

/// Squared Euclidean distance, for picking the *nearest* of several in range.
fn squared_distance(a: Point, b: Point) -> i64 {
    let dx = i64::from(a.x) - i64::from(b.x);
    let dy = i64::from(a.y) - i64::from(b.y);
    dx * dx + dy * dy
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chebyshev_is_the_square_uo_measures() {
        assert_eq!(chebyshev(Point::new(0, 0, 0), Point::new(3, 1, 0)), 3);
        assert_eq!(chebyshev(Point::new(5, 5, 0), Point::new(5, 5, 0)), 0);
    }
}
