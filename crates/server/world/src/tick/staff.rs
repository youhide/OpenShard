use super::*;
use openshard_protocol::containers::GridSlot;
use openshard_protocol::gump::GumpPoint;

impl World {
    /// Act on a targeting cursor's answer. Looks up what the cursor was raised for
    /// and, if the click was not cancelled, does it. A cancel just clears the
    /// pending target.
    pub(super) fn handle_target(
        &mut self,
        connection: ConnectionId,
        response: openshard_protocol::target::TargetResponse,
    ) {
        let Some(&actor) = self.state.players.get(&connection) else {
            return;
        };
        let Some(purpose) = self.state.take_target(actor) else {
            return; // no cursor was up for this mobile
        };
        if response.cancelled {
            return;
        }
        match purpose {
            openshard_state::TargetPurpose::Teleport => {
                crate::gm::teleport_to(&mut self.state, actor, response.location);
            }
            openshard_state::TargetPurpose::Skill { skill } => {
                let outcome = skills::on_target(&mut self.state, actor, skill, response.object);
                if let Some(theft) = outcome.stolen {
                    self.carry_out_theft(theft);
                }
                if let Some(tamed) = outcome.tamed {
                    npc::tame(
                        &mut self.state,
                        tamed.creature,
                        tamed.tamer,
                        tamed.slots,
                        tamed.angered,
                    );
                }
            }
            openshard_state::TargetPurpose::SkillSecond { skill, first } => {
                // The item-started skills (a bandage, a lockpick) answer with what
                // to spend; the rest resolve on their own.
                let (bandage, pick) =
                    skills::on_item_target(&mut self.state, actor, skill, first, response.object);
                if let Some(started) = bandage {
                    if let Some(serial) = self.state.registry.serial_of(started.bandage) {
                        items::consume(&mut self.state, serial, 1);
                    }
                }
                if let Some(broke) = pick {
                    if let Some(serial) = self.state.registry.serial_of(broke.pick) {
                        items::consume(&mut self.state, serial, 1);
                    }
                }
                skills::on_second_target(&mut self.state, actor, skill, first, response.object);
            }
            openshard_state::TargetPurpose::SetTrap { kind, power } => {
                let target = response
                    .object
                    .and_then(|serial| self.state.registry.entity_of(serial));
                match target {
                    Some(target)
                        if self
                            .state
                            .registry
                            .has::<openshard_state::components::Container>(target) =>
                    {
                        self.state.registry.insert(
                            target,
                            openshard_state::components::Trap {
                                kind,
                                power,
                                level: 0,
                            },
                        );
                        self.notify_self(actor, "It is trapped now.");
                    }
                    _ => self.notify_self(actor, "Only a container can be trapped."),
                }
            }
            openshard_state::TargetPurpose::Harvest { tool } => {
                // The one purpose that wants the *ground* rather than an object,
                // so it reads the reply's point and tile graphic and ignores the
                // serial. What the tile actually is, the map decides — see
                // `skills::resolve_harvest_target`, which is where a client naming
                // a static that is not there is refused.
                let facet = self.state.facet_of(actor);
                if let Some(target) = skills::resolve_harvest_target(
                    &self.state,
                    facet.0,
                    response.location,
                    response.graphic.map_or(0, |graphic| graphic.0),
                ) {
                    skills::begin_harvest(&mut self.state, actor, tool, target);
                }
            }
            openshard_state::TargetPurpose::TurnKey { key } => {
                // The key may have gone while the cursor was up, and the target may be
                // nothing at all — a key turned on the sky opens nothing and says so.
                let target = response
                    .object
                    .and_then(|serial| self.state.registry.entity_of(serial));
                match target {
                    Some(target) if items::in_key_reach(&self.state, actor, target) => {
                        items::turn_key(&mut self.state, actor, key, target);
                    }
                    _ => self.notify_self(actor, "That does not have a lock."),
                }
            }
            openshard_state::TargetPurpose::Spell { spell, success } => {
                // The cast already paid and rolled; now it has its aim. Announce
                // it (so the pack can react) and run the core effect if it took.
                if let Some(serial) = self.state.registry.serial_of(actor) {
                    self.state.bus.send(magic::SpellCast {
                        caster: actor,
                        serial,
                        spell,
                        target: response.object,
                        success,
                    });
                }
                if success {
                    self.apply_spell_effect(actor, spell, response.object, response.location);
                }
            }
        }
    }

    /// Route a `0xB1` — a gump button — to whoever opened that gump.
    ///
    /// There is one reply packet for every dialog on the shard, so the `gump_id`
    /// the client echoes back is the only thing saying which. Three destinations:
    /// the engine's own admin menu, the quest dialogs, and everything else, which
    /// belongs to the pack that opened it and leaves as a `GumpAnswered`.
    pub(super) fn handle_gump_response(&mut self, connection: ConnectionId, response: GumpResponse) {
        // The quest dialogs answer themselves — the core owns that window, the
        // way it owns the container and vendor ones.
        if openshard_quests::handle(&mut self.state, connection, &response) {
            return;
        }
        // And so does the craft window, for the same reason and by the same
        // rule: the reply is matched against the context the server remembers
        // drawing, so one for a window this side never opened does nothing.
        if crafting::handle(&mut self.state, connection, &response) {
            return;
        }
        // And the moongate destination list, likewise matched against what the
        // server remembers drawing.
        if self.handle_gate_gump(connection, &response) {
            return;
        }
        // And the runebook.
        if self.handle_runebook_gump(connection, &response) {
            return;
        }
        // A reply to a gump that is *not* the engine's own belongs to the pack
        // that opened it (a notice board, a shard's custom menu). Forward it as a
        // `GumpAnswered` rather than dropping it, then stop — only the admin gump
        // runs the staff path below.
        if response.gump_id.validate(&[crate::admin::ADMIN_GUMP]).is_none() {
            if let Some(&actor) = self.state.players.get(&connection) {
                if let Some(serial) = self.state.registry.serial_of(actor) {
                    self.state.bus.send(crate::events::GumpAnswered {
                        serial,
                        gump_id: response.gump_id,
                        button: response.button,
                        switches: response.switches,
                        text_entries: response.text_entries,
                    });
                }
            }
            return;
        }
        let Some((actor, verb)) = crate::admin::button_action(&self.state, connection, &response) else {
            return;
        };
        // The engine holds no spawn data: it emits the verb, and the script pack —
        // where a shard's spawns are edited without a rebuild — decides what it
        // means, registering regions through `op_register_spawner` or clearing them.
        if let Some(serial) = self.state.registry.serial_of(actor) {
            self.state.bus.send(AdminMenuAction {
                serial: Some(serial),
                action: verb.to_owned(),
            });
        }
        gm::notify(&mut self.state, actor, &format!("Admin: {verb}."));
    }
}

impl World {
    /// Finish a theft `skills` decided on: move the item, or make a criminal.
    ///
    /// Two doors this crate is above and `skills` is below — `items` moves a thing
    /// into a pack, `combat` turns somebody grey — so the decision arrives as a
    /// value and the tick spends it, the `ai::think_one` split.
    fn carry_out_theft(&mut self, theft: skills::Stolen) {
        let Some(thief) = self.state.registry.serial_of(theft.thief) else {
            return;
        };
        if theft.took {
            if let Some(backpack) = items::backpack_of(&self.state, thief) {
                self.state.registry.insert(
                    theft.item,
                    openshard_state::components::Contained {
                        container: backpack,
                        position: GumpPoint::new(60, 60),
                        grid: GridSlot(0),
                    },
                );
            }
        }
        // Caught or not, reaching into somebody's pack is a crime — ServUO flags a
        // thief the moment the attempt is made, not only when it fails.
        combat::flag_criminal(&mut self.state, theft.thief);
        let _ = theft.victim;
    }
}
