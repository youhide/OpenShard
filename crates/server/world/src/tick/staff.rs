use openshard_protocol::containers::GridSlot;
use openshard_protocol::gump::GumpPoint;
use openshard_state::{
    Drawn,
    kind_from_drawn,
};

use super::*;

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
            openshard_state::TargetPurpose::AdminCreature { kind } => {
                crate::admin::place_creature(&mut self.state, actor, kind, response.location);
            }
            openshard_state::TargetPurpose::Teleport => {
                crate::gm::teleport_to(&mut self.state, actor, response.location);
            }
            openshard_state::TargetPurpose::Sight => {
                crate::gm::report_sight(&mut self.state, actor, response.location);
            }
            openshard_state::TargetPurpose::PlaceHouse { deed } => {
                self.place_house_from_deed(actor, deed, response.location);
            }
            openshard_state::TargetPurpose::HouseList { change } => {
                self.change_house_list_for(actor, change, response.object);
            }
            openshard_state::TargetPurpose::HouseStorage { change, house } => {
                self.change_house_storage(actor, house, change, response.object);
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
                let accepted = skills::resolve_harvest_target(
                    &self.state,
                    facet,
                    response.location,
                    response.graphic.unwrap_or(Graphic(0)),
                )
                .is_some_and(|target| skills::begin_harvest(&mut self.state, actor, tool, target));
                if !accepted {
                    self.state.refuse_harvest(actor);
                }
            }
            openshard_state::TargetPurpose::Carve { tool } => {
                items::carve(&mut self.state, actor, tool, response.object);
            }
            openshard_state::TargetPurpose::GuildInvite => {
                // Re-checked here: the cursor outlives the click that raised it,
                // and a leader who disbanded or was deposed while it was up
                // invites nobody.
                let target = response
                    .object
                    .and_then(|serial| self.state.registry.entity_of(serial));
                match target {
                    Some(target) => {
                        match openshard_guilds::invite(&mut self.state, actor, target) {
                            Ok(()) => {
                                self.notify_self(actor, "They have been asked.");
                                self.state
                                    .system_message(target, "You have been asked to join a guild.");
                            }
                            Err(refusal) => self.notify_self(actor, refusal.message()),
                        }
                    }
                    None => self.notify_self(actor, "That cannot join a guild."),
                }
            }
            openshard_state::TargetPurpose::PartyInvite => {
                // Re-checked when the click lands, for `GuildInvite`'s reason:
                // the party can fill, or be disbanded, while the cursor is up.
                let target = response
                    .object
                    .and_then(|serial| self.state.registry.entity_of(serial));
                match target {
                    Some(target) => {
                        match openshard_party::invite(&mut self.state, actor, target) {
                            Ok(()) => {
                                self.notify_self(actor, "They have been asked along.");
                                self.state
                                    .system_message(target, "You have been invited to join a party.");
                            }
                            Err(refusal) => self.notify_self(actor, refusal.message()),
                        }
                    }
                    None => self.notify_self(actor, "That cannot join a party."),
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
        // And the guild window, whose rows are only ever what this side
        // remembers drawing — see `openshard_guilds::gump`.
        if openshard_guilds::handle(&mut self.state, connection, &response) {
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
        // And a healer's "wouldst thou like to be resurrected?" confirm.
        if self.handle_healer_gump(connection, &response) {
            return;
        }
        // And a house sign, whose rows are likewise only what this side
        // remembers drawing — see `openshard_housing::sign`.
        if openshard_housing::sign::handle(&mut self.state, connection, &response) {
            return;
        }
        // A reply to a gump that is *not* the engine's own belongs to the pack
        // that opened it (a notice board, a shard's custom menu). Forward it as a
        // `GumpAnswered` rather than dropping it, then stop — only the admin gump
        // runs the staff path below.
        if response
            .gump_id
            .validate(&[
                crate::admin::ADMIN_GUMP,
                crate::admin::ADMIN_ITEM_GUMP,
                crate::admin::ADMIN_CREATURE_GUMP,
            ])
            .is_none()
        {
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
        let Some((actor, action)) = crate::admin::button_action(&self.state, connection, &response) else {
            return;
        };
        match action {
            crate::admin::ButtonAction::Verb(verb) => {
                // The world publishes the verb and does not know who answers it; a
                // listener turns it into commands that lay a facet's content or clear it.
                if let Some(serial) = self.state.registry.serial_of(actor) {
                    self.state.bus.send(AdminMenuAction {
                        serial: Some(serial),
                        action: verb.to_owned(),
                    });
                }
                gm::notify(&mut self.state, actor, &format!("Admin: {verb}."));
                // A reply button takes the gump off the screen at the client's end —
                // that is what `GumpButton::Reply` *is* on the wire, not a client
                // misbehaving — so a menu meant to survive its own button has to be
                // drawn again from this side.
                crate::admin::open_menu(&mut self.state, actor);
            }
            crate::admin::ButtonAction::OpenItemCreator => {
                crate::admin::open_item_creator(&mut self.state, actor);
            }
            crate::admin::ButtonAction::CreateItem | crate::admin::ButtonAction::CreateItemKind => {
                match crate::admin::item_request(&response) {
                    Ok(item) => {
                        let Some(serial) = self.state.registry.serial_of(actor) else {
                            return;
                        };
                        let (created, description) = match item {
                            crate::admin::ItemRequest::Kind {
                                kind,
                                material,
                                amount,
                                stackable,
                            } => {
                                (
                                    items::give_kind_to_backpack(
                                        &mut self.state,
                                        serial,
                                        kind,
                                        material,
                                        amount,
                                        stackable,
                                    ),
                                    format!("kind {}", kind.0),
                                )
                            }
                            crate::admin::ItemRequest::LegacyArt {
                                graphic,
                                hue,
                                amount,
                                stackable,
                            } => {
                                let created = if let Some((kind, material)) =
                                    kind_from_drawn(Drawn { id: graphic, hue })
                                {
                                    // A known projection is created from its durable
                                    // identity even when it came through the legacy
                                    // debug form.
                                    items::give_kind_to_backpack(
                                        &mut self.state,
                                        serial,
                                        kind,
                                        material,
                                        amount,
                                        stackable,
                                    )
                                } else {
                                    items::give_to_backpack(
                                        &mut self.state,
                                        serial,
                                        graphic,
                                        hue,
                                        amount,
                                        stackable,
                                    )
                                };
                                (created, format!("{:#06x}", graphic.0))
                            }
                        };
                        if created {
                            gm::notify(
                                &mut self.state,
                                actor,
                                &format!("Created {description} in your backpack."),
                            );
                        } else {
                            gm::notify(&mut self.state, actor, "Your backpack cannot hold that item.");
                        }
                    }
                    Err(message) => gm::notify(&mut self.state, actor, message),
                }
            }
            crate::admin::ButtonAction::PlaceCreature => {
                match crate::admin::creature_kind(&response) {
                    Ok(kind) => crate::admin::begin_creature_placement(&mut self.state, actor, kind),
                    Err(message) => gm::notify(&mut self.state, actor, message),
                }
            }
        }
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
                let contained = openshard_state::components::Contained {
                    container: backpack,
                    position:  GumpPoint::new(60, 60),
                    grid:      GridSlot(0),
                };
                relocate_item(
                    &mut self.state,
                    theft.item,
                    LiveItemLocation::contained(contained),
                )
                .expect("a stolen item moved to the thief's pack has one owner");
            }
        }
        // Caught or not, reaching into somebody's pack is a crime — ServUO flags a
        // thief the moment the attempt is made, not only when it fails.
        combat::flag_criminal(&mut self.state, theft.thief);
        let _ = theft.victim;
    }
}
