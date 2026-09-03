//! Summons: a creature a spell called up, which stands for a while and then goes.
//!
//! `npc` owns what a creature *is* — it spawns them, dresses them and gives them a
//! brain — so calling one up belongs here beside [`crate::pets`] and
//! [`crate::guards`], and `magic` keeps deciding only *that* a summon happened. The
//! shape is the guard's: a mobile spawned for a purpose, carrying the tick it goes
//! ([`Summoned`]), retired by a sweep the world runs once a tick.
//!
//! # A summon is a pet with a deadline
//!
//! ServUO's `BaseCreature.Summon` calls `SetControlMaster(caster)` and sets
//! `Summoned = true`, and everything a controlled creature does then follows: it is
//! friendly, it walks at its master's heel, it answers "all kill", and it counts
//! against `Followers`. All four of those already exist here as [`Pet`], so a summon
//! *is* one — the marker beside it carries only what a pet has not got, which is a
//! tick to vanish on. Nothing that follows, obeys or counts had to learn a second
//! kind of creature.
//!
//! # What the marker is for beyond this file
//!
//! Three other places read [`Summoned`] and none of them could have been covered
//! from here: the world's save sweep skips one (a restored five-minute daemon is a
//! permanent one whose caster no longer exists — the same reason a field tile and a
//! gate are skipped), the death path gives one no corpse (pre-AoS
//! `DeleteCorpseOnDeath`, and without it a summoned daemon would print gold), and
//! Dispel, when it lands, is the question "is this thing summoned" and nothing else.

use openshard_entities::EntityId;
use openshard_map::grid::Tile;
use openshard_protocol::mobile::Notoriety;
use openshard_protocol::wire::{
    Graphic,
    Hue,
    SoundId,
};
use openshard_protocol::world::{
    DamageType,
    Facet,
    Point,
};
use openshard_state::components::{
    Position,
    Skills,
    Spellbook,
    SummonKind,
    Summoned,
};
use openshard_state::summon::{
    ROLLED_LIFETIME_FROM,
    ROLLED_LIFETIME_SPAN,
    SUMMON_AGGRESSION,
    SUMMONABLE_BEASTS,
    SummonLifetime,
    magery_lifetime_seconds,
    summoned,
};
use openshard_state::{
    Skill,
    TICKS_PER_SECOND,
    WorldState,
};

use crate::dress::ShoeType;
use crate::spawn::{
    SpawnSpec,
    spawn,
};

/// The puff a summon goes out in, and its sound — ServUO's `BaseCreature.Dispel`,
/// which is the reference's one picture of a summon *ending*.
///
/// Its own `UnsummonTimer` is silent: it calls `Delete()` and nothing else. That is
/// a creature blinking out of existence with no feedback, which reads as a client
/// glitch — the same reason [`crate::guards`] flashes a guard away rather than
/// letting it vanish — so the dispel art is used for every way a summon can end.
const UNSUMMON_GRAPHIC: Graphic = Graphic(0x3728);
const UNSUMMON_SOUND: SoundId = SoundId(0x0201);

/// How fast a summon swings, in ticks. ServUO gives its creatures a speed derived
/// from dexterity; this engine's `SwingSpeed` is a plain interval, and the summons
/// are all trained fighters, so they all take the same brisk one.
const SUMMON_SWING_TICKS: u64 = 2 * TICKS_PER_SECOND;

/// Call up a creature for `caster` and hand it back.
///
/// `at` is where the spell aimed. Whether that is *the* tile or only an anchor is
/// the creature's own business ([`SummonData::at_the_mark`]): the two summons that
/// take a target are laid on the tile the player picked and refused if it is
/// blocked, while the six that take none appear on a free tile *beside* the caster,
/// never on them — ServUO's `FindValidSpawnLocation(.., surroundingsOnly: true)`.
///
/// `None` is "nothing appeared", which the caller reports as a blocked location —
/// the reference's own answer (cliloc 501942) for the case that actually happens. A
/// world out of mobile serials also lands here, and has already said so in the log.
///
/// [`SummonData::at_the_mark`]: openshard_state::summon::SummonData::at_the_mark
pub fn summon(state: &mut WorldState, caster: EntityId, kind: SummonKind, at: Point) -> Option<EntityId> {
    let data = summoned(kind);
    let facet = state.facet_of(caster);
    let anchor = if data.at_the_mark {
        at
    } else {
        state.registry.get::<Position>(caster).map_or(at, |p| p.0)
    };
    let spot = spawn_spot(state, facet, anchor, !data.at_the_mark)?;
    // The body last, because Summon Creature draws one and the draw must not happen
    // on a cast that is about to be refused for want of room — a seeded generator is
    // a shared stream, and a roll nobody uses shifts every later roll on the shard.
    let body = if matches!(kind, SummonKind::Creature) {
        SUMMONABLE_BEASTS[state.rng.below(SUMMONABLE_BEASTS.len() as u32) as usize]
    } else {
        data.body
    };
    let creature = spawn(
        state,
        SpawnSpec {
            body,
            hue: Hue(0),
            hits: data.hits,
            // Overwritten by `pets::tame` below, which turns a controlled creature
            // the friendly green every client draws one in. Named honestly here all
            // the same: a summon in flight between these two lines is nobody's yet.
            notoriety: Notoriety::Neutral,
            damage: data.damage,
            resistance: data.resistance,
            swing: SUMMON_SWING_TICKS,
            sight: data.sight,
            aggression: SUMMON_AGGRESSION,
            // The shard's own creature beat.
            beat: 0,
            ranged: None,
            ranged_kind: DamageType::Physical,
            // A summon has no life of its own to drift through: it heels, or it
            // fights what it was told to.
            wander: false,
            position: spot,
            facet,
            // No name of its own — the client shows what its body is, which for
            // every one of these is exactly what the spell promised.
            name: None,
            title: None,
            shoe: ShoeType::None,
            // Killing something that was never really there earns nothing.
            // ServUO says the same by skipping every award for a `Summoned`.
            fame: 0,
            karma: 0,
            night_home: None,
            banker: false,
            vendor: false,
            healer: false,
            equipment: Vec::new(),
            skills: data.skills.to_vec(),
            // Nothing summonable casts. ServUO's summons are all melee — the
            // one that is not, the Energy Vortex, fights by touch here too —
            // and a summoned caster would want a repertoire in
            // `openshard_state::summon`'s table rather than a number invented
            // at this call site.
            mana: 0,
            spells: Spellbook(0),
        },
    )?;
    // Its master's, by the one path a creature becomes somebody's — so a summon
    // heels, obeys "all kill" and counts against the follower cap without any of
    // that code learning what a summon is.
    crate::pets::tame(state, creature, caster, data.slots, false);
    let expires_at = state.ticks + lifetime_ticks(state, caster, data.lifetime);
    state.registry.insert(creature, Summoned { kind, expires_at });
    Some(creature)
}

/// Take a summon out of the world in a puff, wherever it stood.
///
/// The one exit, so an expiry and a death end the same way and neither can grow a
/// step the other forgets.
pub fn unsummon(state: &mut WorldState, creature: EntityId) {
    if let Some(&Position(at)) = state.registry.get::<Position>(creature) {
        crate::flash(state, creature, at, UNSUMMON_GRAPHIC, UNSUMMON_SOUND);
    }
    state.despawn_mobile(creature);
}

/// Retire the summons whose time is up. The tick counter, like every other timer
/// here, so a shard replays.
///
/// The follower count on the caster's status bar needs no telling: it is derived
/// from what stands in the world (`skills::followers_of`), and the bar's own
/// half-second diff notices the slot come free.
pub fn expire_summons(state: &mut WorldState) {
    let now = state.ticks;
    let done: Vec<EntityId> = state
        .registry
        .query::<Summoned>()
        .filter(|(_, summon)| now >= summon.expires_at)
        .map(|(entity, _)| entity)
        .collect();
    for creature in done {
        unsummon(state, creature);
    }
}

/// How long this caster's summon stands, in ticks.
fn lifetime_ticks(state: &mut WorldState, caster: EntityId, lifetime: SummonLifetime) -> u64 {
    let seconds = match lifetime {
        SummonLifetime::Magery => {
            let magery = state
                .registry
                .get::<Skills>(caster)
                .map_or(0, |s| s.get(Skill::Magery));
            magery_lifetime_seconds(magery)
        }
        // On the world's own generator, so a replay summons for the same span.
        SummonLifetime::Rolled => ROLLED_LIFETIME_FROM + u64::from(state.rng.below(ROLLED_LIFETIME_SPAN)),
    };
    // A summon that expired the tick it appeared would be a spell that costs
    // eighth-circle mana and does nothing; the floor is what a caster with no
    // Magery at all (a staff `.summon`, a script) gets.
    seconds.max(1) * TICKS_PER_SECOND
}

/// Where the creature actually stands.
///
/// With `surroundings_only` the eight tiles around `anchor` are tried and the
/// anchor itself never is — ServUO's `FindValidSpawnLocation`, whose whole point is
/// that a self-cast summon appears *beside* its caster. The walk starts at a
/// rotation drawn from the world's generator, so two elementals in a row do not
/// both land to the north-west, and a replay lands them the same.
///
/// Without it, only the aimed tile: Blade Spirits is laid where the player pointed
/// or not at all.
fn spawn_spot(state: &mut WorldState, facet: Facet, anchor: Point, surroundings_only: bool) -> Option<Point> {
    /// ServUO's `m_Offsets`, the eight neighbours in its own order.
    const OFFSETS: [(i32, i32); 8] = [
        (-1, -1),
        (-1, 0),
        (-1, 1),
        (0, -1),
        (0, 1),
        (1, -1),
        (1, 0),
        (1, 1),
    ];
    if !surroundings_only {
        return standing_point(state, facet, anchor.x, anchor.y, anchor.z);
    }
    let rotation = state.rng.below(OFFSETS.len() as u32) as usize;
    for step in 0..OFFSETS.len() {
        let (dx, dy) = OFFSETS[(rotation + step) % OFFSETS.len()];
        let x = i32::from(anchor.x) + dx;
        let y = i32::from(anchor.y) + dy;
        if !(0..=i32::from(u16::MAX)).contains(&x) || !(0..=i32::from(u16::MAX)).contains(&y) {
            continue; // off the world edge
        }
        if let Some(spot) = standing_point(state, facet, x as u16, y as u16, anchor.z) {
            return Some(spot);
        }
    }
    None
}

/// The point a body put at `x, y` would occupy, coming from `near_z` — or `None`
/// where nothing there can hold one.
///
/// [`openshard_movement::arrival_z`] and not the bare map, for the reason it gives:
/// a ship's deck, a house floor and a stair laid this morning are all places a
/// summon may stand and none of them are in the map file.
fn standing_point(state: &WorldState, facet: Facet, x: u16, y: u16, near_z: i8) -> Option<Point> {
    let z = openshard_movement::arrival_z(
        &state.footing(facet, openshard_map::overlay::Doors::AsTheyStand),
        Tile::new(x, y),
        i32::from(near_z),
        openshard_movement::PLAYER_HEIGHT,
    )?;
    i8::try_from(z).ok().map(|z| Point::new(x, y, z))
}
