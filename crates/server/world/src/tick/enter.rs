use super::command::GuildSeat;
use super::*;
use openshard_protocol::wire::Hue;

struct ResolvedCharacter {
    requested_facet: Facet,
    saved_serial: Option<Serial>,
    saved_position: Option<Point>,
    saved_facing: Option<Facing>,
    appearance: Option<Appearance>,
    sheet: Option<Box<CharacterSheet>>,
}

struct ActiveEntry {
    connection: ConnectionId,
    version: ClientVersion,
    account: AccountName,
    name: CharacterName,
    access: AccessLevel,
    entity: EntityId,
    serial: Serial,
    facet: Facet,
    position: Point,
    facing: Facing,
    body: Body,
    logged_out_dead: bool,
}

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

        let resolved = self.resolve_character(&account, &name, character);
        let facet = self.entry_facet(connection, resolved.requested_facet);
        let (entity, serial) = self.spawn_entering_mobile(connection, resolved.saved_serial)?;
        let position = self.entry_position(facet, resolved.saved_position);
        let facing = resolved
            .saved_facing
            .unwrap_or_else(|| Facing::walking(Direction::South));
        let look = resolved.appearance.unwrap_or_else(Appearance::default_human);
        let body = Body {
            id: look.body,
            hue: look.hue,
        };
        let logged_out_dead = resolved.sheet.as_ref().is_some_and(|sheet| sheet.dead);
        let entry = ActiveEntry {
            connection,
            version,
            account,
            name,
            access,
            entity,
            serial,
            facet,
            position,
            facing,
            body,
            logged_out_dead,
        };

        self.insert_entry_identity(&entry);
        if let Some(sheet) = resolved.sheet {
            self.restore_character_sheet(entity, *sheet);
        }
        self.activate_entry(&entry);
        self.restore_entry_inventory(serial);
        self.send_entry_packets(&entry);
        self.finish_entry(&entry);
        Ok(())
    }

    fn resolve_character(
        &self,
        account: &AccountName,
        name: &CharacterName,
        character: Character,
    ) -> ResolvedCharacter {
        match character {
            Character::Saved => self
                .roster
                .get(account, name)
                .and_then(StoredCharacter::from_record)
                .map_or_else(
                    || ResolvedCharacter {
                        requested_facet: self.state.default_facet,
                        saved_serial: None,
                        saved_position: None,
                        saved_facing: None,
                        appearance: None,
                        sheet: None,
                    },
                    |stored| ResolvedCharacter {
                        requested_facet: stored.facet,
                        saved_serial: Some(stored.serial),
                        saved_position: Some(stored.position),
                        saved_facing: Some(stored.facing),
                        appearance: Some(stored.appearance),
                        sheet: Some(Box::new(stored.sheet)),
                    },
                ),
            Character::Fresh(fresh) => ResolvedCharacter {
                requested_facet: fresh.facet,
                saved_serial: None,
                saved_position: fresh.start,
                saved_facing: None,
                appearance: fresh.appearance,
                sheet: fresh.sheet,
            },
        }
    }

    fn entry_facet(&self, connection: ConnectionId, requested: Facet) -> Facet {
        if self.state.facets.contains_key(&requested) {
            requested
        } else {
            warn!(
                %connection,
                facet = requested.0,
                "unloaded facet; falling back to the default"
            );
            self.state.default_facet
        }
    }

    fn spawn_entering_mobile(
        &mut self,
        connection: ConnectionId,
        saved_serial: Option<Serial>,
    ) -> Result<(EntityId, Serial), RefusedEntry> {
        if let Some(serial) = saved_serial {
            let entity = self.state.registry.spawn();
            if let Err(error) = self.state.registry.bind_serial(entity, serial) {
                warn!(%connection, ?error, "could not restore the saved serial");
                self.state.registry.despawn(entity);
                return Err(RefusedEntry::SerialInUse);
            }
            return Ok((entity, serial));
        }

        self.state
            .registry
            .spawn_with_serial(SerialKind::Mobile)
            .map_err(|_| {
                warn!(%connection, "the mobile serial pool is exhausted");
                RefusedEntry::NoSerialsLeft
            })
    }

    fn entry_position(&self, facet: Facet, saved: Option<Point>) -> Point {
        saved
            .map(|saved| {
                let tile = Tile::new(saved.x, saved.y);
                openshard_movement::arrival_z(
                    &self.state.footing(facet, Doors::AsTheyStand),
                    tile,
                    i32::from(saved.z),
                    openshard_movement::PLAYER_HEIGHT,
                )
                .and_then(|z| i8::try_from(z).ok())
                .map_or(saved, |z| Point::new(saved.x, saved.y, z))
            })
            .unwrap_or_else(|| self.state.start_position(facet))
    }

    fn insert_entry_identity(&mut self, entry: &ActiveEntry) {
        let entity = entry.entity;
        self.state.registry.insert(entity, Position(entry.position));
        self.state.registry.insert(entity, Heading(entry.facing));
        self.state.registry.insert(entity, entry.body);
        self.state.registry.insert(entity, Name(entry.name.0.clone()));
        self.roster.enrol(&entry.account, &entry.name);
        self.state.registry.insert(entity, Account(entry.account.clone()));
        self.state.registry.insert(entity, entry.facet);
        self.state.registry.insert(entity, Access(entry.access));
        if entry.access >= AccessLevel::GameMaster {
            self.state
                .registry
                .insert(entity, openshard_state::components::Staff);
        }

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
        self.state.registry.insert(
            entity,
            Stamina {
                current: DEFAULT_DEXTERITY,
                max: DEFAULT_DEXTERITY,
            },
        );
    }

    fn restore_character_sheet(&mut self, entity: EntityId, sheet: CharacterSheet) {
        let CharacterSheet {
            strength,
            dexterity,
            intelligence,
            skills: saved_skills,
            stat_locks,
            effects,
            dead: _,
            fame,
            karma,
            murders,
            quests,
            done_quests,
            guild,
            guild_candidate,
        } = sheet;
        let strength = strength.max(1);
        self.state.registry.insert(
            entity,
            Stats {
                strength,
                dexterity,
                intelligence,
            },
        );
        self.state.registry.insert(
            entity,
            Hitpoints {
                current: strength,
                max: strength,
            },
        );
        self.state.registry.insert(
            entity,
            Mana {
                current: intelligence,
                max: intelligence,
            },
        );
        self.state.registry.insert(
            entity,
            Stamina {
                current: dexterity,
                max: dexterity,
            },
        );

        self.restore_character_standing(entity, fame, karma, murders, guild, guild_candidate);

        let mut skills = openshard_state::components::Skills::default();
        let shard_cap = self.state.gameplay.skill_cap;
        for (id, value, lock, cap) in saved_skills {
            let Some(skill) = openshard_state::skill::Skill::from_id(id) else {
                continue;
            };
            skills.set(skill, value);
            skills.set_lock(skill, lock);
            skills.set_cap(skill, if cap == 0 { shard_cap } else { cap });
        }
        self.state.registry.insert(entity, skills);

        let now = self.state.ticks;
        self.restore_stat_training(entity, stat_locks, now);
        Self::apply_effects(&mut self.state.registry, entity, &effects, now);
        Self::apply_quests(&mut self.state.registry, entity, &quests, &done_quests, now);
    }

    fn restore_character_standing(
        &mut self,
        entity: EntityId,
        fame: i32,
        karma: i32,
        murders: u16,
        guild: Option<GuildSeat>,
        guild_candidate: Option<u32>,
    ) {
        if fame != 0 {
            self.state
                .registry
                .insert(entity, openshard_state::components::Fame(fame));
        }
        if karma != 0 {
            self.state
                .registry
                .insert(entity, openshard_state::components::Karma(karma));
        }
        if murders != 0 {
            self.state
                .registry
                .insert(entity, openshard_state::components::Murders(murders));
        }
        if let Some(seat) = guild {
            self.state.registry.insert(
                entity,
                openshard_state::components::GuildMember {
                    guild: seat.guild,
                    title: seat.title,
                    rank: seat.rank,
                },
            );
        }
        if let Some(guild) = guild_candidate {
            self.state.registry.insert(
                entity,
                openshard_state::components::GuildCandidate {
                    guild: openshard_state::GuildId(guild),
                },
            );
        }
    }

    fn restore_stat_training(
        &mut self,
        entity: EntityId,
        locks: openshard_persistence::StatLockRecord,
        now: openshard_state::WorldTick,
    ) {
        self.state.registry.insert(
            entity,
            openshard_state::components::StatLocks {
                strength: openshard_state::StatLock::from_bits(locks.strength),
                dexterity: openshard_state::StatLock::from_bits(locks.dexterity),
                intelligence: openshard_state::StatLock::from_bits(locks.intelligence),
            },
        );
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
    }

    fn activate_entry(&mut self, entry: &ActiveEntry) {
        let entity = entry.entity;
        self.state.registry.insert(entity, Combat::player_entered());
        self.state.registry.insert(entity, Notoriety::Innocent);
        self.state.registry.insert(entity, Resistance::none());
        self.state
            .registry
            .insert(entity, Movement(Walker::new(entry.position, entry.facing)));
        self.state.registry.insert(
            entity,
            Client {
                connection: entry.connection,
            },
        );
        self.state.players.insert(entry.connection, entity);
        self.attach(
            entry.connection,
            entry.version,
            entry.account.clone(),
            entry.access,
        );
        debug!(
            connection = %entry.connection,
            version = ?entry.version,
            tooltips = entry.version.supports(Feature::Tooltips),
            tooltip_hash = entry.version.supports(Feature::TooltipHash),
            context_menu = entry.version.supports(Feature::NewContextMenu),
            tooltip_mode = ?self.state.gameplay.tooltip_mode,
            context_menus = self.state.gameplay.context_menus,
            "player feature gates"
        );
        self.state.place_mobile(entry.facet, entity, entry.position);
        self.state.seen.insert(entity, HashSet::new());
    }

    fn restore_entry_inventory(&mut self, serial: Serial) {
        let restored = self.restore_inventory(serial);
        let has_backpack = openshard_state::equipped_items(&self.state, serial)
            .any(|(_, worn)| worn.layer == items::BACKPACK_LAYER);
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

        let has_bank = openshard_state::equipped_items(&self.state, serial)
            .any(|(_, worn)| worn.layer == npc::BANK_LAYER);
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
    }

    fn send_entry_packets(&mut self, entry: &ActiveEntry) {
        let map = {
            let state = self.state.facet_state(entry.facet);
            MapSize::for_client(entry.facet, state.width(), state.height(), entry.version)
        };
        self.state.send_packet(
            entry.connection,
            &ServerPacket::PlayerStart(PlayerStart {
                serial: entry.serial,
                body: entry.body.id,
                position: entry.position,
                facing: entry.facing,
                map,
            }),
        );
        self.send_entry_environment(entry);
        self.send_entry_character_state(entry);
        self.state
            .send_packet(entry.connection, &ServerPacket::LoginComplete(LoginComplete));
    }

    fn send_entry_environment(&mut self, entry: &ActiveEntry) {
        self.state.send_packet(
            entry.connection,
            &ServerPacket::MapChange(MapChange { map: entry.facet }),
        );
        if self.state.gameplay.tooltip_mode != TooltipMode::Off || self.state.gameplay.context_menus {
            let extended = entry.version.supports(Feature::ExtraFeatureMask);
            self.state.send(
                entry.connection,
                encode_supported_features(SupportedFeatures::AOS, extended),
            );
        }
        self.state.send_packet(
            entry.connection,
            &ServerPacket::SeasonChange(SeasonChange {
                season: self.state.gameplay.season,
                play_sound: false,
            }),
        );
        let weather = self.weather();
        self.state
            .send_packet(entry.connection, &ServerPacket::WeatherChange(weather));
        let flags = self.state.stance_of(entry.entity);
        self.state.send_packet(
            entry.connection,
            &ServerPacket::PlayerUpdate(PlayerUpdate {
                serial: entry.serial,
                body: entry.body.id,
                hue: entry.body.hue,
                flags,
                position: entry.position,
                facing: entry.facing,
            }),
        );
    }

    fn send_entry_character_state(&mut self, entry: &ActiveEntry) {
        let level = self.initial_light(entry.connection);
        self.state
            .send_packet(entry.connection, &ServerPacket::LightLevel(LightLevel { level }));
        self.send_status(entry.connection, entry.entity);
        self.send_skills(entry.connection, entry.entity);
        self.send_stat_locks(entry.connection, entry.entity);
        if let Some(mine) = self.state.mobile_incoming(entry.entity, entry.entity) {
            self.state
                .send_packet(entry.connection, &ServerPacket::MobileIncoming(mine));
        }
        self.state.send_packet(
            entry.connection,
            &ServerPacket::AuthorityNotice(AuthorityNotice {
                level: self.state.access_level(entry.entity),
            }),
        );
        if let Some(notice) = self.world_notice(entry.entity) {
            self.state
                .send_packet(entry.connection, &ServerPacket::WorldNotice(notice));
        }
    }

    fn finish_entry(&mut self, entry: &ActiveEntry) {
        self.state.bus.send(PlayerEntered {
            connection: entry.connection,
            entity: entry.entity,
            serial: entry.serial,
            position: entry.position,
        });
        info!(
            serial = %entry.serial,
            name = %entry.name,
            position = %entry.position,
            "in world"
        );
        self.state.refresh_around(entry.entity);
        if entry.logged_out_dead {
            self.enter_ghost_state(entry.entity, entry.serial, false);
        }
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
