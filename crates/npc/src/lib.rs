//! Townsfolk: the bankers (and, soon, vendors) who make a town a place with
//! people in it rather than a set of props.
//!
//! # Why a crate and not more of `tick.rs`
//!
//! `world/tick.rs` is orchestration — the command dispatch, the system order, the
//! movement machinery. Rules do not go there. Townsfolk behaviour — the service a
//! banker offers, the words it says, the little life that turns it to face you and
//! shuffles it near its post — is a gameplay domain, so it is a `fn(&mut
//! WorldState)` here, the same shape `combat`, `chat` and `skills` use. The
//! components it hangs on ([`Npc`], [`Banker`]) live in `state`.
//!
//! # The AI, and its seam
//!
//! [`live`] is the per-tick beat. It does everything it can directly on the world
//! — greet, turn to face, count gold — and returns the one thing it cannot: the
//! *steps* it wants to take, because stepping is bound to the terrain and the walk
//! machinery the tick owns. That is the same decide-then-apply split the creature
//! brain uses (`ai::think_one` returns a direction, the tick calls `step`).
//!
//! The base is ServUO's townsfolk behaviour, kept simple and improved where it is
//! cheap to: a banker greets by name with a line chosen fresh each time, keeps to
//! a home range, and its wandering is validated by the same terrain the client
//! walks (a step into a wall just turns it, harmlessly). Proper A* pathfinding —
//! for chasing and for finding a way *around* an obstacle rather than nosing into
//! it — is the next AI slice; Sphere's is a poor guide and ServUO's is the base to
//! beat.

use openshard_entities::EntityId;
use openshard_gateway::ConnectionId;
use openshard_protocol::{encode_message, Direction, Facing, Point, SYSTEM_SERIAL};
use openshard_state::components::{
    Amount, Banker, Contained, Container, Equipped, Graphic, Heading, Name, Npc, Position,
};
use openshard_state::rng::Rng;
use openshard_state::sectors::in_range;
use openshard_state::WorldState;

mod spawn;
mod vendor;
pub use spawn::{spawn, MobileSpawned, SpawnSpec};
pub use vendor::{
    buy, buy_keyword, offer_sell_list, open_shop, sell, stock, StockLine, STOCK_LAYER,
};

/// The bank box graphic, gump and layer — ServUO's `BankBox` on `Layer.Bank`. A
/// character wears one; a banker opens it. Exported so the world equips it on the
/// same layer this crate opens.
pub const BANK_GRAPHIC: u16 = 0x0E7C;
/// The bank box gump.
pub const BANK_GUMP: u16 = 0x004A;
/// The bank layer, `Layer.Bank`.
pub const BANK_LAYER: u8 = 0x1D;

/// How near a banker a player must be for "bank" to open the box — ServUO's 12.
const BANK_RANGE: u32 = 12;
/// The gold-coin graphic, `Gold`'s itemid in ServUO. What a balance counts.
pub(crate) const GOLD_GRAPHIC: u16 = 0x0EED;
/// How near a player has to come for a townsperson to greet them.
const GREET_RANGE: u32 = 4;
/// The muted grey the client draws townsfolk chatter in.
pub(crate) const GREET_HUE: u16 = 0x03B2;
/// The font a greeting is spoken in.
pub(crate) const GREET_FONT: u16 = 3;

/// How long between an NPC's beats, in ticks (~2s at 20Hz).
const BEAT_TICKS: u64 = 40;
/// How long a townsperson waits between greetings — long enough not to natter at
/// someone standing at the counter.
const GREET_COOLDOWN: u64 = 15 * 20;
/// The chance, in a hundred, that an idle NPC drifts a step this beat.
const WANDER_CHANCE: u32 = 35;

/// The greeting lines a banker picks from, one fresh each time so it does not
/// repeat itself. `{name}` is filled with the visitor's name.
const GREETINGS: &[&str] = &[
    "Greetings, {name}. Say 'bank' and I shall open thy box.",
    "Well met, {name}. Thy account is safe with me — just say 'bank'.",
    "Ah, {name}! Come to see thy gold? Say 'bank'.",
    "A good day to thee, {name}. Say 'bank' or 'balance' as thou wilt.",
    "Welcome to the bank, {name}. How may I serve thee?",
    "{name}! Always a pleasure. Say 'bank' to open thy box.",
];

/// The lines a banker greets a nameless soul with.
const GREETINGS_ANON: &[&str] = &[
    "Greetings, traveller. Say 'bank' to open thy box.",
    "Welcome to the bank. Say 'bank', and I shall serve thee.",
    "Well met. Bank with me — just say the word.",
];

/// Personal names a generated banker draws from; the title "the banker" follows.
const PERSONAL_NAMES: &[&str] = &[
    "Alanna",
    "Bartholomew",
    "Cedric",
    "Damaris",
    "Edmund",
    "Fenwick",
    "Gwendolyn",
    "Halbert",
    "Isolde",
    "Joric",
    "Katrisha",
    "Lucan",
    "Merrick",
    "Nesta",
    "Osric",
    "Perrin",
    "Rowena",
    "Selwyn",
    "Talia",
    "Ulric",
    "Vesper",
    "Willow",
];

/// A generated banker's full name: a personal name and the title townsfolk wear,
/// e.g. "Rowena the banker". Uses the world's seeded generator, so a replay names
/// the same banker the same.
pub fn banker_name(rng: &mut Rng) -> String {
    let name = PERSONAL_NAMES[rng.below(PERSONAL_NAMES.len() as u32) as usize];
    format!("{name} the banker")
}

/// Answer a banker's keywords for a speaking player, if one is in reach. "bank"
/// opens the box; "balance" reports the gold in it. A banker has to be within
/// [`BANK_RANGE`] — the service is the townsperson's, not the word's.
pub fn banker_keywords(
    state: &mut WorldState,
    connection: ConnectionId,
    actor: EntityId,
    text: &str,
) {
    let lower = text.to_lowercase();
    let wants = |word: &str| lower.split(|c: char| !c.is_alphabetic()).any(|w| w == word);
    if !wants("bank") && !wants("balance") {
        return;
    }
    if !banker_in_reach(state, actor) {
        return;
    }
    if wants("balance") {
        let gold = bank_gold(state, actor);
        notify(
            state,
            connection,
            &format!("Thy bank box holds {gold} gold."),
        );
    }
    if wants("bank") {
        openshard_items::open_worn_container(state, connection, actor, BANK_LAYER);
    }
}

/// One tick of townsfolk life: every NPC due a beat greets a nearby visitor and
/// turns to face them, or takes an idle step near its home. Returns the steps it
/// wants — `(serial, direction)` — for the tick to apply through its own
/// terrain-checked `step`. Everything else is done here on the world.
#[must_use]
pub fn live(state: &mut WorldState) -> Vec<(u32, u8)> {
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
        if let Some(mut n) = state.registry.get::<Npc>(npc).copied() {
            n.next_beat = now + BEAT_TICKS;
            state.registry.insert(npc, n);
        }
        let Some(&Position(at)) = state.registry.get::<Position>(npc) else {
            continue;
        };
        let facet = state.facet_of(npc);

        // Someone close? Greet and face them (bankers only, for now), and stand
        // still this beat — you do not wander off mid-hello.
        if let Some((visitor, visitor_at)) = nearest_player(state, facet, at, GREET_RANGE) {
            if try_greet(state, npc, at, visitor, visitor_at, now) {
                continue;
            }
        }

        // Otherwise drift near home.
        if let Some(dir) = wander_step(state, npc, at) {
            if let Some(serial) = state.registry.serial_of(npc) {
                steps.push((serial.raw(), dir));
            }
        }
    }
    steps
}

/// Greet and face a visitor if this NPC is a banker whose cooldown has passed.
/// Returns whether it greeted (and so should not also wander this beat).
fn try_greet(
    state: &mut WorldState,
    npc: EntityId,
    at: Point,
    visitor: EntityId,
    visitor_at: Point,
    now: u64,
) -> bool {
    // Only bankers have something to say yet; a plain townsperson just stands.
    let Some(banker) = state.registry.get::<Banker>(npc).copied() else {
        return false;
    };
    if now < banker.next_greet {
        return false;
    }
    // Turn to face them, and let watchers see the turn.
    if let Some(dir) = openshard_ai::direction_toward(at, visitor_at) {
        state.registry.insert(npc, Heading(Facing::walking(dir)));
        state.broadcast_move(npc);
    }
    // A line chosen fresh, by name when the visitor has one.
    let name = state.registry.get::<Name>(visitor).map(|n| n.0.clone());
    let line = match name {
        Some(name) => {
            let pick = state.rng.below(GREETINGS.len() as u32) as usize;
            GREETINGS[pick].replace("{name}", &name)
        }
        None => {
            let pick = state.rng.below(GREETINGS_ANON.len() as u32) as usize;
            GREETINGS_ANON[pick].to_owned()
        }
    };
    openshard_chat::speak(state, npc, 0, GREET_HUE, GREET_FONT, &line);
    state.registry.insert(
        npc,
        Banker {
            next_greet: now + GREET_COOLDOWN,
        },
    );
    true
}

/// An idle step for an NPC: head home when it has strayed to its range, else drift
/// a random direction now and then. `None` means stand still this beat. The tile
/// is not checked here — the tick's `step` validates it against the terrain, and a
/// step into a wall simply turns the NPC.
fn wander_step(state: &mut WorldState, npc: EntityId, at: Point) -> Option<u8> {
    let Npc { home, wander, .. } = *state.registry.get::<Npc>(npc)?;
    if wander == 0 {
        return None;
    }
    let strayed = chebyshev(at, home) >= u32::from(wander);
    if strayed {
        // Back toward the post — pathed around the counter, not into it. A
        // townsperson is human: a shut door on the way home is opened, not an
        // obstacle (the auto-close swings it shut again behind them).
        let facet = state.facet_of(npc);
        let dir = openshard_ai::step_toward(state, facet, at, home, true)?;
        if let Some(tile) = openshard_movement::step_from(at, Direction::from_bits(dir)) {
            let door = state
                .facet_state(facet)
                .live_terrain()
                .blocker_at(tile.x, tile.y)
                .filter(|o| o.door)
                .map(|o| o.entity);
            if let Some(door) = door {
                openshard_items::open_door(state, door);
                return None;
            }
        }
        Some(dir)
    } else if state.rng.below(100) < WANDER_CHANCE {
        // A small idle drift — one of the eight directions (wire bytes 0..8).
        Some(state.rng.below(8) as u8)
    } else {
        None
    }
}

/// The nearest player to `at` within `range` on `facet`, and where it stands.
fn nearest_player(
    state: &WorldState,
    facet: u8,
    at: Point,
    range: u32,
) -> Option<(EntityId, Point)> {
    state
        .players
        .values()
        .filter_map(|&entity| {
            let pos = state.registry.get::<Position>(entity)?.0;
            (state.facet_of(entity) == facet && in_range(pos, at, range)).then_some((entity, pos))
        })
        .min_by_key(|(_, pos)| squared_distance(*pos, at))
}

/// Whether a banker stands within [`BANK_RANGE`] of `actor`, on its facet.
fn banker_in_reach(state: &WorldState, actor: EntityId) -> bool {
    let Some(&Position(at)) = state.registry.get::<Position>(actor) else {
        return false;
    };
    let facet = state.facet_of(actor);
    state.registry.query::<Banker>().any(|(banker, _)| {
        state.facet_of(banker) == facet
            && state
                .registry
                .get::<Position>(banker)
                .is_some_and(|p| in_range(p.0, at, BANK_RANGE))
    })
}

/// The gold in a mobile's bank box — the amounts of every gold pile inside it.
fn bank_gold(state: &WorldState, actor: EntityId) -> u32 {
    let Some(owner) = state.registry.serial_of(actor) else {
        return 0;
    };
    let Some(bank) = state
        .registry
        .query::<Equipped>()
        .find(|(item, eq)| {
            eq.mobile == owner && eq.layer == BANK_LAYER && state.registry.has::<Container>(*item)
        })
        .and_then(|(item, _)| state.registry.serial_of(item))
    else {
        return 0;
    };
    state
        .registry
        .query::<Contained>()
        .filter(|(item, held)| {
            held.container == bank
                && state
                    .registry
                    .get::<Graphic>(*item)
                    .is_some_and(|g| g.id == GOLD_GRAPHIC)
        })
        .map(|(item, _)| u32::from(state.registry.get::<Amount>(item).map_or(1, |a| a.0)))
        .sum()
}

/// Send a private system line to a connection — a `0x1C` from the system serial,
/// the "the bank says" reply a keyword earns.
pub(crate) fn notify(state: &mut WorldState, connection: ConnectionId, text: &str) {
    let packet = encode_message(
        SYSTEM_SERIAL,
        0xFFFF,
        0,
        GREET_HUE,
        GREET_FONT,
        "System",
        text,
    );
    state.send(connection, packet);
}

/// Chebyshev distance — the square UO measures range in.
fn chebyshev(a: Point, b: Point) -> u32 {
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
    fn a_generated_banker_name_carries_the_title_and_replays() {
        // The name draws from the world's seeded generator, so the same seed names
        // the same banker — a shard replays its townsfolk, titles and all.
        let mut a = Rng::new(0x51ED);
        let mut b = Rng::new(0x51ED);
        let name = banker_name(&mut a);
        assert!(
            name.ends_with(" the banker"),
            "the title follows the name: {name}"
        );
        assert!(
            name.len() > " the banker".len(),
            "a personal name precedes it"
        );
        assert_eq!(
            name,
            banker_name(&mut b),
            "the same seed names the same banker"
        );
    }

    #[test]
    fn every_greeting_line_asks_to_bank() {
        // However the line varies, it always tells the visitor the word that opens
        // the box — the point of the greeting, not just flavour.
        for line in GREETINGS.iter().chain(GREETINGS_ANON) {
            assert!(
                line.to_lowercase().contains("bank"),
                "a greeting should mention banking: {line}"
            );
        }
    }

    #[test]
    fn chebyshev_is_the_square_uo_measures() {
        assert_eq!(chebyshev(Point::new(0, 0, 0), Point::new(3, 1, 0)), 3);
        assert_eq!(chebyshev(Point::new(5, 5, 0), Point::new(5, 5, 0)), 0);
    }
}
