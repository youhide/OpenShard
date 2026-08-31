//! What a townsperson says: the keyword table a trade answers by.
//!
//! Shared substrate, not rules. The `npc` crate decides *when* an NPC speaks and
//! to whom; this is only the data it reads, so it lives below it the way
//! [`QuestDefs`](crate::QuestDefs) lives below the quest system.
//!
//! # Where the content comes from
//!
//! ServUO gives the *mechanism* — `VendorAI.OnSpeech` matches shop keywords within
//! four tiles, `HandlesOnSpeech` gates it, and XmlSpawner's `XmlDialog` attachment
//! is the keyword-and-response engine for talking NPCs — but almost no
//! per-profession content: a vendor's whole vocabulary is cliloc 500186
//! ("Greetings.  Have a look around.") and 501522 ("I shall not treat with scum
//! like thee!"). Two lines for sixty-eight trades, so the rest is written rather
//! than ported.
//!
//! It is `data/speech.json`, compiled by `build.rs` into [`shipped`] — content in
//! the tree, the way [`quest::shipped`](crate::quest::shipped) is. The engine
//! keeps a bare default beside it and not instead of it: `npc::speech`'s
//! `DEFAULT_GREETINGS` is what a trade with no table falls back to, so a shard
//! that deletes every row here still has townsfolk who answer.
//!
//! # Keyed by the trade string, never by an index
//!
//! The key is the [`Title`](crate::components::Title) the NPC is spawned with —
//! "the blacksmith". An index would silently move every line in the table the day
//! someone reorders the list, and unlike a quest there is nothing in a save to
//! notice it went wrong.

use std::collections::HashMap;

/// Everything one trade says.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct SpeechTable {
    /// Lines it greets an approaching player with. `{name}` is replaced with the
    /// visitor's name; a line without it is used for a nameless visitor too.
    pub greetings: Vec<String>,
    /// What it says to itself when nobody is near — ambient colour. Empty is
    /// silence, which is the right default for most trades.
    pub barks:     Vec<String>,
    /// Keyword groups and the answers to them. The first group with a match wins,
    /// so the order they are written in is their precedence — a specific keyword
    /// goes above a general one.
    pub entries:   Vec<SpeechEntry>,
    /// What it says when spoken to and nothing matched. `None` stays quiet, which
    /// is better than a shopkeeper answering every passing conversation.
    pub fallback:  Option<String>,
}

/// One keyword group and its answers.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct SpeechEntry {
    /// The words that trigger it, lowercase. Matched as whole words against what
    /// was said — a substring match is how "unsellable" opened a shop.
    pub keywords: Vec<String>,
    /// The answers, one picked at random so an NPC asked twice does not repeat.
    pub lines:    Vec<String>,
}

impl SpeechEntry {
    /// Whether `words` — already lowercased and split — contains any of the
    /// keywords. A multi-word keyword ("vendor buy") matches as a run.
    #[must_use]
    pub fn matches(&self, words: &[&str]) -> bool {
        self.keywords.iter().any(|keyword| {
            let wanted: Vec<&str> = keyword.split_whitespace().collect();
            match wanted.len() {
                0 => false,
                1 => words.contains(&wanted[0]),
                n => words.windows(n).any(|window| window == wanted.as_slice()),
            }
        })
    }
}

/// Every trade's speech, by key.
///
/// Replaced wholesale, like [`QuestDefs`](crate::QuestDefs): registration re-runs
/// from the top, and merging would leave lines that were deleted still being
/// spoken.
///
/// # It used to hold the personal names too, and nothing read them
///
/// There were two more fields here, `male_names` and `female_names`, filled by a
/// `set_names` the script pack called with ServUO's 1,500 and 2,132. Nothing ever
/// read them. `npc::names` documented `npc::speech::registered_name` as the
/// function that would, and that function was never written, so every townsperson
/// in Felucca was named from `npc/data/names.json` throughout — the override was
/// dead the whole time it looked like policy.
///
/// So the names have one home, `npc/data/names.json`, and it is not a default
/// waiting to be overridden. ServUO's full lists stay out for the reason
/// `npc::names` gives: they are the operator's `Data/names.xml`, the same rule
/// that keeps client files out of this repository.
#[derive(Clone, Default, Debug)]
pub struct Dialogue {
    tables: HashMap<String, SpeechTable>,
}

impl Dialogue {
    /// Replace every trade's table.
    pub fn set_tables(&mut self, tables: HashMap<String, SpeechTable>) {
        self.tables = tables;
    }

    /// The table for a trade, if one is defined.
    #[must_use]
    pub fn table(&self, title: &str) -> Option<&SpeechTable> {
        self.tables.get(title)
    }

    /// Whether any trade has a table.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tables.is_empty()
    }
}

include!(concat!(env!("OUT_DIR"), "/speech.rs"));

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(keywords: &[&str]) -> SpeechEntry {
        SpeechEntry {
            keywords: keywords.iter().map(|k| (*k).to_owned()).collect(),
            lines:    vec!["aye".to_owned()],
        }
    }

    #[test]
    fn a_keyword_matches_a_whole_word_and_not_a_substring() {
        // The bug this replaces: "sell" was matched as a substring, so saying
        // "that sword is unsellable" opened a shopkeeper's buy-back list.
        let e = entry(&["sell"]);
        assert!(e.matches(&["i", "wish", "to", "sell"]));
        assert!(!e.matches(&["unsellable"]));
        assert!(!e.matches(&["seller"]));
    }

    #[test]
    fn a_multi_word_keyword_matches_only_in_order() {
        // ServUO's `vendor buy` keyword is two words and means nothing apart.
        let e = entry(&["vendor buy"]);
        assert!(e.matches(&["vendor", "buy"]));
        assert!(e.matches(&["hey", "vendor", "buy", "please"]));
        assert!(!e.matches(&["buy", "vendor"]));
        assert!(!e.matches(&["vendor"]));
    }

    #[test]
    fn an_empty_keyword_never_matches() {
        // A stray comma in the data would otherwise answer everything. `build.rs`
        // rejects one in `data/speech.json`; this is the guard for a table that
        // reached the engine some other way.
        assert!(!entry(&[""]).matches(&["anything"]));
    }

    #[test]
    fn a_table_is_replaced_wholesale_not_merged() {
        let mut dialogue = Dialogue::default();
        let mut first = HashMap::new();
        first.insert("the baker".to_owned(), SpeechTable::default());
        dialogue.set_tables(first);
        assert!(dialogue.table("the baker").is_some());

        let mut second = HashMap::new();
        second.insert("the smith".to_owned(), SpeechTable::default());
        dialogue.set_tables(second);
        assert!(
            dialogue.table("the baker").is_none(),
            "a reload must drop what the pack deleted"
        );
    }

    #[test]
    fn the_shipped_trades_are_keyed_by_a_title_an_npc_is_actually_spawned_with() {
        // The key is a `Title` and the lookup is exact, so a stray capital or a
        // trailing space is a table that can never be found. `build.rs` cannot
        // check this one — it is a fact about the other end of the rendezvous.
        let shipped = shipped();
        assert!(!shipped.is_empty(), "the shard ships no trade speech at all");
        for (title, _) in &shipped {
            assert_eq!(title.trim(), title, "{title:?} is padded");
            assert!(
                title.starts_with("the ") || title.starts_with("a "),
                "{title:?} does not read as a title an NPC wears"
            );
        }
    }

    #[test]
    fn every_shipped_keyword_can_be_matched_by_something_a_player_could_say() {
        // The pairing that makes the table work at all: `overhear` splits on
        // anything that is not alphanumeric or an apostrophe, so a keyword holding
        // punctuation is unreachable however it is spelled.
        for (title, table) in shipped() {
            for entry in &table.entries {
                for keyword in &entry.keywords {
                    let words: Vec<&str> = keyword.split_whitespace().collect();
                    assert!(
                        words
                            .iter()
                            .all(|word| word.chars().all(|c| c.is_alphanumeric() || c == '\'')),
                        "{title}: keyword {keyword:?} holds what a sentence is split on"
                    );
                    assert!(
                        entry.matches(&words),
                        "{title}: {keyword:?} does not match itself"
                    );
                }
            }
        }
    }
}
