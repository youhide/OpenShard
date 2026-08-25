//! What a quest *is*: the definition a shard writes, and the progress a player
//! makes against it.
//!
//! Shared substrate, not rules. The types here are read by the quest system that
//! offers and turns in a quest, by the gump that draws it, and by the persistence
//! that saves it — so they live below all three, the way [`Region`](crate::Region)
//! lives below the guards that read it.
//!
//! # Definitions are content; progress belongs to the player
//!
//! A [`QuestDef`] is content: a title, some objectives, some rewards. The shard's
//! own are [`shipped`] — `data/quests.json`, compiled in — and whoever registers
//! last replaces the lot, so a definition is never persisted: the content is the
//! source of truth for what a quest *is*, every boot. A [`QuestState`] is the
//! opposite: it is what one character has done, it is saved with them, and it
//! must survive the definitions being edited.
//!
//! That is why a quest is keyed by its **string**, never by its index.
//! Indices are how a saved "you have killed 3 of 5 rats" silently becomes progress
//! on a different quest the day someone reorders the list.
//!
//! The model is ServUO's `BaseQuest`/`BaseObjective`/`BaseReward`, field for
//! field, so that converting real quests later is transcription rather than
//! design.

use openshard_protocol::wire::{Graphic, Hue};
use std::collections::HashMap;

/// The stable identifier of a quest definition.
///
/// Quest keys cross content and persistence boundaries as strings, but inside
/// the world they must not be confused with an NPC name, region name, or other
/// arbitrary text.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct QuestKey(String);

impl QuestKey {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

/// What an objective asks for.
///
/// ServUO's objective classes, as one enum: the concrete list is small, closed,
/// and each variant is read in exactly one place, which is a worse fit for a trait
/// than for a match.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ObjectiveKind {
    /// Kill `count` creatures of a body. ServUO's `SlayObjective`.
    Slay {
        /// The body that counts. Matched against the victim's, so any creature
        /// drawn as this counts.
        body: Graphic,
    },
    /// Carry `count` of an item at once. ServUO's `ObtainObjective`.
    ///
    /// Counted from the backpack rather than announced, because nothing in the
    /// engine emits an event when an item changes hands — see the diffing pass in
    /// the quest system. Progress therefore goes *down* when the items are dropped,
    /// which is ServUO's behaviour too.
    Obtain {
        /// The item graphic that counts.
        graphic: Graphic,
    },
    /// Take `count` of an item to a named NPC. ServUO's `DeliverObjective`.
    Deliver {
        /// What to carry.
        graphic: Graphic,
        /// Who to take it to, by name. A name and not a serial: the destination is
        /// written before anything has been spawned, and a name still means the
        /// same thing after a restart.
        to: String,
    },
    /// Walk someone to a named region. ServUO's `EscortObjective`.
    Escort {
        /// The destination region's name, as `Regions` knows it.
        region: String,
    },
}

/// One thing a quest asks for.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ObjectiveDef {
    /// What it asks.
    pub kind: ObjectiveKind,
    /// How many. At least 1; an objective asking for none is complete on sight.
    pub count: u16,
    /// What to call the thing, in the gump — "sewer rat", "spiders' silk".
    pub name: String,
    /// How long the player has, in seconds. `0` is untimed, which is the norm.
    pub seconds: u32,
}

impl ObjectiveDef {
    /// Whether this objective runs against a clock.
    #[must_use]
    pub const fn timed(&self) -> bool {
        self.seconds > 0
    }
}

/// What a quest pays.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RewardKind {
    /// Coins into the backpack.
    Gold(u32),
    /// An item into the backpack.
    Item {
        /// Its graphic.
        graphic: Graphic,
        /// Its hue, or [`Hue`]`(0)`.
        hue: Hue,
        /// How many.
        amount: u16,
        /// Whether it merges onto a like pile.
        stackable: bool,
    },
}

/// One reward, with the name the gump shows for it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RewardDef {
    /// What the player gets.
    pub kind: RewardKind,
    /// What to call it in the rewards page.
    pub name: String,
}

/// A quest, as the content defines it.
///
/// The text fields are ServUO's, and each is shown at exactly one moment:
/// `description` when the quest is offered and in the log, `refuse` when it is
/// turned down, `uncomplete` when the giver is talked to before it is finished,
/// `complete` at turn-in, `failed` when a timer runs out.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct QuestDef {
    /// Its id, and the key a player's progress is saved under.
    pub key: QuestKey,
    /// The quest's name, in the log and the offer.
    pub title: String,
    /// What it asks, in prose.
    pub description: String,
    /// What the giver says when the offer is refused.
    pub refuse: String,
    /// What the giver says when the quest is in progress but not done.
    pub uncomplete: String,
    /// What the giver says on turn-in.
    pub complete: String,
    /// What is said when a timed objective runs out.
    pub failed: String,
    /// What it asks for.
    pub objectives: Vec<ObjectiveDef>,
    /// What it pays.
    pub rewards: Vec<RewardDef>,
    /// Whether *every* objective must be met (ServUO's `AllObjectives`), or any
    /// one of them is enough.
    pub all_objectives: bool,
    /// Whether it can only ever be done once by a character.
    pub done_once: bool,
    /// How long before it may be taken again, in seconds. `0` is immediately —
    /// unless [`done_once`](Self::done_once), which outranks it.
    pub restart_delay_secs: u32,
}

impl Default for QuestDef {
    fn default() -> Self {
        Self {
            key: QuestKey::default(),
            title: String::new(),
            description: String::new(),
            refuse: String::new(),
            uncomplete: String::new(),
            complete: String::new(),
            failed: String::new(),
            objectives: Vec::new(),
            rewards: Vec::new(),
            // ServUO's default: a quest asks for everything on its list.
            all_objectives: true,
            done_once: false,
            restart_delay_secs: 0,
        }
    }
}

/// Every quest this shard knows, by key.
///
/// Replaced wholesale by whoever registers — there is no "add one". The tree's
/// own [`shipped`] quests go in at boot, and a script pack that still registers
/// its own replaces them on the tick after; merging instead would leave a quest
/// the newer source has deleted still on offer.
#[derive(Clone, Default, Debug)]
pub struct QuestDefs {
    defs: Vec<QuestDef>,
    by_key: HashMap<QuestKey, usize>,
}

impl QuestDefs {
    /// Replace every definition with `defs`.
    ///
    /// A duplicate key keeps the *last* one, so a pack that redefines a quest
    /// later in its load order wins — the same rule a redefined function follows
    /// in the script itself.
    pub fn set(&mut self, defs: Vec<QuestDef>) {
        self.by_key = defs
            .iter()
            .enumerate()
            .map(|(index, def)| (def.key.clone(), index))
            .collect();
        self.defs = defs;
    }

    /// The definition for a key, if it is still defined.
    ///
    /// `None` is an ordinary answer, not a fault: a saved quest whose definition
    /// has since been removed reads as `None`, and every caller treats that as
    /// "this quest no longer exists" rather than failing.
    #[must_use]
    pub fn get(&self, key: &QuestKey) -> Option<&QuestDef> {
        self.by_key.get(key).and_then(|&index| self.defs.get(index))
    }

    /// Whether any quest is defined at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    /// How many are defined.
    #[must_use]
    pub fn len(&self) -> usize {
        self.defs.len()
    }
}

include!(concat!(env!("OUT_DIR"), "/quests.rs"));

#[cfg(test)]
mod tests {
    use super::*;

    fn quest(key: &str, title: &str) -> QuestDef {
        QuestDef {
            key: QuestKey::new(key),
            title: title.to_owned(),
            ..QuestDef::default()
        }
    }

    #[test]
    fn a_registration_replaces_everything_before_it() {
        let mut defs = QuestDefs::default();
        defs.set(vec![quest("rat_cull", "A Plague of Rats")]);
        defs.set(vec![quest("silk_gather", "Silk for the Spellwright")]);
        assert!(
            defs.get(&QuestKey::new("rat_cull")).is_none(),
            "a quest no longer defined must stop being offered"
        );
        assert_eq!(defs.len(), 1);
    }

    #[test]
    fn a_repeated_key_keeps_the_last_definition() {
        let mut defs = QuestDefs::default();
        defs.set(vec![quest("rat_cull", "Old"), quest("rat_cull", "New")]);
        assert_eq!(defs.get(&QuestKey::new("rat_cull")).unwrap().title, "New");
    }

    #[test]
    fn an_unknown_key_is_an_answer_not_a_fault() {
        let defs = QuestDefs::default();
        assert!(defs.get(&QuestKey::new("no_such_quest")).is_none());
    }

    #[test]
    fn every_shipped_quest_is_reachable_by_its_own_key() {
        let shipped = shipped();
        assert!(!shipped.is_empty(), "the shard ships no quests at all");

        let keys: Vec<QuestKey> = shipped.iter().map(|quest| quest.key.clone()).collect();
        let mut defs = QuestDefs::default();
        defs.set(shipped);

        // A key that does not come back is one `set` overwrote — two rows of
        // `data/quests.json` sharing a key, which `build.rs` is supposed to have
        // refused. The one a player could take would be whichever came last, and
        // nothing would say so.
        assert_eq!(defs.len(), keys.len(), "two shipped quests share a key");
        for key in keys {
            assert!(
                defs.get(&key).is_some(),
                "shipped quest {key:?} cannot be looked up"
            );
        }
    }

    #[test]
    fn the_shipped_escort_quest_names_no_region() {
        // Not an oversight to be tidied up: one definition covers every
        // escortable traveller precisely because it does *not* name a
        // destination — the engine picks one when the quest is accepted. Filling
        // this in would send all sixty-odd of them to the same town.
        let escort = shipped()
            .into_iter()
            .find(|quest| quest.key == QuestKey::new("escort"));
        let escort = escort.expect("the shard ships the escort quest");
        assert_eq!(
            escort.objectives[0].kind,
            ObjectiveKind::Escort {
                region: String::new()
            },
            "the escort objective must leave its destination to the engine"
        );
    }
}
