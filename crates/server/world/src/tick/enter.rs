use super::*;
use openshard_protocol::wire::Hue;

impl World {
    /// Take a connection over from the login conversation.
    ///
    /// The world's whole knowledge of a client that is playing nothing yet. It is
    /// what makes the connection addressable — [`WorldState::send_packet`] frames
    /// a packet for the version recorded here — so everything the character screen
    /// will need to answer starts with this row existing.
    ///
    /// Called twice on a real shard, and that is deliberate rather than tolerated:
    /// once for `Command::Authenticated`, and again from [`enter`](Self::enter),
    /// which carries the same version on its own command and would otherwise have
    /// to trust that the hand-off happened. Every test that queues an `Enter`
    /// without one does exactly that. One writer, two callers; the value written
    /// is the same one both times, because both read it off the auth key the login
    /// socket issued.
    ///
    /// Which is why this writes the identity *into* the row rather than replacing
    /// it. Since S7 the row also carries what the client is in the middle of — the
    /// item on its cursor above all — and that is not something either caller
    /// knows or is entitled to reset.
    ///
    /// [`WorldState::send_packet`]: openshard_state::WorldState::send_packet
    pub(super) fn attach(
        &mut self,
        connection: ConnectionId,
        version: ClientVersion,
        account: AccountName,
        access: AccessLevel,
    ) {
        // Written into the existing row when there is one, rather than over it:
        // both callers pass the same identity, but the row now also carries what
        // the client is in the middle of, and a second hand-off must not put a
        // dragged item back on the floor of a world it never left.
        match self.state.connection_mut(connection) {
            Some(row) => row.identify(version, account, access),
            None => {
                self.state.connections.insert(
                    connection,
                    openshard_state::connection::Connection::new(version, account, access),
                );
            }
        }
    }

    /// Put a character into the world for a connection that asked to play it.
    ///
    /// Either the connection ends the call in the world, or it is told it did
    /// not: an entry that fails without a [`PlayerRefused`] would leave the
    /// binary's phase on `Entering` — the name of the gap between the queued
    /// command and the tick that applies it — with nothing that ever moves it on,
    /// and the client waiting on "logging into shard" forever.
    ///
    /// That obligation is why the work is in [`try_enter`](Self::try_enter) and
    /// this is a wrapper. A failure path there is a `return Err(reason)`, and the
    /// refusal is emitted in one place for all of them, so a fourth one added
    /// later cannot forget to say so. It used to be checkable only by reading
    /// every early return.
    pub(super) fn enter(&mut self, entering: Entering) {
        let connection = entering.connection;
        if let Err(reason) = self.try_enter(entering) {
            self.refuse_entry(connection, reason);
        }
    }

    /// The entry itself. See [`enter`](Self::enter) for why it returns a
    /// [`RefusedEntry`] rather than emitting one.
    fn try_enter(&mut self, entering: Entering) -> Result<(), RefusedEntry> {
        let Entering {
            connection,
            version,
            account,
            name,
            access,
            character,
        } = entering;
        if self.state.players.contains_key(&connection) {
            warn!(%connection, "already in the world");
            return Err(RefusedEntry::AlreadyInWorld);
        }

        // Split the two arrivals into what the rest of this function reads. The
        // `Option`s below are the honest ones — a character nobody ever described
        // has no look and no sheet — and they are local: a *stored* character
        // cannot reach the arms below half-unpacked, because `StoredCharacter`
        // has no way to say it.
        //
        // Resolving `Saved` is this function's job rather than the caller's,
        // because the roster is the world's since S4 of
        // `docs/connection_state.md`. This is the one place a name becomes a row.
        // A name nothing has recorded — config-seeded, or created this run and
        // never logged out — is neither an error nor a refusal: it enters fresh
        // on the default facet, which is what it did when the shard resolved this
        // and passed a bare `Character::fresh`.
        let (requested_facet, saved_serial, saved_position, saved_facing, appearance, sheet) = match character
        {
            Character::Saved => match self
                .roster
                .get(&account, &name)
                .and_then(StoredCharacter::from_record)
            {
                Some(stored) => (
                    stored.facet,
                    Some(stored.serial),
                    Some(stored.position),
                    Some(stored.facing),
                    Some(stored.appearance),
                    Some(Box::new(stored.sheet)),
                ),
                None => (self.state.default_facet, None, None, None, None, None),
            },
            Character::Fresh(fresh) => (
                fresh.facet,
                None,
                fresh.start,
                None,
                fresh.appearance,
                fresh.sheet,
            ),
        };

        // A character can only stand on a facet the shard loaded. An unloaded one
        // — a save from a shard that had more facets, say — falls back to the
        // default rather than leaving the character nowhere.
        //
        // The `Facet` is carried the whole way now: `WorldState::facets` is keyed
        // by it, so nothing between here and the component is a bare byte. Only the
        // `warn!` opens it, because a tracing field wants something printable.
        let facet = if self.state.facets.contains_key(&requested_facet) {
            requested_facet
        } else {
            warn!(%connection, facet = requested_facet.0, "unloaded facet; falling back to the default");
            self.state.default_facet
        };

        // A stored character comes back on the serial it was saved under; a new
        // one takes a fresh serial from the pool. The saved serial was reserved
        // at boot (see `World::reserve_serial`), so binding it here cannot collide.
        let (entity, serial) = match saved_serial {
            Some(saved) => {
                let entity = self.state.registry.spawn();
                if let Err(error) = self.state.registry.bind_serial(entity, saved) {
                    warn!(%connection, ?error, "could not restore the saved serial");
                    self.state.registry.despawn(entity);
                    return Err(RefusedEntry::SerialInUse);
                }
                (entity, saved)
            }
            None => match self.state.registry.spawn_with_serial(SerialKind::Mobile) {
                Ok(pair) => pair,
                Err(_) => {
                    warn!(%connection, "the mobile serial pool is exhausted");
                    return Err(RefusedEntry::NoSerialsLeft);
                }
            },
        };

        // A loaded character spawns exactly where it was saved, its own z
        // included; a fresh one takes the world's configured start on its facet.
        let position = saved_position.unwrap_or_else(|| self.state.start_position(facet));
        // A stored character faces the way it logged out; a fresh one has never
        // faced anywhere, so it takes the same south the client's own creation
        // screen shows it standing.
        let facing = saved_facing.unwrap_or_else(|| Facing::walking(Direction::South));
        // A created or loaded character brings its body and hue; without one it
        // falls back to the default. `Body` and `Appearance` now hold the same wire
        // `Drawn`/`Hue`, so this is a move rather than an unwrap-and-rewrap: the
        // only place the numbers are opened is the encoder that puts them on the
        // wire.
        let look = appearance.unwrap_or_else(Appearance::default_human);
        let body = Body {
            id: look.body,
            hue: look.hue,
        };

        self.state.registry.insert(entity, Position(position));
        self.state.registry.insert(entity, Heading(facing));
        self.state.registry.insert(entity, body);
        self.state.registry.insert(entity, Name(name.0.clone()));
        // A character in the world exists, whatever the roster had heard about it
        // before. That is not a formality: a character created this run enters
        // once and is not described anywhere until it logs out, so without this
        // the list the character screen draws would be missing the character the
        // player is playing. Idempotent — the common case is a name already
        // enrolled by boot — and it deliberately records nothing about *where*
        // the character is; the logout does that.
        self.roster.enrol(&account, &name);
        self.state.registry.insert(entity, Account(account.clone()));
        self.state.registry.insert(entity, facet);
        // The account's authority, re-derived each login and never saved with the
        // character — so it is what the GM command gate reads.
        self.state.registry.insert(entity, Access(access));
        // And its staff *mode*, on by default so a staff login behaves as it
        // always has. `.gm` takes it off to walk under a player's rules; the
        // authority above is untouched, which is what lets `.gm` put it back.
        if access >= AccessLevel::GameMaster {
            self.state
                .registry
                .insert(entity, openshard_state::components::Staff);
        }
        // Strength caps hit points, intelligence caps mana — the first derived
        // numbers. Character creation will choose the stats; until it does, the
        // defaults reproduce the flat hundreds the world had before.
        self.state.registry.insert(
            entity,
            Stats {
                strength: DEFAULT_HITPOINTS,
                dexterity: DEFAULT_DEXTERITY,
                intelligence: DEFAULT_MANA,
            },
        );
        self.state.registry.insert(
            entity,
            Hitpoints {
                current: DEFAULT_HITPOINTS,
                max: DEFAULT_HITPOINTS,
            },
        );
        self.state.registry.insert(
            entity,
            Mana {
                current: DEFAULT_MANA,
                max: DEFAULT_MANA,
            },
        );
        // Stamina is dexterity's pool, and starts full: the client reads its
        // run-eligibility from here, so anything less than max needlessly slows
        // the first run out of the gate.
        self.state.registry.insert(
            entity,
            Stamina {
                current: DEFAULT_DEXTERITY,
                max: DEFAULT_DEXTERITY,
            },
        );
        // Did this character log out dead? Read it before the sheet is consumed;
        // the ghost state is re-applied at the end, once the body and inventory
        // (its saved death shroud included) are in place.
        let logged_out_dead = sheet.as_ref().is_some_and(|s| s.dead);
        // A created or restored character brings its own stats and skills, over
        // the flat defaults above: strength re-caps hit points, intelligence
        // mana, and the trained skills (with their lock arrows) come back as they
        // were chosen or last stood. A bare enter (a test, an old save) keeps the
        // defaults and no skills.
        if let Some(sheet) = sheet {
            self.state.registry.insert(
                entity,
                Stats {
                    strength: sheet.strength.max(1),
                    dexterity: sheet.dexterity,
                    intelligence: sheet.intelligence,
                },
            );
            self.state.registry.insert(
                entity,
                Hitpoints {
                    current: sheet.strength.max(1),
                    max: sheet.strength.max(1),
                },
            );
            self.state.registry.insert(
                entity,
                Mana {
                    current: sheet.intelligence,
                    max: sheet.intelligence,
                },
            );
            self.state.registry.insert(
                entity,
                Stamina {
                    current: sheet.dexterity,
                    max: sheet.dexterity,
                },
            );
            // Standing. Zero is the same as absent everywhere that reads these, so a
            // fresh character carries neither component and an old save reads as one.
            if sheet.fame != 0 {
                self.state
                    .registry
                    .insert(entity, openshard_state::components::Fame(sheet.fame));
            }
            if sheet.karma != 0 {
                self.state
                    .registry
                    .insert(entity, openshard_state::components::Karma(sheet.karma));
            }
            if sheet.murders != 0 {
                self.state
                    .registry
                    .insert(entity, openshard_state::components::Murders(sheet.murders));
            }
            // Guild membership, straight back on as the component it was swept
            // from. Not checked against the guild table here: `guild_of` already
            // reads a membership naming a guild that is gone as no membership, so
            // a guild dropped by hand from the database orphans nobody, and a
            // check here would instead throw the membership away for good.
            if let Some(seat) = sheet.guild {
                self.state.registry.insert(
                    entity,
                    openshard_state::components::GuildMember {
                        guild: seat.guild,
                        title: seat.title,
                        rank: seat.rank,
                    },
                );
            }
            if let Some(guild) = sheet.guild_candidate {
                self.state.registry.insert(
                    entity,
                    openshard_state::components::GuildCandidate {
                        guild: openshard_state::GuildId(guild),
                    },
                );
            }
            if !sheet.skills.is_empty() {
                let mut skills = openshard_state::components::Skills::default();
                let shard_cap = self.state.gameplay.skill_cap;
                for (id, value, lock, cap) in sheet.skills {
                    let Some(skill) = openshard_state::skill::Skill::from_id(id) else {
                        continue;
                    };
                    skills.set(skill, value);
                    skills.set_lock(skill, lock);
                    skills.set_cap(skill, if cap == 0 { shard_cap } else { cap });
                }
                self.state.registry.insert(entity, skills);
            }
            // A poison (and, later, the buffs and debuffs beside it) comes back
            // on relog — logging out and in is not a cure. Its pulses resume from
            // this tick.
            let now = self.state.ticks;
            // The stat arrows the player set, and how long ago each stat rose.
            // The saved value is an *age*, so it is added back onto the current
            // tick: a stamp from the previous run would sit in this one's future
            // and freeze the stat until the counter caught up.
            let locks = sheet.stat_locks;
            self.state.registry.insert(
                entity,
                openshard_state::components::StatLocks {
                    strength: openshard_state::StatLock::from_bits(locks.strength),
                    dexterity: openshard_state::StatLock::from_bits(locks.dexterity),
                    intelligence: openshard_state::StatLock::from_bits(locks.intelligence),
                },
            );
            // An age of zero is not "rose zero ticks ago" — it is
            // `StatLockRecord::default()`'s sentinel for "never gained", the same
            // one `claim_stat_timer` reads a missing component as. Feeding it
            // through `now - age` like a real age would land on `now` itself: a
            // stat that has never risen would read as having just risen, and a
            // brand-new character would carry a full cooldown into its first
            // login. Zero must restore to zero, not to now.
            let restore_gain = |age: u64| {
                if age == 0 {
                    openshard_state::WorldTick::ZERO
                } else {
                    openshard_state::WorldTick::from_raw(
                        now.saturating_sub(openshard_state::WorldTick::from_raw(age)),
                    )
                }
            };
            self.state.registry.insert(
                entity,
                openshard_state::components::LastStatGain {
                    strength: restore_gain(locks.strength_age),
                    dexterity: restore_gain(locks.dexterity_age),
                    intelligence: restore_gain(locks.intelligence_age),
                },
            );
            Self::apply_effects(&mut self.state.registry, entity, &sheet.effects, now);
            // And the quest log, with every clock re-based on this tick — a relog
            // is not a way to reset a cooldown or stop a timer.
            Self::apply_quests(
                &mut self.state.registry,
                entity,
                &sheet.quests,
                &sheet.done_quests,
                now,
            );
        }
        self.state.registry.insert(entity, Combat::default());
        self.state.registry.insert(entity, Notoriety::Innocent);
        self.state.registry.insert(entity, Resistance::none());
        // No explicit `MeleeDamage` or `SwingSpeed`: a player's blow and pace come
        // from the weapon they wield (or the bare-hands default), derived fresh in
        // `melee_blow`/`swing_speed`. Those components stay a creature's/script's
        // override — a fixed one here would make every weapon hit for the same 5.
        self.state
            .registry
            .insert(entity, Movement(Walker::new(position, facing)));
        self.state.registry.insert(entity, Client { connection });
        self.state.players.insert(connection, entity);
        // A character can reach the world without a hand-off — every test that
        // queues an `Enter` of its own does — so the session row is written here
        // too. See `attach`.
        self.attach(connection, version, account, access);
        // The AoS feature gates for this connection, at debug — tooltips and
        // context menus are version-gated, so this is where to look if a modern
        // client unexpectedly shows no hover names.
        debug!(
            %connection,
            version = ?version,
            tooltips = version.supports(Feature::Tooltips),
            tooltip_hash = version.supports(Feature::TooltipHash),
            context_menu = version.supports(Feature::NewContextMenu),
            tooltip_mode = ?self.state.gameplay.tooltip_mode,
            context_menus = self.state.gameplay.context_menus,
            "player feature gates"
        );
        self.state.place_mobile(facet, entity, position);
        self.state.seen.insert(entity, HashSet::new());

        // Bring back what this character was carrying, if the store had it. A
        // returning character re-equips its saved backpack, bank box and gear; a
        // new one has nothing waiting.
        let restored = self.restore_inventory(serial);

        // Every character wears a backpack. Without it the paperdoll's bag is dead
        // and there is nowhere to put anything picked up. Equipped before the
        // packets go out so it rides in the `0x78` that tells the client — and
        // everyone watching — what this mobile is wearing. A returning character's
        // backpack came back with its inventory; only a character that restored
        // none — a brand-new one, or one whose save predates item persistence —
        // gets a fresh starter bag.
        let has_backpack = self
            .state
            .registry
            .query::<Equipped>()
            .any(|(_, worn)| worn.mobile == serial && worn.layer == items::BACKPACK_LAYER);
        if !restored || !has_backpack {
            items::equip_new_container(
                &mut self.state,
                serial,
                BACKPACK_GRAPHIC,
                BACKPACK_GUMP,
                Hue(0),
                items::BACKPACK_LAYER,
            );
        }

        // And a bank box, on the bank layer. Like the backpack it is worn, so it
        // persists with the character and its contents survive a restart — which is
        // what makes a bank worth anything. A returning character's came back with
        // its saved inventory; a new one gets an empty one.
        let has_bank = self
            .state
            .registry
            .query::<Equipped>()
            .any(|(_, worn)| worn.mobile == serial && worn.layer == npc::BANK_LAYER);
        if !has_bank {
            items::equip_new_container(
                &mut self.state,
                serial,
                npc::BANK_GRAPHIC,
                npc::BANK_GUMP,
                Hue(0),
                npc::BANK_LAYER,
            );
        }

        // The order is the client's, not ours. 0x1B must come first — until it
        // lands there is no body to attach anything to — and 0x55 must come
        // last, because it is what tells the client to start drawing. What is
        // between can be reordered; the two ends cannot.
        // The size of the facet this character is actually on, not Britannia's.
        // The facets are not all the same shape — Ilshenar is 2304×1600 and
        // Tokuno 1448×1448 — and a client told the world is 7168×4096 when it is
        // not draws the edge of it wherever it likes. On Felucca and Trammel that
        // is also true of the *client's own version* — see
        // `MapSize::for_client`/`ClientVersion::WIDE_MAP`.
        let map = {
            let state = self.state.facet_state(facet);
            MapSize::for_client(facet, state.width(), state.height(), version)
        };
        self.state.send_packet(
            connection,
            &ServerPacket::PlayerStart(PlayerStart {
                serial,
                body: body.id,
                position,
                facing,
                map,
            }),
        );
        self.state
            .send_packet(connection, &ServerPacket::MapChange(MapChange { map: facet }));
        // AoS SupportedFeatures, sent *again* at world entry — this is the copy
        // ServUO's `DoLogin` sends right after the login confirm, and the one
        // ClassicUO reads to turn on in-world object tooltips and context menus.
        // The `0xB9` sent before the character list only configures the
        // character-select screen; without this one a modern client never asks for
        // an OPL or opens a context menu, no matter its version. Off only when the
        // shard serves neither.
        if self.state.gameplay.tooltip_mode != TooltipMode::Off || self.state.gameplay.context_menus {
            let extended = version.supports(Feature::ExtraFeatureMask);
            self.state.send(
                connection,
                encode_supported_features(SupportedFeatures::AOS, extended),
            );
        }
        // Which season the client draws its trees in, with the change sound off:
        // this is a login, not a turn of the year. Between the map change and the
        // player update, where ServUO's `Mobile.SendEverything` puts it — the
        // client redraws the world for the season, so it wants to know before it
        // is told where the player stands.
        self.state.send_packet(
            connection,
            &ServerPacket::SeasonChange(SeasonChange {
                // Already a `Season` on `Gameplay`, which `openshard_config`
                // has refused to let past four since it was parsed.
                season: self.state.gameplay.season,
                play_sound: false,
            }),
        );
        // The flag byte this body would be described to anybody else with —
        // `WorldState::stance_of`, which says why the `0x20` carries it too.
        // A game master logging in learns from this packet, and from nothing
        // else that survives their first step, that they walk through bodies.
        let flags = self.state.stance_of(entity);
        self.state.send_packet(
            connection,
            &ServerPacket::PlayerUpdate(PlayerUpdate {
                serial,
                body: body.id,
                hue: body.hue,
                flags,
                position,
                facing,
            }),
        );
        // The light where this character actually is and at the hour it actually
        // is — the region's dark, a relogged Night Sight, or the time of day. One
        // rule computes it, here and on every tick after; remembering it here is
        // what stops the refresh pass sending it a second time immediately.
        let level = self.initial_light(connection);
        self.state
            .send_packet(connection, &ServerPacket::LightLevel(LightLevel { level }));
        // The status bar, stamina and all. Without it the client believes it has
        // zero stamina and refuses to run — see `MobileStatus`. Sent before the
        // login-complete that starts the client drawing, so the numbers are there
        // the moment the paperdoll can be opened.
        self.send_status(connection, entity);
        // The skill window's full contents, so it is filled the moment the player
        // opens it — ServUO sends `SkillUpdate` on world entry the same way.
        self.send_skills(connection, entity);
        // And the three stat arrows, which come out of their own packet: a client
        // that never receives one draws every arrow pointing up, whatever the
        // character actually saved.
        self.send_stat_locks(connection, entity);
        // The player's own `0x78`, so its client learns its equipment — and the
        // serial of the backpack it must be able to double-click open. The client
        // draws its body from `0x1B`, but its worn items come from here; `reveal`
        // sends this mobile to *others*, never to itself, so this is the one place
        // it hears about its own paperdoll.
        if let Some(mine) = self.state.mobile_incoming(entity, entity) {
            self.state
                .send_packet(connection, &ServerPacket::MobileIncoming(mine));
        }
        // What this shard holds the character at, so its client can stop
        // offering staff commands nobody may run. This engine's own `0xBF`
        // subcommand and no reference client's — see `AuthorityNotice`, which
        // is also where "a stock client ignores it" is argued.
        //
        // The *authority* and not the staff mode: `.gm` moves the mode all
        // session and this never moves at all, which is why it is sent once,
        // here, and never refreshed.
        self.state.send_packet(
            connection,
            &ServerPacket::AuthorityNotice(AuthorityNotice {
                level: self.state.access_level(entity),
            }),
        );
        // And what world this shard thinks it is standing in, for the same
        // reason and in the same place: this engine's own `0xBF`, sent once,
        // because it is what a client of ours needs *before* it can ask for a
        // chunk of ground. See `openshard_protocol::chunks`, and
        // `docs/map/new_map_representation/to_the_client.md` for the plan it
        // opens.
        if let Some(notice) = self.world_notice(entity) {
            self.state
                .send_packet(connection, &ServerPacket::WorldNotice(notice));
        }
        self.state
            .send_packet(connection, &ServerPacket::LoginComplete(LoginComplete));

        self.state.bus.send(PlayerEntered {
            connection,
            entity,
            serial,
            position,
        });
        info!(%serial, %name, position = %position, "in world");

        // Draw whoever is already here, and draw this one for them. Both
        // directions, because arriving is symmetric: the newcomer has an empty
        // screen and everyone nearby has a gap where it now stands.
        self.state.refresh_around(entity);

        // A character that logged out dead comes back a ghost. Applied last, after
        // the living body was drawn and the saved death shroud re-equipped: it
        // swaps in the grey body, remembers the living one for resurrection, tells
        // the client it is dead, and redraws (the living forget it, ghosts see it).
        // No corpse is laid — that one still lies where it fell, a saved item.
        if logged_out_dead {
            self.enter_ghost_state(entity, serial, false);
        }
        Ok(())
    }

    /// Send a player its own `0x11` status — the paperdoll numbers, and the only
    /// packet that carries stamina. A client with no status believes it has zero
    /// stamina and will only ever walk, so this goes out on world entry and again
    /// whenever the client asks (`0x34`).
    ///
    /// The numbers themselves are [`World::status_of`]'s: stats and pools read off
    /// components, gold, weight, armour and followers derived from what the
    /// character carries, wears and rides.
    pub(super) fn send_status(&mut self, connection: ConnectionId, entity: EntityId) {
        let Some(status) = self.status_of(entity) else {
            return;
        };
        self.state
            .send_packet(connection, &ServerPacket::MobileStatus(status));
    }

    /// Redraw a mobile's own status bar (`0x11`), if it is a connected player.
    ///
    /// Str/dex/int and the maxima do not move in ordinary play, so nothing
    /// re-sends the status but this — a stat buff landing, or wearing off. An NPC,
    /// or a player between sessions, is a no-op.
    pub(super) fn refresh_status_of(&mut self, serial: Serial) {
        let Some(entity) = self.state.registry.entity_of(serial) else {
            return;
        };
        if let Some(connection) = self.state.connection_of(entity) {
            self.send_status(connection, entity);
        }
    }
}
