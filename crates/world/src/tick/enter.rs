use super::*;

impl World {
    /// The facet a mobile is on, or the default if it carries none.
    ///
    /// Always a facet the world actually has: [`enter`](Self::enter) clamps an
    /// unloaded facet to the default before it ever reaches a `Facet` component,
    pub(super) fn enter(&mut self, entering: Entering) {
        let Entering {
            connection,
            version,
            account,
            name,
            serial,
            position,
            facet,
            appearance,
            sheet,
            access,
        } = entering;
        if self.state.players.contains_key(&connection) {
            warn!(%connection, "already in the world");
            return;
        }

        // A character can only stand on a facet the shard loaded. An unloaded one
        // — a save from a shard that had more facets, say — falls back to the
        // default rather than leaving the character nowhere.
        let facet = if self.state.facets.contains_key(&facet) {
            facet
        } else {
            warn!(%connection, facet, "unloaded facet; falling back to the default");
            self.state.default_facet
        };

        // A stored character comes back on the serial it was saved under; a new
        // one takes a fresh serial from the pool. The saved serial was reserved
        // at boot (see `World::reserve_serial`), so binding it here cannot collide.
        let (entity, serial) = match serial.and_then(Serial::new) {
            Some(saved) => {
                let entity = self.state.registry.spawn();
                if let Err(error) = self.state.registry.bind_serial(entity, saved) {
                    warn!(%connection, ?error, "could not restore the saved serial");
                    self.state.registry.despawn(entity);
                    return;
                }
                (entity, saved)
            }
            None => match self.state.registry.spawn_with_serial(SerialKind::Mobile) {
                Ok(pair) => pair,
                Err(_) => {
                    warn!(%connection, "the mobile serial pool is exhausted");
                    return;
                }
            },
        };

        // A loaded character spawns exactly where it was saved, its own z
        // included; a fresh one takes the world's configured start on its facet.
        let position = position.unwrap_or_else(|| self.state.start_position(facet));
        let facing = Facing::walking(Direction::South);
        // A created or loaded character brings its body and hue; without one it
        // falls back to the default.
        let body = Body {
            id: appearance.map_or(BODY_HUMAN_MALE, |look| look.body),
            hue: appearance.map_or(DEFAULT_HUE, |look| look.hue),
        };

        self.state.registry.insert(entity, Position(position));
        self.state.registry.insert(entity, Heading(facing));
        self.state.registry.insert(entity, body);
        self.state.registry.insert(entity, Name(name.clone()));
        self.state.registry.insert(entity, Account(account));
        self.state.registry.insert(entity, Facet(facet));
        // The account's authority, re-derived each login and never saved with the
        // character — so it is what the GM command gate reads.
        self.state.registry.insert(entity, Access(access));
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
            if !sheet.skills.is_empty() {
                let mut skills = openshard_state::components::Skills::default();
                for (id, value, lock) in sheet.skills {
                    skills.set(id, value);
                    skills.set_lock(id, lock);
                }
                self.state.registry.insert(entity, skills);
            }
            // A poison (and, later, the buffs and debuffs beside it) comes back
            // on relog — logging out and in is not a cure. Its pulses resume from
            // this tick.
            let now = self.state.ticks;
            Self::apply_effects(&mut self.state.registry, entity, &sheet.effects, now);
            // The saved quest log rides back onto the character as an opaque blob;
            // the pack reads it when the login's `QuestLoaded` event reaches it.
            if !sheet.quest_blob.is_empty() {
                self.state.registry.insert(
                    entity,
                    openshard_state::components::QuestLog(sheet.quest_blob),
                );
            }
        }
        self.state.registry.insert(entity, Combat::default());
        self.state.registry.insert(entity, Notoriety::Innocent);
        self.state.registry.insert(
            entity,
            MeleeDamage {
                amount: combat::SWING_DAMAGE,
            },
        );
        self.state.registry.insert(entity, Resistance::default());
        // No explicit `SwingSpeed`: a player swings at the pace their dexterity
        // dictates, through `swing_speed`.
        self.state
            .registry
            .insert(entity, Movement(Walker::new(position, facing)));
        self.state.registry.insert(
            entity,
            Client {
                connection,
                version,
            },
        );
        self.state.players.insert(connection, entity);
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
        self.state
            .facet_state_mut(facet)
            .sectors
            .insert(entity, position);
        self.state.seen.insert(entity, HashSet::new());

        // Bring back what this character was carrying, if the store had it. A
        // returning character re-equips its saved backpack, bank box and gear; a
        // new one has nothing waiting.
        let restored = self.restore_inventory(serial.raw());

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
            .any(|(_, worn)| worn.mobile == serial && worn.layer == BACKPACK_LAYER);
        if !restored || !has_backpack {
            items::equip_new_container(
                &mut self.state,
                serial,
                BACKPACK_GRAPHIC,
                BACKPACK_GUMP,
                0,
                BACKPACK_LAYER,
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
                0,
                npc::BANK_LAYER,
            );
        }

        // The order is the client's, not ours. 0x1B must come first — until it
        // lands there is no body to attach anything to — and 0x55 must come
        // last, because it is what tells the client to start drawing. What is
        // between can be reordered; the two ends cannot.
        self.state.send(
            connection,
            PlayerStart {
                serial: serial.raw(),
                body: body.id,
                position,
                facing,
                map_width: DEFAULT_MAP_WIDTH,
                map_height: DEFAULT_MAP_HEIGHT,
            }
            .encode(),
        );
        self.state.send(connection, encode_map_change(facet));
        // AoS SupportedFeatures, sent *again* at world entry — this is the copy
        // ServUO's `DoLogin` sends right after the login confirm, and the one
        // ClassicUO reads to turn on in-world object tooltips and context menus.
        // The `0xB9` sent before the character list only configures the
        // character-select screen; without this one a modern client never asks for
        // an OPL or opens a context menu, no matter its version. Off only when the
        // shard serves neither.
        if self.state.gameplay.tooltip_mode != TooltipMode::Off || self.state.gameplay.context_menus
        {
            let extended = version.supports(Feature::ExtraFeatureMask);
            self.state.send(
                connection,
                encode_supported_features(AOS_FEATURE_FLAGS, extended),
            );
        }
        self.state.send(
            connection,
            PlayerUpdate {
                serial: serial.raw(),
                body: body.id,
                hue: body.hue,
                flags: 0,
                position,
                facing,
            }
            .encode(),
        );
        self.state.send(connection, encode_light_level(LIGHT_DAY));
        // A relogged Night Sight is still lit — the buff persisted (restored above)
        // — so re-send the bright personal light over the ambient just sent.
        if magic::behaviour_buff(&self.state, entity, openshard_state::effect::NIGHT_SIGHT)
            .is_some()
        {
            self.state
                .send(connection, encode_light_level(LIGHT_NIGHTSIGHT));
        }
        // The status bar, stamina and all. Without it the client believes it has
        // zero stamina and refuses to run — see `MobileStatus`. Sent before the
        // login-complete that starts the client drawing, so the numbers are there
        // the moment the paperdoll can be opened.
        self.send_status(connection, entity);
        // The skill window's full contents, so it is filled the moment the player
        // opens it — ServUO sends `SkillUpdate` on world entry the same way.
        self.send_skills(connection, entity);
        // The player's own `0x78`, so its client learns its equipment — and the
        // serial of the backpack it must be able to double-click open. The client
        // draws its body from `0x1B`, but its worn items come from here; `reveal`
        // sends this mobile to *others*, never to itself, so this is the one place
        // it hears about its own paperdoll.
        if let Some(mine) = self.state.mobile_incoming(entity) {
            self.state.send(connection, mine.encode(version));
        }
        self.state.send(connection, encode_login_complete());

        self.state.bus.send(PlayerEntered {
            entity,
            serial,
            position,
        });
        info!(%serial, name, position = %position, "in world");

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
    }

    /// Send a player its own `0x11` status — the paperdoll numbers, and the only
    /// packet that carries stamina. A client with no status believes it has zero
    /// stamina and will only ever walk, so this goes out on world entry and again
    /// whenever the client asks (`0x34`). Reads the mobile's own components;
    /// stamina tracks dexterity, as it does in UO, until a stamina system exists.
    pub(super) fn send_status(&mut self, connection: ConnectionId, entity: EntityId) {
        let Some(Client { version, .. }) = self.state.registry.get::<Client>(entity).copied()
        else {
            return;
        };
        let Some(serial) = self.state.registry.serial_of(entity) else {
            return;
        };
        let name = self
            .state
            .registry
            .get::<Name>(entity)
            .map_or_else(String::new, |n| n.0.clone());
        let stats = self.state.registry.get::<Stats>(entity).copied();
        let hits = self.state.registry.get::<Hitpoints>(entity).copied();
        let mana = self.state.registry.get::<Mana>(entity).copied();
        let stamina = self.state.registry.get::<Stamina>(entity).copied();
        let (strength, dexterity, intelligence) = stats
            .map_or((DEFAULT_HITPOINTS, DEFAULT_DEXTERITY, DEFAULT_MANA), |s| {
                (s.strength, s.dexterity, s.intelligence)
            });
        let (hits_now, hits_max) = hits.map_or((DEFAULT_HITPOINTS, DEFAULT_HITPOINTS), |h| {
            (h.current, h.max)
        });
        let (mana_now, mana_max) =
            mana.map_or((DEFAULT_MANA, DEFAULT_MANA), |m| (m.current, m.max));
        // The real pool if the mobile carries one; otherwise dexterity, so an NPC
        // or a bare test mobile still reads as able to run.
        let (stamina_now, stamina_max) =
            stamina.map_or((dexterity, dexterity), |s| (s.current, s.max));

        let status = MobileStatus {
            serial: serial.raw(),
            name,
            hits: hits_now,
            hits_max,
            female: false,
            strength,
            dexterity,
            intelligence,
            stamina: stamina_now,
            stamina_max,
            mana: mana_now,
            mana_max,
            gold: 0,
            armor: 0,
            // A body's own weight, well under the cap: an overloaded client will
            // not run either, so this is deliberately light until an inventory
            // weight system replaces it.
            weight: BODY_WEIGHT,
            max_weight: max_weight(strength),
            stat_cap: STAT_CAP,
            followers: 0,
            followers_max: MAX_FOLLOWERS,
        };
        self.state.send(connection, status.encode(version));
    }

    /// The connection a mobile is played over, if it is a connected player.
    pub(super) fn connection_of(&self, entity: EntityId) -> Option<ConnectionId> {
        self.state
            .players
            .iter()
            .find(|(_, &e)| e == entity)
            .map(|(&connection, _)| connection)
    }

    /// Redraw a mobile's own status bar (`0x11`), if it is a connected player.
    ///
    /// Str/dex/int and the maxima do not move in ordinary play, so nothing
    /// re-sends the status but this — a stat buff landing, or wearing off. An NPC,
    /// or a player between sessions, is a no-op.
    pub(super) fn refresh_status_of(&mut self, serial: u32) {
        let Some(entity) = Serial::new(serial).and_then(|s| self.state.registry.entity_of(s))
        else {
            return;
        };
        if let Some(connection) = self.connection_of(entity) {
            self.send_status(connection, entity);
        }
    }
}
