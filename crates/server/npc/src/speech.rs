//! What a townsperson answers, and when.
//!
//! # The mechanism is ServUO's; the words are data
//!
//! `VendorAI.OnSpeech` is the shape: an NPC overhears whatever is said near it,
//! looks for keywords, and answers — and `HandlesOnSpeech` bounds it to four
//! tiles, because a shopkeeper across the square has no business replying. That is
//! ported here whole. What ServUO does *not* have is a line for every trade: a
//! `BaseVendor`'s entire vocabulary is cliloc 500186 ("Greetings.  Have a look
//! around.") and 501522 ("I shall not treat with scum like thee!").
//!
//! So this file holds those two and a generic greeting — what a trade with no
//! table falls back to — and the per-trade lines are content, keyed by the NPC's
//! [`Title`]. They live in `state/data/speech.json` and reach the world through
//! `server::content`, the same way quests do.
//!
//! # Keywords are whole words
//!
//! The shop keywords used to be a substring test on the whole line, which meant
//! "that sword is unsellable" opened a buy-back list. ServUO matches *keyword
//! ids* the client encodes; matching whole words is the closest honest equivalent
//! without a cliloc keyword table, and it is what [`SpeechEntry::matches`] does.
//!
//! # Named, or not
//!
//! ServUO distinguishes two ways to reach a vendor: `vendor buy`/`vendor sell`
//! work on whoever is nearest, and a bare `buy`/`sell` works only when the vendor
//! was *named* in the sentence (`BaseAI.WasNamed`). That is why saying "sell" in a
//! crowded bank does not open four shops at once, and it is ported here.

use openshard_entities::EntityId;
use openshard_gateway::ConnectionId;
use openshard_protocol::mobile::Notoriety;
use openshard_state::components::{
    Escortable,
    Name,
    Npc,
    Position,
    Title,
    Vendor,
};
use openshard_state::sectors::in_range;
use openshard_state::{
    SpeechEntry,
    WorldState,
};

use crate::live::GREET_RANGE;

/// ServUO cliloc 500186, a vendor's own greeting.
const VENDOR_GREETING: &str = "Greetings.  Have a look around.";
/// ServUO cliloc 501522: what a shopkeeper tells a criminal.
pub(crate) const REFUSE_SCUM: &str = "I shall not treat with scum like thee!";
/// What a shopkeeper says to someone trying to trade after hours. Ours — neither
/// reference has a closing time — so it is a plain line rather than a cliloc
/// number invented to look like one.
pub(crate) const CLOSED_FOR_THE_NIGHT: &str = "The shop is closed. Come back in the morning.";

/// What a townsperson says instead of a greeting once the day is over. Also ours,
/// and deliberately few: a night line is atmosphere, and the pack can register its
/// own per trade the same way it registers the day's.
const NIGHT_GREETINGS: &[&str] = &[
    "Good evening to thee.",
    "A late hour to be abroad.",
    "The day is done. Rest well.",
];

/// The greeting a trade with no registered table falls back to. Deliberately
/// bland: a bare shard should read as unfinished, not as wrong.
const DEFAULT_GREETINGS: &[&str] = &[
    "Greetings, {name}.",
    "Well met, {name}.",
    "Good day to thee, {name}.",
];

/// The same, for a visitor with no name.
const DEFAULT_GREETINGS_ANON: &[&str] = &["Greetings.", "Well met.", "Good day to thee."];

/// The line an NPC greets `visitor` with, or `None` if it has nothing to say.
///
/// The trade's registered greetings win; a vendor with none falls back to ServUO's
/// own 500186; anything else to a bland default. `{name}` is filled with the
/// visitor's name, and a line that needs one is skipped for a nameless visitor.
pub(crate) fn greeting_for(state: &mut WorldState, npc: EntityId, visitor: EntityId) -> Option<String> {
    let visitor_name = state.registry.get::<Name>(visitor).map(|n| n.0.clone());

    // A traveller waiting for an escort asks for one, ahead of anything its trade
    // would otherwise say — ServUO's `BaseEscortable.OnMovement`, which is what makes
    // the sixty of them scattered across Felucca findable at all. Only while it is
    // *unescorted*: one already being led has nothing to ask for.
    if let Some(escort) = state.registry.get::<Escortable>(npc) {
        if escort.escorter.is_none() {
            return Some(match escort.destination.as_str() {
                // A pack may leave the destination for the quest to choose when
                // someone accepts, in which case there is no place to name yet.
                "" => "I am looking for an escort. Wilt thou take me?".to_owned(),
                to => format!("I am looking to go to {to}, will you take me?"),
            });
        }
    }

    // After hours the trade has nothing to sell and nothing to say about it, so
    // the shopkeeper's own line gives way to a civil good evening. Ahead of the
    // registered table, because a table keyed on the trade is a *daytime* answer:
    // "have a look around" from a shop that is shut reads worse than silence.
    if !crate::live::working_hours(state) {
        let pool: Vec<String> = NIGHT_GREETINGS.iter().map(|s| (*s).to_owned()).collect();
        let line = pick(state, &pool)?;
        return Some(fill_name(&line, visitor_name.as_deref()));
    }

    let registered = table_lines(state, npc, |t| &t.greetings);

    let line = if let Some(lines) = registered {
        pick(state, &lines)?
    } else if state.registry.has::<Vendor>(npc) {
        VENDOR_GREETING.to_owned()
    } else {
        let pool: Vec<String> = match &visitor_name {
            Some(_) => DEFAULT_GREETINGS.iter().map(|s| (*s).to_owned()).collect(),
            None => DEFAULT_GREETINGS_ANON.iter().map(|s| (*s).to_owned()).collect(),
        };
        pick(state, &pool)?
    };
    Some(fill_name(&line, visitor_name.as_deref()))
}

/// An idle remark for this NPC, or `None` when its trade registered none. There is
/// no core default: an invented bark is worse than a quiet street.
pub(crate) fn bark_line(state: &mut WorldState, npc: EntityId) -> Option<String> {
    let lines = table_lines(state, npc, |t| &t.barks)?;
    pick(state, &lines)
}

/// Answer whatever a player just said, for every townsperson in earshot.
///
/// The one entry point the tick calls. Runs after the words have already been
/// spoken and shown, so it reads as a question the town answers rather than a
/// hidden command — the shape `banker_keywords` and `guard_keywords` established.
///
/// Shop keywords are handled here too, and this is where the old substring test
/// lived: see [`shop_request`].
pub fn overhear(state: &mut WorldState, connection: ConnectionId, actor: EntityId, text: &str) {
    let lowered = text.to_lowercase();
    let words: Vec<&str> = lowered
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|w| !w.is_empty())
        .collect();
    if words.is_empty() {
        return;
    }

    // A shop first, because "buy" and "sell" are actions and not conversation; a
    // trade whose table also lists them would otherwise answer instead of opening.
    if shop_request(state, connection, actor, &words) {
        return;
    }

    // Then the trades in earshot, nearest first, and only the nearest one that has
    // something to say answers — a chorus of five shopkeepers reciting the same
    // line is worse than one.
    for npc in listeners(state, actor) {
        if answer(state, npc, &words) {
            return;
        }
    }
}

/// Open a shop if the sentence asked for one. Returns whether it did.
///
/// ServUO's `VendorAI.OnSpeech`: `vendor buy`/`vendor sell` reach the nearest
/// shopkeeper unqualified, and a bare `buy`/`sell` only when that shopkeeper was
/// named. Checked in that order, and "sell" before "buy" so neither steals the
/// other.
fn shop_request(state: &mut WorldState, connection: ConnectionId, actor: EntityId, words: &[&str]) -> bool {
    let addressed = |verb: &str| {
        // "vendor sell" — no name needed.
        words.windows(2).any(|w| w == ["vendor", verb])
            // or a bare "sell", if a shopkeeper in earshot was named.
            || (words.contains(&verb) && named_vendor(state, actor, words))
    };
    if addressed("sell") {
        return crate::offer_sell_list(state, connection, actor);
    }
    if addressed("buy") {
        return crate::buy_keyword(state, connection, actor);
    }
    false
}

/// Whether a shopkeeper in earshot was named in the sentence — ServUO's
/// `BaseAI.WasNamed`, which matches any word of the NPC's name so "Rowena buy"
/// works on "Rowena the blacksmith".
fn named_vendor(state: &WorldState, actor: EntityId, words: &[&str]) -> bool {
    listeners(state, actor).into_iter().any(|npc| {
        state.registry.has::<Vendor>(npc)
            && state.registry.get::<Name>(npc).is_some_and(|name| {
                name.0
                    .split_whitespace()
                    .filter(|part| !part.eq_ignore_ascii_case("the"))
                    .any(|part| words.iter().any(|w| w.eq_ignore_ascii_case(part)))
            })
    })
}

/// Have one NPC answer, if any of its trade's keywords matched. Returns whether it
/// spoke. A trade with a `fallback` answers even an unmatched sentence, which is
/// how ServUO's talking NPCs read; a trade without one stays quiet.
fn answer(state: &mut WorldState, npc: EntityId, words: &[&str]) -> bool {
    let Some(title) = state.registry.get::<Title>(npc).map(|t| t.0.clone()) else {
        return false;
    };
    let Some(table) = state.dialogue.table(&title) else {
        return false;
    };
    let matched: Option<Vec<String>> = table
        .entries
        .iter()
        .find(|entry: &&SpeechEntry| entry.matches(words))
        .map(|entry| entry.lines.clone());
    let fallback = table.fallback.clone();

    let line = match matched {
        Some(lines) => pick(state, &lines),
        None => fallback,
    };
    match line {
        Some(line) => {
            crate::say(state, npc, &line);
            true
        }
        None => false,
    }
}

/// Whether this mobile may trade at all — ServUO's `BaseVendor.CheckVendorAccess`,
/// plus the shard's opening hours. Either refusal is spoken out loud.
///
/// # Why both live here
///
/// This is called at all four doors into a shop — opening it, buying, offering
/// the sell list, and selling — because a client with the buy window already up
/// can still send a `0x3B` after the door was shut behind it. Adding a second
/// predicate beside it at four sites is three chances to add it at three.
///
/// The closing hour is **ours**, marked as such: neither reference shuts a shop,
/// and it rides on `gameplay.npc_schedule` so a shard that has not asked for a
/// daily routine has no closing time either. It matters more than flavour —
/// a vendor's stock crate is worn, so the shop is wherever the shopkeeper is
/// standing, and a shopkeeper that has walked off for the night should not still
/// be selling from wherever it ended up.
#[must_use]
pub fn check_vendor_access(state: &mut WorldState, vendor: EntityId, buyer: EntityId) -> bool {
    if !crate::live::working_hours(state) {
        crate::say(state, vendor, CLOSED_FOR_THE_NIGHT);
        return false;
    }
    // ServUO refuses a criminal outright. The grey flag and the red standing are
    // the two ways to be one here, and both are read from the same place every
    // other "who may do what to whom" rule reads it.
    let standing = state.notoriety_of(buyer);
    if standing != Notoriety::Criminal && standing != Notoriety::Murderer {
        return true;
    }
    crate::say(state, vendor, REFUSE_SCUM);
    false
}

/// The townsfolk close enough to hear `actor`, nearest first.
fn listeners(state: &WorldState, actor: EntityId) -> Vec<EntityId> {
    let Some(&Position(at)) = state.registry.get::<Position>(actor) else {
        return Vec::new();
    };
    let facet = state.facet_of(actor);
    let mut found: Vec<(u32, EntityId)> = state
        .registry
        .query::<Npc>()
        .filter_map(|(npc, _)| {
            let pos = state.registry.get::<Position>(npc)?.0;
            (state.facet_of(npc) == facet && in_range(pos, at, GREET_RANGE))
                .then(|| (crate::live::chebyshev(pos, at), npc))
        })
        .collect();
    found.sort_by_key(|&(distance, _)| distance);
    found.into_iter().map(|(_, npc)| npc).collect()
}

/// The lines one field of an NPC's registered table holds, or `None` when its
/// trade has no table or the field is empty.
fn table_lines(
    state: &WorldState,
    npc: EntityId,
    field: impl Fn(&openshard_state::SpeechTable) -> &Vec<String>,
) -> Option<Vec<String>> {
    let title = state.registry.get::<Title>(npc)?;
    let table = state.dialogue.table(&title.0)?;
    let lines = field(table);
    (!lines.is_empty()).then(|| lines.clone())
}

/// One of `lines`, on the world's seeded generator so a town replays.
fn pick(state: &mut WorldState, lines: &[String]) -> Option<String> {
    if lines.is_empty() {
        return None;
    }
    let index = state.rng.below(lines.len() as u32) as usize;
    lines.get(index).cloned()
}

/// Fill a line's `{name}` with the visitor's, or drop the placeholder when there
/// is none — never leave `{name}` on a client's screen.
fn fill_name(line: &str, name: Option<&str>) -> String {
    match name {
        Some(name) => line.replace("{name}", name),
        None => line.replace("{name}", "traveller"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_placeholder_never_reaches_the_client() {
        // A `{name}` on screen is the kind of bug a player screenshots.
        assert_eq!(fill_name("Hail, {name}!", Some("Rowena")), "Hail, Rowena!");
        assert_eq!(fill_name("Hail, {name}!", None), "Hail, traveller!");
        assert_eq!(fill_name("Hail!", None), "Hail!");
    }

    #[test]
    fn every_default_greeting_survives_a_nameless_visitor() {
        for line in DEFAULT_GREETINGS.iter().chain(DEFAULT_GREETINGS_ANON) {
            assert!(!fill_name(line, None).contains("{name}"), "{line}");
        }
    }

    #[test]
    fn the_anonymous_greetings_ask_for_no_name() {
        // The pool picked for a nameless visitor must not need substitution at all,
        // or "Greetings, traveller" reads as a fallback rather than a greeting.
        for line in DEFAULT_GREETINGS_ANON {
            assert!(!line.contains("{name}"), "{line}");
        }
    }
}
