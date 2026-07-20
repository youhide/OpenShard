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

    /// Answer a single-click (`0x09`): draw the clicked mobile's name over its
    /// head, seen only by the asker, in its notoriety colour.
    ///
    /// Mobiles with a name only — a townsperson, a player. A nameless creature and
    /// a plain item say nothing rather than a blank label; item names wait on a
    /// tiledata name lookup.
    pub(super) fn single_click(&mut self, connection: ConnectionId, serial: u32) {
        let Some(target) = Serial::new(serial).and_then(|s| self.state.registry.entity_of(s))
        else {
            return;
        };
        let Some(name) = self.state.registry.get::<Name>(target) else {
            return;
        };
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
        // The object's own serial makes the client draw the text over it; an empty
        // speaker name and the label mode make it a name tag, not speech.
        let packet = encode_message(serial, body, LABEL_MODE, hue, 3, "", &name);
        self.state.send(connection, packet);
    }
}
