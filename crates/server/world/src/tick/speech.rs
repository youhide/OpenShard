use super::*;
use openshard_protocol::wire::Hue;

impl World {
    /// A player's speech, with staff commands split off the front. A
    /// `.`-prefixed line from a game master runs as a command and never reaches
    /// anyone's screen; from an ordinary player it is just speech, so a player can
    /// still say ".hello" out loud. The authority gate lives here, not in `gm`,
    /// so the command module can assume a call is already cleared.
    ///
    /// The seam where the client's three header values stop being raw. `mode` is
    /// class B and interprets totally; `hue` and `font` are class C and their
    /// checks — which set of hues, which of the client's faces this shard allows —
    /// are content that does not exist in this repo yet, so they pass through
    /// unchecked and *visibly* so. See `docs/protocol_newtypes.md`: a `.0` with no
    /// `validate` beside it is the grep the plan asks for.
    pub(super) fn say(
        &mut self,
        connection: ConnectionId,
        mode: RawTalkMode,
        hue: RawHue,
        font: RawFont,
        text: String,
    ) {
        let mode = mode.interpret();
        let hue = Hue(hue.0); // unchecked: no allowed-hue set exists yet
        let font = Font(font.0); // unchecked: the client's font count is its own
        if let Some(rest) = text.strip_prefix(gm::COMMAND_PREFIX) {
            if let Some(&actor) = self.state.players.get(&connection) {
                // The *authority*, not the staff mode: a game master who has
                // turned their mode off with `.gm` still commands, which is what
                // lets them turn it back on.
                if self.state.staff_authority(actor) {
                    // `.res` resurrects the actor. Handled here, not in `gm`:
                    // resurrection redraws the ghost across every screen, a
                    // World-level operation, while `gm::run` works on `WorldState`.
                    // The one-account way to test the ghost round-trip solo.
                    if rest
                        .split_whitespace()
                        .next()
                        .is_some_and(|c| c.eq_ignore_ascii_case("res"))
                    {
                        if self.state.registry.has::<Ghost>(actor) {
                            self.resurrect(actor, true);
                            self.notify_self(actor, "You are alive again.");
                        } else {
                            self.notify_self(actor, "You are not dead.");
                        }
                        return;
                    }
                    // `.moongates` lays the city gates. Handled here for the same
                    // reason `.res` is: it draws items across every screen, which
                    // is a World operation, and `gm::run` works on `WorldState`.
                    if rest
                        .split_whitespace()
                        .next()
                        .is_some_and(|c| c.eq_ignore_ascii_case("moongates"))
                    {
                        let placed = self.place_public_moongates();
                        self.notify_self(
                            actor,
                            &format!("Placed {placed} moongate(s); the rest already stood."),
                        );
                        return;
                    }
                    gm::run(&mut self.state, actor, rest);
                    return;
                }
            }
        }
        // Guild and alliance lines leave here and never reach `chat::say`: they
        // pick their listeners by membership, and everything below this measures
        // a distance. Branching *before* the broadcast rather than teaching the
        // broadcast a third rule is the whole of why it is safe — a guild line
        // that fell through would be a private one said out loud in the street.
        if matches!(mode, TalkMode::Guild | TalkMode::Alliance) {
            if let Some(&actor) = self.state.players.get(&connection) {
                let spoken = match mode {
                    TalkMode::Alliance => {
                        openshard_guilds::say_to_alliance(&mut self.state, actor, hue, font, &text)
                    }
                    _ => openshard_guilds::say_to_guild(&mut self.state, actor, hue, font, &text),
                };
                if let Err(refusal) = spoken {
                    self.state.system_message(actor, refusal.message());
                }
            }
            return;
        }
        chat::say(&mut self.state, connection, mode, hue, font, &text);

        // Townsperson services triggered by keyword: saying "bank" near a banker
        // opens your bank box. The words were still spoken above, so it reads as a
        // request the banker answers, not a hidden command.
        if let Some(&actor) = self.state.players.get(&connection) {
            npc::banker_keywords(&mut self.state, connection, actor, &text);
            // And "guards" in a guarded region calls one down on whoever nearby
            // has earned it. Same shape: the word was spoken, and the town
            // answers it.
            npc::guard_keywords(&mut self.state, connection, actor, &text);
            // And the townsfolk in earshot hear it: a shop keyword opens a shop, a
            // trade keyword earns an answer. ServUO's `VendorAI.OnSpeech`, bounded
            // to its four tiles.
            //
            // This replaced a substring test on the whole line — `contains("sell")`
            // — under which "that sword is unsellable" opened a buy-back list, and
            // a bare "sell" opened the shop of whichever vendor happened to be
            // nearest in a crowded bank. Keywords are whole words now, and a bare
            // "buy"/"sell" needs the shopkeeper named.
            npc::overhear(&mut self.state, connection, actor, &text);
            // And your own pets hear you: "all kill", "<name> stay". ServUO matches
            // the client's keyword ids here; this matches the words, because the
            // parser skips the keyword block and the text arrives either way.
            npc::hear_pet_order(&mut self.state, actor, &text);
            // And "quest" near a quest giver offers what it has. The same shape
            // again — the word was spoken, and whoever is standing there answers.
            if let Some(serial) = self.state.registry.serial_of(actor) {
                quests::speech_offer(&mut self.state, serial, &text);
            }
        }
    }

    /// Answer a single-click (`0x09`): draw the clicked object's name over it,
    /// seen only by the asker.
    ///
    /// A named mobile — a townsperson, a player — labels in its notoriety colour.
    /// A plain item labels in the default text hue with its tiledata name (the
    /// classic 2D "tooltip": what a modern client shows on hover, this client asks
    /// for a click at a time). A nameless creature or an item on an unmapped world
    /// says nothing rather than a blank label. Mirrors Sphere's `addCharName` /
    /// `addItemName`.
    pub(super) fn single_click(&mut self, connection: ConnectionId, serial: Serial) {
        let Some(target) = self.state.registry.entity_of(serial) else {
            return;
        };
        let asker = self.state.players.get(&connection).copied();

        // The guild line first, above the name: `[Warlord, OSS]` in the speech
        // hue, ServUO's `Mobile.OnSingleClick`. Its own label rather than part of
        // the name, so the name below keeps the notoriety hue and the two read as
        // two different things.
        if let Some(label) = self.state.guild_label(target) {
            let packet = ServerPacket::SpokenMessage(SpokenMessage {
                serial: Some(serial),
                graphic: self.state.registry.get::<Body>(target).map(|body| body.id),
                mode: TalkMode::Label,
                hue: TEXT_HUE,
                font: Font::DEFAULT,
                name: String::new(),
                text: label,
            });
            self.state.send_packet(connection, &packet);
        }

        // A mobile carries a `Name` and a `Body`; an item a `Drawn` and no
        // `Name`. The two cases pick a different graphic, hue, and name source.
        let (graphic, hue, text) = if let Some(name) = self.state.registry.get::<Name>(target) {
            // The earned name, not the bare one: ServUO shows a fame title to an
            // onlooker once the mobile's fame reaches 5000.
            let name = openshard_state::titled_name(&self.state, target, &name.0.clone());
            let Some(body) = self.state.registry.get::<Body>(target).map(|b| b.id) else {
                return;
            };
            // Relative, like the bar: `Notoriety.Compute(from, this)` is what
            // ServUO hues this label with, so a guildmate's name reads green to
            // you and blue to the stranger clicking beside you. Falls back to the
            // absolute standing when the asker is nobody — a click can only
            // arrive from a connection, so that is a spectator case, not a
            // player one.
            let hue = asker
                .map_or_else(
                    || self.state.notoriety_of(target),
                    |asker| self.state.notoriety_toward(asker, target),
                )
                .name_hue();
            (body, hue, name)
        } else {
            let Some(&Drawn { id, .. }) = self.state.registry.get::<Drawn>(target) else {
                return;
            };
            let Some(name) = self
                .state
                .tiles
                .as_deref()
                .and_then(|tiles| tiles.item_name(id.0))
            else {
                return;
            };
            // Resolve the tiledata name's `%s%` pluralisation markers, then read
            // "3 bolts of cloth" for a stack, "a bolt of cloth" for a single —
            // Sphere's `GetNameFull`. The amount count decides both the markers and
            // the prefix.
            let amount = self.state.registry.get::<Amount>(target).map_or(1, |a| a.0);
            let resolved = openshard_uofiles::tiledata::pluralize_name(name, amount > 1);
            let text = if amount > 1 {
                format!("{amount} {resolved}")
            } else {
                resolved
            };
            (id, TEXT_HUE, text)
        };

        // The object's own serial makes the client draw the text over it; an empty
        // speaker name and the label mode make it a name tag, not speech.
        let packet = ServerPacket::SpokenMessage(SpokenMessage {
            serial: Some(serial),
            graphic: Some(graphic),
            mode: TalkMode::Label,
            hue,
            font: Font::DEFAULT,
            name: String::new(),
            text,
        });
        self.state.send_packet(connection, &packet);
    }

    /// Answer an AoS tooltip request (`0xD6`): send each named object's property
    /// list back to the asker. The client batches several serials as it hovers; a
    /// serial it cannot see or that names nothing is simply skipped. Off entirely
    /// when the shard serves no tooltips.
    pub(super) fn query_properties(&mut self, connection: ConnectionId, serials: &[RawSerial]) {
        if self.state.gameplay.tooltip_mode == TooltipMode::Off {
            return;
        }
        for &serial in serials {
            if let Some(entity) = serial.validate().and_then(|s| self.state.registry.entity_of(s)) {
                self.state.send_property_list(connection, entity);
            }
        }
    }
}
