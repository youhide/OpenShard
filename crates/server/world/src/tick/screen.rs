//! The character screen, answered out of a tick.
//!
//! Everything between the game login and the world: the `0xA9` list, creating a
//! character, deleting one, and picking one to play. It was the shard binary's
//! until S5 of `docs/connection_state.md`, for one structural reason — the world
//! could not address a connection with no character, and could not say which
//! characters existed. Both are fixed (S1's connection row, S5a's roster), and
//! this is what they were fixed for.
//!
//! # Why it belongs here rather than in the login crate
//!
//! Which characters an account has is world state: it is what the store's
//! character rows say, it changes when a character is deleted mid-run, and a
//! character being *played* is a fact only the world holds. Answering it from
//! outside meant a second list that had to agree with the world's and could not
//! see it — see the roster's own doc for what that cost.
//!
//! Login keeps what is genuinely not simulation: credentials, argon2, the auth
//! key and the relay. It ends at `Command::Authenticated`.

use openshard_protocol::identity::RawCharacterName;
use openshard_protocol::login::{
    CHARACTER_NAME_LENGTH,
    CharacterList,
    CharacterListFlags,
    CharacterListUpdate,
    DeleteReject,
    DeleteResult,
    DenyReason,
    LoginDenied,
    MIN_CHARACTER_SLOTS,
    StartLocation,
};
use openshard_protocol::skill::SkillLock;
use openshard_protocol::wire::RawCharacterSlot;
use openshard_protocol::world::CreateCharacter;

use super::*;

/// What the character screen shows, beside the characters themselves.
///
/// Configuration rather than simulation — which starting cities are offered and
/// which client capabilities are advertised are the operator's choices — so it is
/// handed to the world at boot the way [`Gameplay`] is, and never read from a
/// config file in here.
///
/// [`Gameplay`]: openshard_state::Gameplay
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct CharacterScreen {
    /// The starting cities offered at creation, in the order the client indexes:
    /// `start_location` on the create packet is a raw index into this list.
    ///
    /// Never empty on a configured shard — ClassicUO refuses to open the creation
    /// screen without one and says so ("No city found. Something wrong with the
    /// received cities.") — but empty is representable, because a `World::new`
    /// with no boot behind it has no cities and no client either.
    pub starts:   Vec<StartLocation>,
    /// The `0xA9` client-capability mask. This is the one ClassicUO reads to turn
    /// on AoS tooltips and context menus, not the `0xB9` below.
    pub flags:    CharacterListFlags,
    /// The `0xB9` SupportedFeatures mask sent just ahead of the list.
    /// [`SupportedFeatures::NONE`] means "do not advertise": no `0xB9` goes out
    /// and a modern client stays on the classic single-click name path.
    pub features: SupportedFeatures,
}

impl World {
    /// Offer this connection its account's characters — the `0xA9` list, with the
    /// `0xB9` feature mask ahead of it when the shard advertises one.
    ///
    /// One buffer would do, and the login crate used to build one; two sends are
    /// the same thing on the wire, since both packets are self-framing and the
    /// outbox preserves order.
    fn send_character_list(&mut self, connection: ConnectionId) {
        let Some(row) = self.state.connections.get(&connection) else {
            // Nothing to answer: the connection went away between the packet and
            // this tick, which is ordinary and not worth a warning.
            return;
        };
        let (version, account) = (row.version, row.account.clone());
        let characters = self.roster.characters(&account);
        debug!(%connection, %account, count = characters.len(), "sending the character list");
        if self.screen.features != SupportedFeatures::NONE {
            let extended = version.supports(Feature::ExtraFeatureMask);
            self.state.send(
                connection,
                encode_supported_features(self.screen.features, extended),
            );
        }
        self.state.send_packet(
            connection,
            &ServerPacket::CharacterList(CharacterList {
                characters,
                starts: self.screen.starts.clone(),
                flags: self.screen.flags,
            }),
        );
    }

    /// Take a connection over from the login conversation and show it its
    /// characters.
    pub(super) fn authenticated(
        &mut self,
        connection: ConnectionId,
        version: ClientVersion,
        account: AccountName,
        access: AccessLevel,
    ) {
        self.attach(connection, version, account, access);
        self.send_character_list(connection);
    }

    /// Whether some connection is playing this character right now.
    ///
    /// Asked of the entities, which is the only place that can answer it: a
    /// character is being played exactly when there is a player entity carrying
    /// that account and that name. This used to be asked of the shard's session
    /// table — the world could not be asked synchronously, and the question
    /// arrived on a packet the binary answered — and before that of a serial the
    /// caller had to look up, which a character created this run does not have.
    /// Neither hole exists here: no serial is needed, and the entity is the fact
    /// itself rather than a projection of it.
    fn is_playing(&self, account: &AccountName, name: &CharacterName) -> bool {
        self.state.players.values().any(|&entity| {
            let played_by = self.state.registry.get::<Account>(entity);
            let called = self.state.registry.get::<Name>(entity);
            match (played_by, called) {
                (Some(Account(on)), Some(Name(called))) => {
                    on.normalized() == account.normalized() && called.to_lowercase() == name.normalized()
                }
                _ => false,
            }
        })
    }

    /// Refuse something on the character screen with the `0x82` a client renders
    /// as a login error, and leave the connection where it is.
    ///
    /// The client stays on the screen it was on and the player can try again,
    /// which is what Sphere does with the same packet. Nothing here closes the
    /// socket: a refused *creation* is not a refused entry.
    fn refuse_screen(&mut self, connection: ConnectionId, reason: DenyReason) {
        self.state
            .send_packet(connection, &ServerPacket::LoginDenied(LoginDenied { reason }));
    }

    /// Create a character on this connection's account and enter the world with
    /// it — the two halves of what a `0x00`/`0xF8` asks for.
    ///
    /// The name is validated here, and this is the only place a
    /// [`RawCharacterName`] from the creation screen becomes a [`CharacterName`]:
    /// trimmed (the client pads its 30-byte field), non-empty, inside the wire
    /// width, not already on the account, and not one character past what the
    /// list can show. A sixth character would be created and then be invisible,
    /// which is worse than being refused where the client can say why.
    pub(super) fn create_character(&mut self, connection: ConnectionId, create: CreateCharacter) {
        let Some(row) = self.state.connections.get(&connection) else {
            warn!(%connection, "create-character from a connection the world does not know");
            return;
        };
        let (account, access, version) = (row.account.clone(), row.access, row.version);

        let name = match self.validate_new_name(&account, &create.name) {
            Ok(name) => name,
            Err(reason) => {
                warn!(%connection, %account, %create.name, ?reason, "character creation refused");
                self.refuse_screen(connection, reason);
                return;
            }
        };
        info!(%connection, %account, %name, "character created");

        // Place it in the city the player picked. `start_location` indexes the
        // very list this shard offered, so a valid pick names a real city; only a
        // client sending an out-of-range index falls back to the default facet and
        // the world's own start.
        let (facet, start) = match self.screen.starts.get(create.start_location.0 as usize) {
            Some(city) => (Facet(city.map.0), Some(city.position)),
            None => (self.state.default_facet, None),
        };

        self.roster.enrol(&account, &name);
        self.enter(Entering {
            connection,
            version,
            account,
            name,
            access,
            character: Character::Fresh(FreshCharacter {
                facet,
                start,
                appearance: Some(Appearance {
                    // No promotion exists yet for either raw value — see
                    // `docs/protocol_newtypes.md` — so this is still an unchecked
                    // pass-through, visible at the call site as `.0`.
                    body: {
                        let (sex, race) = create.sex_race.interpret();
                        Graphic(CreateCharacter::body(sex, race))
                    },
                    hue:  Hue(create.skin_hue.0),
                }),
                sheet: Some(Box::new(Self::chosen_sheet(&create))),
            }),
        });
    }

    /// What the player chose on the creation screen, as a sheet `enter` can
    /// apply.
    ///
    /// The client sends whole points; skills are stored in tenths, so a chosen 50
    /// becomes 500. New skills start unlocked, which is what "training up" is.
    ///
    /// None of the stat or skill values is validated: no promotion exists yet for
    /// `RawStatValue`/`RawSkillValue`, so the `.0`s below are an unchecked
    /// pass-through of client input. See `docs/protocol_newtypes.md`'s pilot
    /// notes.
    fn chosen_sheet(create: &CreateCharacter) -> CharacterSheet {
        CharacterSheet {
            strength:        u16::from(create.strength.0),
            dexterity:       u16::from(create.dexterity.0),
            intelligence:    u16::from(create.intelligence.0),
            skills:          create
                .skills
                .iter()
                .filter(|choice| choice.value.0 > 0)
                // A cap of zero means "whatever this shard caps a skill at" —
                // `enter` fills it in from `[gameplay] skill_cap`, so the knob is
                // read in one place.
                .map(|choice| (choice.skill.0, u16::from(choice.value.0) * 10, SkillLock::Up, 0))
                .collect(),
            // A new character's arrows all point up, and no stat has ever risen.
            stat_locks:      openshard_persistence::StatLockRecord::default(),
            // A new character is clean, and unknown.
            effects:         Vec::new(),
            dead:            false,
            fame:            0,
            karma:           0,
            murders:         0,
            quests:          Vec::new(),
            done_quests:     Vec::new(),
            guild:           None,
            guild_candidate: None,
        }
    }

    /// Turn a name off the creation screen into one this account may have, or say
    /// why not.
    ///
    /// The failure modes are the codes the client can render: a full account is
    /// [`DenyReason::TooManyCharacters`], and an empty, overlong or duplicate name
    /// is [`DenyReason::BadCharacter`].
    fn validate_new_name(
        &self,
        account: &AccountName,
        raw: &RawCharacterName,
    ) -> Result<CharacterName, DenyReason> {
        let trimmed = raw.0.trim();
        if trimmed.is_empty() || raw.0.len() > CHARACTER_NAME_LENGTH {
            return Err(DenyReason::BadCharacter);
        }
        let characters = self.roster.characters(account);
        if characters.len() >= MIN_CHARACTER_SLOTS {
            return Err(DenyReason::TooManyCharacters);
        }
        // Two characters with one name make `0x5D` ambiguous — it echoes the
        // name, not the slot — so a duplicate is refused rather than shadowed.
        if characters
            .iter()
            .any(|entry| entry.name.0.eq_ignore_ascii_case(trimmed))
        {
            return Err(DenyReason::BadCharacter);
        }
        Ok(CharacterName(trimmed.to_owned()))
    }

    /// Enter the world with a character the account already has (`0x5D`).
    ///
    /// The name is the raw one the client echoed off the list it was sent, and it
    /// is looked up on that list rather than trusted: a `0x5D` naming a character
    /// this account does not have is a refusal, not an entry. Where that character
    /// was — serial, spot, look and sheet — is [`enter`](Self::enter)'s to read
    /// out of the roster.
    pub(super) fn play_character(&mut self, connection: ConnectionId, raw: RawCharacterName) {
        let Some(row) = self.state.connections.get(&connection) else {
            warn!(%connection, "character-play from a connection the world does not know");
            return;
        };
        let (account, access, version) = (row.account.clone(), row.access, row.version);

        let Some(name) = self.named_character(&account, &raw) else {
            warn!(%connection, %account, name = %raw, "picked a character this account does not have");
            self.refuse_entry(connection, RefusedEntry::NoSuchCharacter);
            return;
        };
        self.enter(Entering {
            connection,
            version,
            account,
            name,
            access,
            character: Character::Saved,
        });
    }

    /// The character on this account that the client's echoed name means, folded
    /// the way every other name in the shard is.
    fn named_character(&self, account: &AccountName, raw: &RawCharacterName) -> Option<CharacterName> {
        let wanted = raw.0.trim().to_lowercase();
        self.roster
            .characters(account)
            .into_iter()
            .find(|entry| entry.name.normalized() == wanted)
            .map(|entry| entry.name)
    }

    /// Delete the character in a slot of the list this connection was last sent
    /// (`0x83`).
    ///
    /// Refuses — with the `0x85` the client renders — a slot that names no
    /// character, and one somebody is playing. The second is the interesting one:
    /// the connection asking is never the one playing, because it is sitting on
    /// the character screen. It is a *second* connection on the same account,
    /// which is the only way the situation arises at all.
    pub(super) fn delete_character_at(&mut self, connection: ConnectionId, slot: RawCharacterSlot) {
        let Some(row) = self.state.connections.get(&connection) else {
            warn!(%connection, "delete-character from a connection the world does not know");
            return;
        };
        let account = row.account.clone();

        // The slot indexes this account's list, so that list's length is the whole
        // domain and a slot outside it names no character to look up.
        let characters = self.roster.characters(&account);
        let Ok(slot) = slot.validate(characters.len()) else {
            warn!(%connection, %account, slot = slot.0, "delete refused: no such slot");
            self.reject_delete(connection, DeleteResult::CharNotExist);
            return;
        };
        let name = characters[slot.0 as usize].name.clone();

        if self.is_playing(&account, &name) {
            warn!(%connection, %account, %name, "delete refused: it is being played");
            self.reject_delete(connection, DeleteResult::CharBeingPlayed);
            return;
        }

        self.delete_character(&account, &name);
        info!(%connection, %account, %name, "character deleted");
        // And redraw the screen from the list that is left. The client expects the
        // whole list back, not a removal.
        let characters = self.roster.characters(&account);
        self.state.send_packet(
            connection,
            &ServerPacket::CharacterListUpdate(CharacterListUpdate { characters }),
        );
    }

    /// Tell a client its `0x83` did not happen, and why.
    fn reject_delete(&mut self, connection: ConnectionId, result: DeleteResult) {
        self.state
            .send_packet(connection, &ServerPacket::DeleteReject(DeleteReject { result }));
    }
}

#[cfg(test)]
mod tests {
    use openshard_protocol::wire::RawGraphic;
    use openshard_protocol::world::{
        ClientFlags,
        RawProfession,
        RawSexRace,
        RawStartLocationIndex,
        RawStatValue,
    };

    use super::super::tests::{
        authenticate,
        connection,
        delete_slot,
        enter_as,
        packets_for,
        world,
    };
    use super::*;

    fn admin() -> AccountName {
        AccountName("admin".to_owned())
    }

    /// What a client sends when the player finishes the creation screen. Only the
    /// name and the stats matter to anything asserted here; the rest is what the
    /// packet carries and is filled in so the fixture is a whole one.
    fn creating(name: &str) -> CreateCharacter {
        CreateCharacter {
            name:           RawCharacterName(name.to_owned()),
            flags:          ClientFlags(0),
            profession:     RawProfession(0),
            sex_race:       RawSexRace(0),
            strength:       RawStatValue(45),
            dexterity:      RawStatValue(35),
            intelligence:   RawStatValue(10),
            skills:         Vec::new(),
            skin_hue:       RawHue(0x83EA),
            hair:           RawGraphic(0),
            hair_hue:       RawHue(0),
            beard:          RawGraphic(0),
            beard_hue:      RawHue(0),
            start_location: RawStartLocationIndex(0),
            slot:           RawCharacterSlot(0),
            shirt_hue:      RawHue(0),
            pants_hue:      RawHue(0),
        }
    }

    /// The ids of the packets one connection was sent this tick. What most of
    /// these tests assert on: which packet came back is the whole answer, and its
    /// body is the encoder's business and pinned in the encoder's own tests.
    fn replies(world: &mut World, connection: ConnectionId) -> Vec<u8> {
        packets_for(world, connection)
            .into_iter()
            .map(|packet| packet[0])
            .collect()
    }

    #[test]
    fn the_character_list_comes_out_of_the_tick_that_takes_the_connection_over() {
        // The move S5 is: `0xA9` used to be built by the login crate off its own
        // copy of the account's characters. It is built here now, from the roster,
        // which is the same value `0x83` indexes and `0x5D` is checked against —
        // so there is no longer a second list that has to agree with this one.
        let mut world = world();
        let connection = connection();
        world.enrol_character(&admin(), &CharacterName("Lord British".to_owned()));

        authenticate(&mut world, connection, Instant::now());

        let packets = packets_for(&mut world, connection);
        let list = packets.iter().find(|packet| packet[0] == 0xA9);
        let list = list.expect("the character list goes back");
        assert!(
            list.windows(12).any(|window| window == b"Lord British"),
            "with the character the roster has on this account"
        );
    }

    #[test]
    fn a_connection_the_world_never_took_over_is_answered_with_nothing() {
        // Every arm here opens by looking the row up, and this is what the lookup
        // is for: a `0x00` or a `0x83` from a connection that never authenticated
        // has no account to act on. It is dropped rather than defaulted — a
        // default account is somebody's.
        let mut world = world();
        let connection = connection();
        world.queue(Command::CreateCharacter {
            connection,
            create: creating("Nobody"),
        });
        world.tick(Instant::now());

        assert!(replies(&mut world, connection).is_empty());
        assert!(world.characters(&admin()).is_empty(), "and nothing was created");
    }

    #[test]
    fn a_created_character_joins_the_list_and_the_world() {
        let now = Instant::now();
        let mut world = world();
        let connection = connection();
        authenticate(&mut world, connection, now);
        let _ = packets_for(&mut world, connection);

        world.queue(Command::CreateCharacter {
            connection,
            create: creating("Dupre"),
        });
        world.tick(now);

        assert_eq!(
            world
                .characters(&admin())
                .into_iter()
                .map(|entry| entry.name.0)
                .collect::<Vec<_>>(),
            ["Dupre"],
            "it is on the account's list"
        );
        assert!(
            world.state.players.contains_key(&connection),
            "and it is being played, without a second packet from the client"
        );
    }

    #[test]
    fn a_duplicate_name_is_refused_and_the_connection_stays() {
        // The refusal a player can act on: the client shows the error and leaves
        // them on the creation screen to pick another name. Nothing closes here —
        // which is why a refused creation is deliberately not a `PlayerRefused`.
        let now = Instant::now();
        let mut world = world();
        let connection = connection();
        world.enrol_character(&admin(), &CharacterName("Dupre".to_owned()));
        authenticate(&mut world, connection, now);
        let _ = packets_for(&mut world, connection);

        world.queue(Command::CreateCharacter {
            connection,
            // Spelled differently on purpose: names are one name however the
            // player types them, and `0x5D` echoes the name rather than the slot,
            // so two that differ only in case would be ambiguous.
            create: creating("dupre"),
        });
        world.tick(now);

        assert_eq!(replies(&mut world, connection), [0x82], "refused, and told why");
        assert_eq!(world.characters(&admin()).len(), 1, "and nothing was added");
        assert!(
            !world.state.players.contains_key(&connection),
            "and nobody entered the world"
        );
    }

    #[test]
    fn an_empty_name_is_refused() {
        // The client pads its 30-byte name field, so a name of spaces arrives as
        // spaces and is not a name.
        let now = Instant::now();
        let mut world = world();
        let connection = connection();
        authenticate(&mut world, connection, now);
        let _ = packets_for(&mut world, connection);

        world.queue(Command::CreateCharacter {
            connection,
            create: creating("   "),
        });
        world.tick(now);

        assert_eq!(replies(&mut world, connection), [0x82]);
        assert!(world.characters(&admin()).is_empty());
    }

    #[test]
    fn a_sixth_character_is_refused_where_the_client_can_hear_it() {
        // The list shows five slots, so a sixth would be created and then be
        // invisible — worse than a refusal, because nothing anywhere says why.
        let now = Instant::now();
        let mut world = world();
        let connection = connection();
        for name in ["One", "Two", "Three", "Four", "Five"] {
            world.enrol_character(&admin(), &CharacterName(name.to_owned()));
        }
        authenticate(&mut world, connection, now);
        let _ = packets_for(&mut world, connection);

        world.queue(Command::CreateCharacter {
            connection,
            create: creating("Six"),
        });
        world.tick(now);

        assert_eq!(replies(&mut world, connection), [0x82]);
        assert_eq!(world.characters(&admin()).len(), 5);
    }

    #[test]
    fn a_character_being_played_cannot_be_deleted_from_another_connection() {
        // The hole this closes, twice over. The check was once `is_online(serial)`
        // — and a character created this run has no serial recorded anywhere, so
        // for a fresh character it did not run at all. It then became a scan of
        // the shard's session table, which was right but was a projection. Here it
        // is the entity itself: a character is being played exactly when one is
        // standing in the world under that account and name.
        let now = Instant::now();
        let mut world = world();
        // One connection playing "Lord British" — entering enrols it, so nothing
        // else has to put it on the list.
        enter_as(&mut world, connection(), now);

        // And a second on the same account, sitting on the character screen. This
        // is the only way the situation arises at all.
        let screen = delete_slot(&mut world, 0, now);

        assert_eq!(
            replies(&mut world, screen),
            [0xA9, 0x85],
            "the list it was sent at login, then the refusal"
        );
        assert_eq!(
            world.characters(&admin()).len(),
            1,
            "and the character somebody is playing is still there"
        );
    }

    #[test]
    fn a_character_nobody_is_playing_is_deleted_and_the_screen_redrawn() {
        // The other direction, so the test above cannot pass by refusing
        // everything.
        let now = Instant::now();
        let mut world = world();
        world.enrol_character(&admin(), &CharacterName("Dupre".to_owned()));

        let screen = delete_slot(&mut world, 0, now);

        assert_eq!(
            replies(&mut world, screen),
            [0xA9, 0x86],
            "the list at login, then the list again without it"
        );
        assert!(world.characters(&admin()).is_empty());
    }

    #[test]
    fn a_slot_naming_no_character_is_refused() {
        let now = Instant::now();
        let mut world = world();
        let screen = delete_slot(&mut world, 3, now);

        assert_eq!(replies(&mut world, screen), [0xA9, 0x85]);
    }

    #[test]
    fn picking_a_character_the_account_does_not_have_is_refused() {
        // `0x5D` echoes a name, and a name off the wire is an input rather than an
        // invariant. Trusting it would enter a character nobody created — on a
        // fresh serial, at the start city, under whatever name the client sent.
        let now = Instant::now();
        let mut world = world();
        let connection = connection();
        authenticate(&mut world, connection, now);
        let _ = packets_for(&mut world, connection);

        world.queue(Command::PlayCharacter {
            connection,
            name: RawCharacterName("Somebody Else".to_owned()),
        });
        world.tick(now);

        assert!(
            !world.state.players.contains_key(&connection),
            "nobody entered the world"
        );
        assert!(
            world.characters(&admin()).is_empty(),
            "and no character was invented to enter as"
        );
    }

    #[test]
    fn picking_a_character_off_the_list_enters_it_however_it_is_spelled() {
        // The other direction, and the case a real client produces: it echoes the
        // name as the player typed it, and the lookup folds case like every other
        // name in the shard.
        let now = Instant::now();
        let mut world = world();
        let connection = connection();
        world.enrol_character(&admin(), &CharacterName("Lord British".to_owned()));
        authenticate(&mut world, connection, now);

        world.queue(Command::PlayCharacter {
            connection,
            name: RawCharacterName("lord british".to_owned()),
        });
        world.tick(now);

        assert!(world.state.players.contains_key(&connection), "in the world");
    }
}
