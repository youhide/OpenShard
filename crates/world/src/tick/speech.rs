use super::*;

impl World {
    /// A player's speech, with staff commands split off the front. A
    /// `.`-prefixed line from a game master runs as a command and never reaches
    /// anyone's screen; from an ordinary player it is just speech, so a player can
    /// still say ".hello" out loud. The authority gate lives here, not in `gm`,
    /// so the command module can assume a call is already cleared.
    pub(super) fn say(
        &mut self,
        connection: ConnectionId,
        mode: u8,
        hue: u16,
        font: u16,
        text: String,
    ) {
        if let Some(rest) = text.strip_prefix(gm::COMMAND_PREFIX) {
            if let Some(&actor) = self.state.players.get(&connection) {
                let is_gm = self
                    .state
                    .registry
                    .get::<Access>(actor)
                    .is_some_and(|access| access.0 >= AccessLevel::GameMaster);
                if is_gm {
                    gm::run(&mut self.state, actor, rest);
                    return;
                }
            }
        }
        chat::say(&mut self.state, connection, mode, hue, font, &text);

        // Townsperson services triggered by keyword: saying "bank" near a banker
        // opens your bank box. The words were still spoken above, so it reads as a
        // request the banker answers, not a hidden command.
        if let Some(&actor) = self.state.players.get(&connection) {
            npc::banker_keywords(&mut self.state, connection, actor, &text);
            // "sell" near a shopkeeper opens the offer list; "buy" opens the shop —
            // the keyword path to the same gump a double-click reaches. Checked
            // "sell" first so the "buy" substring inside neither steals it.
            let lowered = text.to_ascii_lowercase();
            if lowered.contains("sell") {
                npc::offer_sell_list(&mut self.state, connection, actor);
            } else if lowered.contains("buy") {
                npc::buy_keyword(&mut self.state, connection, actor);
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
    pub(super) fn single_click(&mut self, connection: ConnectionId, serial: u32) {
        let Some(target) = Serial::new(serial).and_then(|s| self.state.registry.entity_of(s))
        else {
            return;
        };

        // A mobile carries a `Name` and a `Body`; an item a `Graphic` and no
        // `Name`. The two cases pick a different graphic, hue, and name source.
        let (graphic, hue, text) = if let Some(name) = self.state.registry.get::<Name>(target) {
            let name = name.0.clone();
            let Some(body) = self.state.registry.get::<Body>(target).map(|b| b.id) else {
                return;
            };
            let hue = self
                .state
                .registry
                .get::<Notoriety>(target)
                .copied()
                .unwrap_or(Notoriety::Innocent)
                .name_hue();
            (body, hue, name)
        } else {
            let Some(&Graphic { id, .. }) = self.state.registry.get::<Graphic>(target) else {
                return;
            };
            let facet = self.state.facet_of(target);
            let Some(name) = self
                .state
                .facet_state(facet)
                .terrain
                .as_deref()
                .and_then(|terrain| terrain.item_name(id))
            else {
                return;
            };
            // Resolve the tiledata name's `%s%` pluralisation markers, then read
            // "3 bolts of cloth" for a stack, "a bolt of cloth" for a single —
            // Sphere's `GetNameFull`. The amount count decides both the markers and
            // the prefix.
            let amount = self.state.registry.get::<Amount>(target).map_or(1, |a| a.0);
            let resolved = crate::tiledata::pluralize_name(name, amount > 1);
            let text = if amount > 1 {
                format!("{amount} {resolved}")
            } else {
                resolved
            };
            (id, TEXT_HUE, text)
        };

        // The object's own serial makes the client draw the text over it; an empty
        // speaker name and the label mode make it a name tag, not speech.
        let packet = encode_message(serial, graphic, LABEL_MODE, hue, 3, "", &text);
        self.state.send(connection, packet);
    }

    /// Answer an AoS tooltip request (`0xD6`): send each named object's property
    /// list back to the asker. The client batches several serials as it hovers; a
    /// serial it cannot see or that names nothing is simply skipped. Off entirely
    /// when the shard serves no tooltips.
    pub(super) fn query_properties(&mut self, connection: ConnectionId, serials: &[u32]) {
        if self.state.gameplay.tooltip_mode == TooltipMode::Off {
            return;
        }
        for &serial in serials {
            if let Some(entity) = Serial::new(serial).and_then(|s| self.state.registry.entity_of(s))
            {
                self.state.send_property_list(connection, entity);
            }
        }
    }
}
