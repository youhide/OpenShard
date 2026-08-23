//! Gates: the pair a spell opens, the eight that always stand, and the two ways
//! to step through either.
//!
//! The trigger lives here rather than in `items` for the reason `tick/traps.rs`
//! does: walking into a gate calls `magic`, and `items` may not depend on
//! `magic` without closing the `skills → items → magic → skills` loop. So the
//! click and the step are dispatched from the tick, and what they *do* is the
//! travel crate's.
//!
//! **Walking in is found, not announced.** There are two movement paths — the
//! player walk and the server-authoritative step — and a call beside each is one
//! to forget. Both already emit `MobileMoved`, so this reads the tick's own
//! events, the shape `guard_crossings` uses. Unlike a once-a-tick scan of every
//! position it cannot miss somebody who steps on and off a gate inside one
//! batch of commands, which is an ordinary thing for the inbox to hold.

use openshard_protocol::gump::{
    ButtonId, CloseGump, GUMP_WHITE, GumpAnswer, GumpButton, GumpDisplay, GumpId, GumpKey, GumpLayout,
    GumpPoint, GumpResponse, SwitchId,
};
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::wire::{Hue, SoundId};
use openshard_state::components::{MOONGATE_GRAPHIC, MOONGATE_REACH, Moongate, Position};

use super::*;

/// The gump id the destination list is drawn under.
///
/// Its own number, distinct from the quest log's `0x0051_*` and the craft
/// window's `0x0052_0001`, so a `0xB1` cannot be confused for another window's.
/// The id is an opaque token the client echoes back and nothing on its side
/// depends on the value — unlike the *button* encoding, which has to be
/// ServUO's exactly.
pub(super) const MOONGATE_GUMP: GumpId = openshard_protocol::gump::id::MOONGATE;

/// The one button that means "go" — ServUO's `MoongateGump` OKAY. Zero is the
/// close box and means cancel, as it does everywhere.
const MOONGATE_OK: ButtonId = ButtonId(1);

/// How long a Gate Travel pair stands — ServUO's thirty seconds.
const GATE_LIFETIME_SECONDS: u64 = 30;

/// ServUO's `GateTravel` sound, played at both ends as the pair opens.
const GATE_OPEN_SOUND: SoundId = SoundId(0x020E);
/// And the one a traveller makes arriving through one.
const GATE_USE_SOUND: SoundId = SoundId(0x01FE);

impl World {
    /// Open a gate at the caster and another at `destination`, each leading to
    /// the other.
    pub(super) fn open_gate_pair(&mut self, caster: EntityId, target_serial: Option<Serial>) {
        let Some(rune) = target_serial.and_then(|serial| self.state.registry.entity_of(serial)) else {
            return;
        };
        if !openshard_items::in_reach(&self.state, rune, caster) {
            self.notify_self(caster, "That is too far away.");
            return;
        }
        let Some((there_facet, there)) = magic::destination_of(&self.state, rune) else {
            self.notify_self(caster, "That rune is not yet marked.");
            return;
        };
        self.open_gate_to(caster, there_facet, there);
    }

    /// The half of a gate cast that needs no rune: open a pair to a destination
    /// already known.
    ///
    /// Split out so a runebook's Gate button reaches exactly the same rules — a
    /// second copy of the both-ends check is how a book quietly becomes a way
    /// past a region that bars gating.
    pub(super) fn open_gate_to(&mut self, caster: EntityId, there_facet: Facet, there: Point) {
        let Some((here_facet, here)) = magic::standing_at(&self.state, caster) else {
            return;
        };

        if there_facet != here_facet && !self.state.gameplay.cross_facet_travel {
            self.notify_self(caster, "You cannot gate to another facet.");
            return;
        }
        if !self.state.facets.contains_key(&there_facet) {
            self.notify_self(caster, magic::TravelKind::GateTo.refusal());
            return;
        }
        // Both ends, each against its own kind — the same pairing Recall makes,
        // and the reason `may_travel` takes one end at a time.
        if !magic::may_travel(&self.state, caster, magic::TravelKind::GateFrom, here_facet, here) {
            self.notify_self(caster, magic::TravelKind::GateFrom.refusal());
            return;
        }
        if !magic::may_travel(&self.state, caster, magic::TravelKind::GateTo, there_facet, there) {
            self.notify_self(caster, magic::TravelKind::GateTo.refusal());
            return;
        }
        // ServUO checks both ends for an existing gate, and so does Sphere for a
        // telepad. Two gates on one tile are two overlapping ways out of the same
        // spot, and closing one leaves the other looking broken.
        if self.gate_at(here_facet, here).is_some() || self.gate_at(there_facet, there).is_some() {
            self.notify_self(caster, "There is already a gate there.");
            return;
        }

        let expires_at = Some(self.state.ticks + GATE_LIFETIME_SECONDS * TICKS_PER_SECOND);
        // Each points at the other's tile. There is no link to keep honest —
        // the destination *is* the link.
        self.spawn_gate(
            here_facet,
            here,
            Moongate {
                facet: there_facet,
                destination: there,
                expires_at,
            },
        );
        self.spawn_gate(
            there_facet,
            there,
            Moongate {
                facet: here_facet,
                destination: here,
                expires_at,
            },
        );
        self.state.play_sound(caster, GATE_OPEN_SOUND);
        self.notify_self(caster, "You open a magical gate to another location.");
    }

    /// Put one gate on the ground — the drawn-item path, `spawn_field_tile`'s
    /// shape.
    ///
    /// Deliberately **not** `items::spawn_item`: that stamps a `Decays` (a second
    /// clock contradicting this one) and emits an `ItemSpawned` the pack would
    /// read as somebody dropping something. A gate owns its own lifetime, and a
    /// permanent one owns none at all.
    pub(super) fn spawn_gate(&mut self, facet: Facet, at: Point, gate: Moongate) -> Option<EntityId> {
        let Ok((entity, _serial)) = self.state.registry.spawn_with_serial(SerialKind::Item) else {
            warn!("out of item serials; not opening a gate");
            return None;
        };
        self.state.registry.insert(
            entity,
            Drawn {
                id: MOONGATE_GRAPHIC,
                hue: Hue(0),
            },
        );
        self.state.registry.insert(entity, Position(at));
        self.state.registry.insert(entity, facet);
        self.state.registry.insert(entity, gate);
        self.state
            .facet_state_mut(facet)
            .sectors
            .insert(entity, at, Occupant::Item);
        // No obstruction, ever: a gate is walked *into*. Blocking the tile is how
        // the walk-in trigger becomes dead code that reads as a movement bug.
        self.state.reveal(entity);
        Some(entity)
    }

    /// Place the city moongates that are missing, and say how many went down.
    ///
    /// Idempotent: a tile that already holds one is skipped, so running it twice
    /// places nine gates and not eighteen. Not run at boot — "nothing
    /// re-populates at boot" holds, and the decoration sweep saves and restores
    /// these like any other placed art.
    pub(super) fn place_public_moongates(&mut self) -> usize {
        let mut placed = 0;
        for gate in magic::PUBLIC_MOONGATES {
            if !self.state.facets.contains_key(&gate.facet) {
                continue;
            }
            if self.public_gate_entity(gate.facet, gate.at).is_some() {
                continue;
            }
            // The decoration path *without* the obstruction: `place_decoration`
            // seals a tile whose tiledata calls the graphic impassable, and a
            // gate has to be walked into. A blocked one is not a slightly worse
            // gate — it is a gate whose walk-in trigger is dead code, which reads
            // as a movement bug rather than a missing rule.
            let Ok((entity, _serial)) = self.state.registry.spawn_with_serial(SerialKind::Item) else {
                warn!("out of item serials; stopping moongate placement");
                break;
            };
            self.state.registry.insert(
                entity,
                Drawn {
                    id: MOONGATE_GRAPHIC,
                    hue: Hue(0),
                },
            );
            self.state.registry.insert(entity, Position(gate.at));
            self.state.registry.insert(entity, gate.facet);
            self.state.registry.insert(entity, Decoration);
            self.state
                .registry
                .insert(entity, Name(format!("a moongate to {}", gate.name)));
            self.state
                .facet_state_mut(gate.facet)
                .sectors
                .insert(entity, gate.at, Occupant::Item);
            self.state.reveal(entity);
            placed += 1;
        }
        placed
    }

    /// The spell-laid gate standing on a tile, if any.
    pub(super) fn gate_at(&self, facet: Facet, at: Point) -> Option<EntityId> {
        self.state
            .registry
            .query::<Moongate>()
            .find(|(entity, _)| {
                self.state.facet_of(*entity) == facet
                    && self
                        .state
                        .registry
                        .get::<Position>(*entity)
                        .is_some_and(|pos| pos.0.x == at.x && pos.0.y == at.y)
            })
            .map(|(entity, _)| entity)
    }

    /// The city moongate standing on a tile, as an entity — the drawn item, so a
    /// reach check has something to measure against.
    pub(super) fn public_gate_entity(&self, facet: Facet, at: Point) -> Option<EntityId> {
        magic::public_gate_at(facet, at)?;
        self.state
            .registry
            .query::<Drawn>()
            .find(|(entity, graphic)| {
                graphic.id == MOONGATE_GRAPHIC
                    && self.state.facet_of(*entity) == facet
                    && self
                        .state
                        .registry
                        .get::<Position>(*entity)
                        .is_some_and(|pos| pos.0.x == at.x && pos.0.y == at.y)
            })
            .map(|(entity, _)| entity)
    }

    /// Whether an entity is a gate of either kind — a spell's, which carries a
    /// [`Moongate`], or a city one, which carries nothing and is recognised by
    /// where it stands.
    ///
    /// The city gates hold no component on purpose: they are saved and restored
    /// as ordinary decoration and their meaning is re-derived every boot, which
    /// is what keeps them out of the schema entirely and what makes a restored
    /// one correct with no restore hook to forget.
    pub(super) fn is_gate(&self, entity: EntityId) -> bool {
        if self.state.registry.has::<Moongate>(entity) {
            return true;
        }
        self.state
            .registry
            .get::<Position>(entity)
            .is_some_and(|at| magic::public_gate_at(self.state.facet_of(entity), at.0).is_some())
    }

    /// Close the gates whose time is up — the tick counter, like a field and a
    /// decaying item, so a gate replays.
    pub(super) fn expire_gates(&mut self) {
        let now = self.state.ticks;
        let done: Vec<EntityId> = self
            .state
            .registry
            .query::<Moongate>()
            .filter(|(_, gate)| gate.expires_at.is_some_and(|at| now >= at))
            .map(|(entity, _)| entity)
            .collect();
        for entity in done {
            self.close_gate(entity);
        }
    }

    /// Take a gate off the world: forget it on every screen, off the sector grid,
    /// out of the registry. `remove_field`'s tail — miss it and what is left is
    /// an invisible gate that still works.
    fn close_gate(&mut self, entity: EntityId) {
        let Some(serial) = self.state.registry.serial_of(entity) else {
            return;
        };
        let facet = self.state.facet_of(entity);
        for watcher in self.state.watchers_of(entity) {
            self.state.forget(watcher, entity, serial);
        }
        self.state.facet_state_mut(facet).sectors.remove(entity);
        self.state.registry.despawn(entity);
    }

    /// Take everyone who stepped onto a gate this tick through it.
    pub(super) fn gate_crossings(&mut self) {
        let moved: Vec<(EntityId, Point)> = self
            .state
            .bus
            .read(&mut self.gated)
            .map(|event| (event.entity, event.to))
            .collect();
        for (entity, to) in moved {
            let facet = self.state.facet_of(entity);
            if let Some(gate) = self.gate_at(facet, to) {
                self.use_gate(entity, gate);
            } else if let Some(gate) = self.public_gate_entity(facet, to) {
                // A city gate offers a list rather than taking you somewhere:
                // it is one gate with nine destinations, not a pair.
                self.open_gate_gump(entity, gate);
            }
        }
    }

    /// A double-click on a gate within reach — ServUO's `Moongate.OnDoubleClick`.
    ///
    /// Returns whether the click was a gate's, so the tick can stop before the
    /// ordinary item dispatch sees it.
    pub(super) fn click_gate(&mut self, player: EntityId, target: EntityId) -> bool {
        if !self.is_gate(target) {
            return false;
        }
        let near = match (
            self.state.registry.get::<Position>(player),
            self.state.registry.get::<Position>(target),
        ) {
            (Some(&Position(here)), Some(&Position(there))) => {
                self.state.facet_of(player) == self.state.facet_of(target)
                    && openshard_state::sectors::in_range(here, there, MOONGATE_REACH)
            }
            _ => false,
        };
        if near {
            self.enter_gate(player, target);
        }
        true
    }

    /// Step into a gate of either kind: a spell's pair takes you to its other
    /// end, a city one offers the list.
    fn enter_gate(&mut self, traveller: EntityId, gate: EntityId) {
        if self.state.registry.has::<Moongate>(gate) {
            self.use_gate(traveller, gate);
            return;
        }
        self.open_gate_gump(traveller, gate);
    }

    /// Draw a city moongate's destination list — ServUO's `MoongateGump`.
    ///
    /// The first engine window to use a gump's *switches*: a radio per city, and
    /// one button that acts on whichever is set. Only a player with a client gets
    /// one; walking a creature onto a public gate does nothing, as it does in
    /// ServUO.
    fn open_gate_gump(&mut self, traveller: EntityId, gate: EntityId) {
        let Some(&Client { connection, .. }) = self.state.registry.get::<Client>(traveller) else {
            return;
        };
        let mut layout = GumpLayout::default();
        layout.no_close();
        let rows = magic::PUBLIC_MOONGATES.len() as i32;
        layout.background(0, 0, 380, 60 + 30 * rows, 5054);
        layout.label(45, 15, GUMP_WHITE, "Choose your destination:");
        for (index, destination) in magic::PUBLIC_MOONGATES.iter().enumerate() {
            let y = 45 + 30 * index as i32;
            // The gate you are standing on is offered and simply refused on
            // selection (ServUO's 1019003), rather than left out — a list whose
            // rows move depending on where you are is a list nobody learns.
            // The switch ids are the rows, numbered from zero — which is what
            // makes the list's length the whole domain a reply is checked against.
            layout.radio(25, y, 0x00D2, 0x00D3, false, SwitchId(index as u32));
            layout.label(60, y, GUMP_WHITE, destination.name);
        }
        let ok_y = 50 + 30 * rows;
        layout.button(25, ok_y, 0x00FA, 0x00FB, GumpButton::Reply, 0, MOONGATE_OK);
        layout.label(60, ok_y, GUMP_WHITE, "Travel");

        let (text, lines) = layout.finish();
        // Close then draw, the `CraftGump` order: a client told to draw twice
        // draws two windows.
        self.state.send_packet(
            connection,
            &ServerPacket::CloseGump(CloseGump {
                gump_id: MOONGATE_GUMP,
                button: ButtonId::CLOSE_BOX,
            }),
        );
        self.state.send_packet(
            connection,
            &ServerPacket::GumpDisplay(GumpDisplay {
                serial: self
                    .state
                    .registry
                    .serial_of(traveller)
                    .map_or(GumpKey::STANDALONE, GumpKey::on),
                gump_id: MOONGATE_GUMP,
                at: GumpPoint::new(50, 50),
                layout: text.to_owned(),
                lines: lines.to_vec(),
            }),
        );
        // Remembered so the reply can re-check the player is still beside the
        // gate it answered for, and so a `0xB1` this side never drew does nothing.
        if let Some(row) = self.state.row_of_mut(traveller) {
            row.gate_gump = Some(gate);
        }
    }

    /// Answer a destination list. Returns whether the reply was one of ours.
    pub(super) fn handle_gate_gump(&mut self, connection: ConnectionId, response: &GumpResponse) -> bool {
        if response.gump_id.validate(&[MOONGATE_GUMP]).is_none() {
            return false;
        }
        let Some(&player) = self.state.players.get(&connection) else {
            return true;
        };
        // Taken, not borrowed: a reply to a window this side never drew finds
        // nothing and does nothing.
        let Some(gate) = self.state.row_of_mut(player).and_then(|row| row.gate_gump.take()) else {
            return true;
        };
        if response.button.interpret() != GumpAnswer::Pressed(MOONGATE_OK) {
            return true; // the close box
        }
        // Nothing selected, or a row no list ever offered — refused, not
        // clamped: taking somebody to whatever sits in slot zero is worse than
        // taking them nowhere.
        let Some(choice) = response
            .switches
            .first()
            .and_then(|switch| switch.validate(magic::PUBLIC_MOONGATES.len()).ok())
        else {
            return true;
        };
        // `validate` bounded it against this very list, so the index holds.
        let destination = &magic::PUBLIC_MOONGATES[choice.0 as usize];

        // Still beside the gate it was opened for: the window can sit on screen
        // while its owner walks away, and a reach checked only when it opened is
        // a reach not checked at all.
        let near = match (
            self.state.registry.get::<Position>(player),
            self.state.registry.get::<Position>(gate),
        ) {
            (Some(&Position(here)), Some(&Position(there))) => {
                self.state.facet_of(player) == self.state.facet_of(gate)
                    && openshard_state::sectors::in_range(here, there, MOONGATE_REACH)
            }
            _ => false,
        };
        if !near {
            self.notify_self(player, "You must be near the gate to use it.");
            return true;
        }
        if self
            .state
            .registry
            .get::<Position>(gate)
            .is_some_and(|at| at.0.x == destination.at.x && at.0.y == destination.at.y)
        {
            self.notify_self(player, "You are already there.");
            return true;
        }
        self.travel_through(player, destination.facet, destination.at);
        true
    }

    /// Step through. One door for both triggers, so a gate cannot be safe to walk
    /// into and unsafe to click.
    pub(super) fn use_gate(&mut self, traveller: EntityId, gate: EntityId) {
        let Some(&gate) = self.state.registry.get::<Moongate>(gate) else {
            return;
        };
        // Mid-cast or holding something, you are too busy — ServUO's 1049616 and
        // 1071955. Both are about arriving somewhere with state that belonged to
        // where you left.
        if self
            .state
            .registry
            .has::<openshard_state::components::Casting>(traveller)
        {
            self.notify_self(traveller, "You are too busy to do that.");
            return;
        }
        if self.state.row_of(traveller).is_some_and(|row| row.held.is_some()) {
            self.notify_self(traveller, "You cannot teleport while dragging an object.");
            return;
        }
        self.travel_through(traveller, gate.facet, gate.destination);
    }

    /// The arrival both kinds of gate share.
    fn travel_through(&mut self, traveller: EntityId, facet: Facet, destination: Point) {
        if !self.state.facets.contains_key(&facet) {
            return;
        }
        self.state.move_to(traveller, facet, destination);
        self.state.play_sound(traveller, GATE_USE_SOUND);
    }
}
