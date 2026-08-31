//! Townsfolk: the people who make a town a place with people in it rather than a
//! set of props.
//!
//! # Why a crate and not more of `tick.rs`
//!
//! `world/tick.rs` is orchestration — the command dispatch, the system order, the
//! movement machinery. Rules do not go there. Townsfolk behaviour — the service a
//! banker offers, the words a trade answers, the little life that turns it to face
//! you and shuffles it near its post — is a gameplay domain, so it is a `fn(&mut
//! WorldState)` here, the same shape `combat`, `chat` and `skills` use. The
//! components it hangs on ([`Npc`], `Banker`, `Title`) live in `state`.
//!
//! # A townsperson is a trade, a look, a beat and a table
//!
//! Four modules, one each:
//!
//! - [`dress`] — what it looks like. ServUO's `BaseVendor.InitBody`/`InitOutfit`:
//!   a gender, a skin hue, hair, a beard, a shirt, trousers or a skirt, shoes.
//! - [`names`] — who it is. A personal name in front of the trade it was spawned
//!   with, so a town is not thirty-eight people called "the banker".
//! - [`live`] — the beat. Greet, face a customer, bark, drift near the post.
//! - [`speech`] — what it answers. ServUO's `VendorAI.OnSpeech`, over the keyword
//!   table `state/data/speech.json` defines per trade.
//!
//! All four are keyed off one thing: the `Title` it is spawned with.
//!
//! The randomness they spend is the world's seeded [`Rng`], never the OS, so a
//! shard populates the same town twice — which is what makes a populated Felucca
//! reproducible at all.

use openshard_entities::EntityId;
use openshard_gateway::ConnectionId;
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::speech::{
    Font,
    SpokenMessage,
    TalkMode,
};
use openshard_protocol::wire::{
    Graphic,
    Hue,
};
use openshard_state::WorldState;
use openshard_state::components::{
    Banker,
    Position,
};
use openshard_state::sectors::in_range;

pub mod dress;
mod guards;
pub mod live;
pub mod names;
mod pets;
mod spawn;
mod speech;
mod vendor;
pub use dress::{
    Appearance,
    FIXED_LAYERS,
    ShoeType,
    dress_townsperson,
};
pub use guards::{
    call_guards,
    expire_guards,
    guard_keywords,
    hunt_with_guards,
};
pub use live::{
    BEAT_JITTER_FRACTION,
    BEAT_TICKS,
    beat_jitter,
    first_beat,
    live,
    next_beat,
};
pub use names::{
    personal_name,
    townsperson_name,
};
pub use pets::{
    hear_pet_order,
    tame,
};
pub use spawn::{
    MobileSpawned,
    SpawnSpec,
    spawn,
};
pub use speech::{
    check_vendor_access,
    overhear,
};
pub use vendor::{
    RESTOCK_TICKS,
    STOCK_LAYER,
    StockLine,
    buy,
    buy_keyword,
    offer_sell_list,
    open_shop,
    sell,
    stock,
};

/// The bank box graphic and gump — ServUO's `BankBox` on `Layer.Bank`. A
/// character wears one; a banker opens it. Exported so the world equips it on the
/// same layer this crate opens.
pub const BANK_GRAPHIC: Graphic = Graphic(0x0E7C);
/// The bank box gump.
pub const BANK_GUMP: Graphic = Graphic(0x004A);
/// The bank layer, `Layer.Bank`. Defined by `items`, which also has to know where
/// a character's own load stops; re-exported here so a banker still reads as
/// owning its own box.
pub use openshard_items::BANK_LAYER;

/// How near a banker a player must be for "bank" to open the box — ServUO's 12.
const BANK_RANGE: u32 = 12;
/// The gold-coin graphic — `items`', so a banker counts the same coin the status
/// bar weighs and a corpse drops.
pub(crate) use openshard_items::GOLD_GRAPHIC;
/// The muted grey the client draws townsfolk chatter in — [`Hue::NPC_SPEECH`],
/// shared with every other crate that gives an NPC a voice.
pub(crate) const GREET_HUE: Hue = Hue::NPC_SPEECH;
/// The font a greeting is spoken in.
pub(crate) const GREET_FONT: Font = Font::DEFAULT;

/// Have an NPC say something out loud, in the muted grey and font the client draws
/// townsfolk chatter in. The one door every townsperson's speech goes through, so
/// the hue and the font are decided once — the greeters, the vendor's answers
/// (`vendor::vendor_says`) and a guard's sentence (`guards::execute`) all come
/// through here. The latter two called `openshard_chat::speak` directly for a
/// while, with arguments that happened to match; that is the shape this door
/// exists to prevent, because it drifts without anything failing.
///
/// A caller that wants a *different* hue is not a townsperson speaking and should
/// not be routed through here — the private "the bank says" line uses [`notify`],
/// and a script's `Speak` names its own hue.
pub(crate) fn say(state: &mut WorldState, npc: EntityId, line: &str) {
    openshard_chat::speak(state, npc, TalkMode::Regular, GREET_HUE, GREET_FONT, line);
}

/// Answer a banker's keywords for a speaking player, if one is in reach. "bank"
/// opens the box; "balance" reports the gold in it. A banker has to be within
/// [`BANK_RANGE`] — the service is the townsperson's, not the word's.
pub fn banker_keywords(state: &mut WorldState, connection: ConnectionId, actor: EntityId, text: &str) {
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
        notify(state, connection, &format!("Thy bank box holds {gold} gold."));
    }
    if wants("bank") {
        openshard_items::open_worn_container(state, connection, actor, BANK_LAYER);
    }
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

/// The gold in a mobile's bank box — `items`', because the status bar and a
/// vendor's bank payment count the same coins the banker reports, and three
/// copies of "what is in the box" would drift.
use openshard_items::banked_gold as bank_gold;

/// Send a private system line to a connection — a `0x1C` from the system serial,
/// the "the bank says" reply a keyword earns.
pub(crate) fn notify(state: &mut WorldState, connection: ConnectionId, text: &str) {
    let packet = ServerPacket::SpokenMessage(SpokenMessage {
        serial:  None, // the system talking, not the banker
        graphic: None,
        mode:    TalkMode::Regular,
        // The system's hue, not [`GREET_HUE`]: this line carries no serial and
        // names itself "System", so it is the shard talking and not the banker.
        // It read `GREET_HUE` while the two names shared a value, which is the
        // confusion splitting them was for.
        hue:     Hue::SYSTEM,
        font:    GREET_FONT,
        name:    "System".to_owned(),
        text:    text.to_owned(),
    });
    state.send_packet(connection, &packet);
}

#[cfg(test)]
mod tests {
    use openshard_state::Rng;

    use super::*;

    #[test]
    fn a_banker_keyword_is_a_word_and_not_a_substring() {
        // The split is what stops "bankrupt" or "unbalanced" opening a bank box.
        // Exercised through the same closure `banker_keywords` uses.
        let lower = "i am bankrupt and my ledger is unbalanced".to_lowercase();
        let wants = |word: &str| lower.split(|c: char| !c.is_alphabetic()).any(|w| w == word);
        assert!(!wants("bank"));
        assert!(!wants("balance"));

        let lower = "bank, please".to_lowercase();
        let wants = |word: &str| lower.split(|c: char| !c.is_alphabetic()).any(|w| w == word);
        assert!(wants("bank"));
    }

    #[test]
    fn a_banker_is_named_after_a_person_not_after_its_trade() {
        // Thirty-eight NPCs called "the banker" is what the pack shipped before
        // the trade and the person became two halves of one label.
        let mut rng = Rng::new(0x51ED);
        let name = townsperson_name(&mut rng, "the banker", false);
        assert!(name.ends_with(" the banker"), "{name}");
        assert_ne!(name, "the banker");
    }
}
