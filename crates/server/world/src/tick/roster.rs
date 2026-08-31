use openshard_protocol::login::CharacterEntry;

use super::*;

/// One character on an account: that it exists, and where it was when it was
/// last seen.
///
/// The two are deliberately not the same fact, and the `Option` is what says so.
/// A character *exists* from the moment it is created; it has a
/// [`CharacterRecord`] only once something has written one — a logout, or a boot
/// that read one out of the store. Between those two moments the character is on
/// the list, is offered by `0xA9`, can be picked and can be deleted, and there is
/// nothing anywhere describing it. That state used to be spelled "absent from the
/// roster", which is the same shape of lie `Option<PlayedCharacter>` told about
/// presence — see `docs/connection_state.md` — and it cost a brand-new character
/// being deleted out from under the connection playing it.
///
/// `None` is *absent*, not unknown: nothing has been recorded, and the character
/// enters fresh. See [`World::enter`](crate::World::enter), which reads exactly
/// that.
struct Enrolled {
    /// The name as it was created, case and all. What `0xA9` shows the player.
    name:  CharacterName,
    /// Where this character was when it was last seen, or `None` if nothing has
    /// ever recorded it.
    saved: Option<CharacterRecord>,
}

/// Which characters each account has, in slot order, and where each of them was.
///
/// # Why the world keeps this at all
///
/// The store has the same rows, but the store is written *later* — a snapshot is
/// handed to a task nobody waits for, so a player who logs out and straight back
/// in can beat their own save. This is the copy that closes that gap: seeded from
/// the store at boot by [`World::restore_characters`], and written by the logout
/// in [`World::despawn`] at the same instant the journal takes its copy.
///
/// # Why it is the world's and not the shard binary's
///
/// It was the binary's until S4 of `docs/connection_state.md`. The world was the
/// only thing that could *fill* it — a logout is a tick — so the world drained a
/// `departed` vector into a table it could not read, and the one caller that
/// needed to read it, entering a character, had to be handed the row on its
/// command. That put the same fact in two places with a channel between them:
/// the roster could be stale for exactly as long as the shard loop took to drain,
/// and nothing said so. Now the writer and the reader are the same value.
///
/// # It is the account's character list
///
/// Since S5 it is, and that is the change: this used to hold only the characters
/// something had *saved*, while which characters *existed* lived in the login
/// crate, outside the world. Two lists that had to agree, with the
/// world's half unable to see the other — the arrangement this whole plan exists
/// to delete. A character is on this list from the moment it is created, whether
/// or not anything has described it yet; see [`Enrolled`].
///
/// The order within an account is the slot order the client indexes: `0x83`
/// names a character by its position in the list `0xA9` last sent, so the two
/// have to be built from the same sequence. Deleting shifts the rest down, which
/// is what the `0x86` resend that follows expects.
pub(super) struct Roster(HashMap<String, Vec<Enrolled>>);

impl Roster {
    /// An empty roster — a shard whose store has not been read yet.
    pub(super) fn new() -> Self {
        Self(HashMap::new())
    }

    /// Put a character on an account's list, if it is not on it already.
    ///
    /// The idempotence is the point: this is called from three places that cannot
    /// see each other — the boot that reads the store, the config seeding beside
    /// it, and entering the world with a character created this run — and any two
    /// of them may name the same character. Adding it twice would make `0x5D`
    /// ambiguous, since it echoes the name and not the slot.
    ///
    /// Nothing is recorded about *where* the character is; that arrives with the
    /// first [`remember`](Self::remember).
    ///
    /// An entry that is already there is not touched at all — not its record, not
    /// its spelling. That, and `remember` adopting the record's spelling, is what
    /// holds the rule `boot::restore` used to hold by call order alone: a name in
    /// both the store and the config keeps the row that describes it *and* the
    /// name it was created under, whichever of the two ran first.
    pub(super) fn enrol(&mut self, account: &AccountName, name: &CharacterName) {
        let characters = self.0.entry(account.normalized()).or_default();
        if characters
            .iter()
            .any(|entry| entry.name.normalized() == name.normalized())
        {
            return;
        }
        characters.push(Enrolled {
            name:  name.clone(),
            saved: None,
        });
    }

    /// File a character's state, replacing whatever was known about it.
    ///
    /// Enrols the character if it is not on the list yet: a logout is a
    /// description of a character that certainly exists, so a record arriving for
    /// a name nothing enrolled is the enrolment being late, not the record being
    /// wrong. The account and name come off the record rather than from the
    /// caller, so a record cannot be filed under the wrong name.
    ///
    /// A record also carries the *spelling* — the case the player typed when the
    /// character was created — and that spelling wins over whatever the entry was
    /// enrolled under. Config's `[[accounts]] characters` names a character that
    /// exists; it does not get to rename it, and an operator writing
    /// `lord british` in a `.toml` must not change what `0xA9` shows. Adopting it
    /// here is what makes this order-independent: `enrol` before `remember` and
    /// `remember` before `enrol` leave the same list, so the boot order decides
    /// nothing about *which* character description survives. See
    /// [`enrol`](Self::enrol) for the half that says the record is never lost.
    pub(super) fn remember(&mut self, record: CharacterRecord) {
        self.enrol(&record.account, &record.name);
        let characters = self
            .0
            .get_mut(&record.account.normalized())
            .expect("enrol just created this account's list");
        let entry = characters
            .iter_mut()
            .find(|entry| entry.name.normalized() == record.name.normalized())
            .expect("enrol just put this character on the list");
        entry.name = record.name.clone();
        entry.saved = Some(record);
    }

    /// Where this character was last seen, or `None` if it does not exist or
    /// nothing has ever recorded it.
    ///
    /// The two are not told apart here because the one caller — entering the
    /// world — does the same thing either way: a character with nothing on file
    /// starts fresh. Whether it exists at all is [`characters`](Self::characters).
    pub(super) fn get(&self, account: &AccountName, character: &CharacterName) -> Option<&CharacterRecord> {
        self.0
            .get(&account.normalized())?
            .iter()
            .find(|entry| entry.name.normalized() == character.normalized())?
            .saved
            .as_ref()
    }

    /// Take a character off its account's list — it has been deleted.
    ///
    /// Hands back what was saved about it, because the caller needs the serial
    /// off that record to forget the store row and the inventory waiting under
    /// it. `None` covers both "no such character" and "nothing was ever recorded
    /// about it", and the caller has nothing to clean up in either case — but the
    /// entry is gone from the list either way, which is the half that used to be
    /// missed: a character created this run and never logged out had no record,
    /// so the early return took the *list* removal with it.
    pub(super) fn forget(
        &mut self,
        account: &AccountName,
        character: &CharacterName,
    ) -> Option<CharacterRecord> {
        let characters = self.0.get_mut(&account.normalized())?;
        let at = characters
            .iter()
            .position(|entry| entry.name.normalized() == character.normalized())?;
        characters.remove(at).saved
    }

    /// An account's characters in slot order, for the `0xA9` list.
    ///
    /// Empty for an account with none; the encoder pads the list out to the
    /// slots the client draws.
    pub(super) fn characters(&self, account: &AccountName) -> Vec<CharacterEntry> {
        self.0
            .get(&account.normalized())
            .map(|characters| {
                characters
                    .iter()
                    .map(|entry| {
                        CharacterEntry {
                            name: entry.name.clone(),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// How many characters have a saved record.
    ///
    /// Test-only, and it used to be the boot log's: the log now counts what
    /// `World::restore_characters` hands back, which is the same number said by
    /// the thing that did the restoring. What is left is this module's own tests
    /// asking whether a record exists at all — "not how many exist", because a
    /// character enrolled from config has never been written down.
    #[cfg(test)]
    fn saved(&self) -> usize {
        self.0
            .values()
            .flatten()
            .filter(|entry| entry.saved.is_some())
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A record with nothing on it but an identity. Everything the roster does
    /// keys on those three fields; the rest is what it carries, not what it is
    /// found by. Written out rather than `..Default::default()` because
    /// `CharacterRecord` deliberately has no `Default` — a zeroed character is
    /// not a character.
    fn record(account: &str, name: &str, serial: Serial) -> CharacterRecord {
        CharacterRecord {
            serial,
            account: AccountName::new(account),
            name: CharacterName::new(name),
            body: 400,
            hue: 0,
            facet: 0,
            x: 0,
            y: 0,
            z: 0,
            facing: 0,
            strength: 10,
            dexterity: 10,
            intelligence: 10,
            skills: Vec::new(),
            stat_locks: openshard_persistence::StatLockRecord::default(),
            effects: Vec::new(),
            dead: false,
            fame: 0,
            karma: 0,
            murders: 0,
            quests: Vec::new(),
            done_quests: Vec::new(),
            guild: None,
            guild_title: String::new(),
            guild_rank: 0,
            guild_candidate: None,
        }
    }

    #[test]
    fn a_character_is_found_however_the_client_spells_it() {
        // The client sends the name back as the player typed it, and the account
        // name came off a `0x91` field. Both halves are folded, which is what
        // makes a lookup with either spelling the same lookup.
        let mut roster = Roster::new();
        roster.remember(record("Admin", "Lord British", Serial::new(7).unwrap()));

        assert_eq!(
            roster
                .get(&AccountName::new("admin"), &CharacterName::new("lord british"))
                .map(|record| record.serial),
            Some(Serial::new(7).unwrap())
        );
        assert!(
            roster
                .get(&AccountName::new("admin"), &CharacterName::new("Dupre"))
                .is_none(),
            "and a character nobody saved is absent, not a default"
        );
    }

    #[test]
    fn two_accounts_may_hold_the_same_character_name() {
        // The account is half the key. Folding only the character name would
        // have one player's logout position overwrite another's.
        let mut roster = Roster::new();
        roster.remember(record("alice", "Dupre", Serial::new(1).unwrap()));
        roster.remember(record("bob", "Dupre", Serial::new(2).unwrap()));

        assert_eq!(roster.saved(), 2);
        assert_eq!(
            roster
                .get(&AccountName::new("bob"), &CharacterName::new("Dupre"))
                .map(|record| record.serial),
            Some(Serial::new(2).unwrap())
        );
    }

    #[test]
    fn forgetting_hands_back_the_serial_the_world_must_drop() {
        let mut roster = Roster::new();
        roster.remember(record("admin", "Lord British", Serial::new(7).unwrap()));

        let dropped = roster.forget(&AccountName::new("ADMIN"), &CharacterName::new("LORD BRITISH"));
        assert_eq!(dropped.map(|record| record.serial), Some(Serial::new(7).unwrap()));
        assert_eq!(roster.saved(), 0);
        assert!(
            roster.characters(&AccountName::new("admin")).is_empty(),
            "and it is off the list, not merely undescribed"
        );
    }

    #[test]
    fn a_character_exists_before_anything_has_described_it() {
        // The distinction this type was rebuilt for. A character created this run
        // has never logged out, so nothing has written a record — and it is still
        // on the list, still offered by 0xA9, still deletable. Treating "no
        // record" as "no character" is how one got deleted out from under the
        // connection playing it.
        let mut roster = Roster::new();
        roster.enrol(&AccountName::new("admin"), &CharacterName::new("Dupre"));

        assert_eq!(
            roster.characters(&AccountName::new("admin")),
            vec![CharacterEntry {
                name: CharacterName::new("Dupre"),
            }],
            "it is on the list"
        );
        assert!(
            roster
                .get(&AccountName::new("admin"), &CharacterName::new("Dupre"))
                .is_none(),
            "with nothing recorded about it"
        );
        assert_eq!(roster.saved(), 0, "and the boot log counts none");
    }

    #[test]
    fn a_character_with_nothing_recorded_is_still_deleted() {
        // The half the old early return dropped: `forget` found no record, so it
        // returned before taking the character off the list at all.
        let mut roster = Roster::new();
        roster.enrol(&AccountName::new("admin"), &CharacterName::new("Dupre"));

        assert!(
            roster
                .forget(&AccountName::new("admin"), &CharacterName::new("Dupre"))
                .is_none(),
            "there is nothing saved to hand back"
        );
        assert!(
            roster.characters(&AccountName::new("admin")).is_empty(),
            "but the character is gone"
        );
    }

    #[test]
    fn enrolling_twice_leaves_one_character() {
        // Boot does it twice on purpose: the store's rows and the config's names
        // are read by two functions that cannot see each other. A duplicate would
        // make `0x5D` ambiguous, because it echoes the name and not the slot.
        let mut roster = Roster::new();
        roster.enrol(&AccountName::new("admin"), &CharacterName::new("Lord British"));
        roster.remember(record("admin", "Lord British", Serial::new(7).unwrap()));
        roster.enrol(&AccountName::new("admin"), &CharacterName::new("lord british"));

        assert_eq!(roster.characters(&AccountName::new("admin")).len(), 1);
        assert_eq!(
            roster
                .get(&AccountName::new("admin"), &CharacterName::new("Lord British"))
                .map(|record| record.serial),
            Some(Serial::new(7).unwrap()),
            "and the late enrolment did not wipe what was known"
        );
    }

    #[test]
    fn the_boot_order_decides_nothing_about_a_character_in_both_halves() {
        // The rule `boot::restore` states in prose: a name in both the store and
        // the config keeps the row that describes it. It is asserted here from
        // both sides, because the prose only ever held one of them.
        //
        // The record survives either way — `enrol` never touches an entry that is
        // there. The *spelling* did not: an operator who wrote `lord british` in
        // the config renamed a character the player created as `Lord British`,
        // for as long as the config seeding happened to run first. `remember`
        // adopting the record's name is what makes both directions equal.
        let played = |roster: &Roster| roster.characters(&AccountName::new("admin"))[0].name.clone();
        let described = |roster: &Roster| {
            roster
                .get(&AccountName::new("admin"), &CharacterName::new("Lord British"))
                .map(|record| record.serial)
        };

        let mut store_first = Roster::new();
        store_first.remember(record("admin", "Lord British", Serial::new(7).unwrap()));
        store_first.enrol(&AccountName::new("admin"), &CharacterName::new("lord british"));

        let mut config_first = Roster::new();
        config_first.enrol(&AccountName::new("admin"), &CharacterName::new("lord british"));
        config_first.remember(record("admin", "Lord British", Serial::new(7).unwrap()));

        assert_eq!(played(&store_first), CharacterName::new("Lord British"));
        assert_eq!(
            played(&config_first),
            CharacterName::new("Lord British"),
            "the config names a character that exists; it does not rename it"
        );
        assert_eq!(described(&store_first), Some(Serial::new(7).unwrap()));
        assert_eq!(
            described(&config_first),
            Some(Serial::new(7).unwrap()),
            "and the late enrolment did not wipe what was known"
        );
    }

    #[test]
    fn the_list_keeps_the_order_the_client_indexes() {
        // `0x83` names a character by its position in the list `0xA9` last sent,
        // and deleting shifts the rest down — which is what the `0x86` resend
        // that follows draws.
        let mut roster = Roster::new();
        for name in ["Alpha", "Beta", "Gamma"] {
            roster.enrol(&AccountName::new("admin"), &CharacterName::new(name));
        }
        let names = |roster: &Roster| {
            roster
                .characters(&AccountName::new("admin"))
                .into_iter()
                .map(|entry| entry.name.0)
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&roster), ["Alpha", "Beta", "Gamma"]);

        roster.forget(&AccountName::new("admin"), &CharacterName::new("Beta"));
        assert_eq!(names(&roster), ["Alpha", "Gamma"], "the rest shift down");
    }
}
