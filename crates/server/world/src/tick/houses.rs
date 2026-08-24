//! Houses across the save: swept out, and rebuilt on the way back in.
//!
//! # What is saved is where it stands, not what it is made of
//!
//! A multi's shape is a pure function of its id and it lives in the client's own
//! files. Saving the components would be saving a copy of a file every client
//! already has — one that goes stale the day the operator updates their install,
//! and then the shard's walls and the client's picture disagree with no way to
//! tell which is right. So the record is the id and the position, and the
//! footprint is recomputed at boot from the same table placement read it from.
//!
//! The consequence to accept: a shard booted **without** client files restores
//! the house entities and gives them no walls. That is the same bargain every
//! other `Terrain` method makes, and it is better than the alternative, which is
//! a house whose walls came from a file the client no longer has.

use openshard_entities::EntityId;
use openshard_gateway::ConnectionId;
use openshard_items as items;
use openshard_persistence::record::HouseRecord;
use openshard_protocol::serial::{RawSerial, Serial};
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::target::{MultiTargetRequest, TargetKind};
use openshard_protocol::wire::{CursorId, Graphic, Hue};
use openshard_protocol::world::{Facet, Point};
use openshard_state::TargetPurpose;
use openshard_state::components::{
    Client, Contained, Drawn, House, HouseDeed, HouseDesign, HouseSign, Position,
};
use tracing::{info, warn};

use super::World;

impl World {
    /// Every house as a saveable record.
    pub(super) fn house_records(&self) -> Vec<HouseRecord> {
        self.state
            .registry
            .query::<House>()
            .filter_map(|(entity, house)| {
                let serial = self.state.registry.serial_of(entity)?;
                let &Position(at) = self.state.registry.get::<Position>(entity)?;
                Some(HouseRecord {
                    serial,
                    multi: house.multi.0,
                    x: at.x,
                    y: at.y,
                    z: at.z,
                    facet: self.state.facet_of(entity).0,
                    owner: house.owner,
                    co_owners: house.co_owners.iter().map(|serial| serial.raw()).collect(),
                    friends: house.friends.iter().map(|serial| serial.raw()).collect(),
                    bans: house.bans.iter().map(|serial| serial.raw()).collect(),
                    age: house.age,
                    lockdowns: house.lockdowns,
                })
            })
            .collect()
    }

    /// Answer a client's `0xBF 0x1E` with the house's design.
    ///
    /// The last of the three-packet conversation, and the only expensive one:
    /// the revision rode out with the draw, the client found it did not hold
    /// that revision, and this is what it costs when it does not. A client that
    /// already has the picture never gets here.
    ///
    /// Silent on every refusal, including "that is not a house". A client may
    /// legitimately ask about a multi that has since come down, and there is
    /// nothing to tell it that it will not learn from the removal anyway.
    pub(super) fn design_details_request(&mut self, connection: ConnectionId, serial: RawSerial) {
        let Some(watcher) = self.state.players.get(&connection).copied() else {
            return;
        };
        let Some(house) = serial.validate().and_then(|s| self.state.registry.entity_of(s)) else {
            return;
        };
        // Asked about a house they cannot see. Refused rather than answered:
        // a design is a few hundred rows, and "send me every house on the shard"
        // should not be one packet away.
        if self.state.facet_of(house) != self.state.facet_of(watcher) {
            return;
        }
        self.state.send_design_detail(watcher, house);
    }

    /// Every ship as a saveable record.
    ///
    /// No component list beside it, unlike the houses: a boat's shape is a pure
    /// function of its multi id, so the hull-and-deck split is recomputed at
    /// boot from the same multi table the mooring read.
    pub(super) fn boat_records(&self) -> Vec<openshard_persistence::record::BoatRecord> {
        self.state
            .registry
            .query::<openshard_state::components::Boat>()
            .filter_map(|(entity, boat)| {
                let serial = self.state.registry.serial_of(entity)?;
                let &Position(at) = self.state.registry.get::<Position>(entity)?;
                Some(openshard_persistence::record::BoatRecord {
                    serial,
                    multi: boat.multi.0,
                    x: at.x,
                    y: at.y,
                    z: at.z,
                    facet: self.state.facet_of(entity).0,
                    owner: boat.owner,
                })
            })
            .collect()
    }

    /// Put the ships back at boot.
    ///
    /// [`restore_houses`](Self::restore_houses)' reasoning, and not through
    /// `openshard_boats::place`: that decides whether a ship *may* float
    /// somewhere, and a ship that was afloat when it was launched stays afloat.
    /// A shard that later corrected a map's water flags would otherwise sink a
    /// fleet at the next restart.
    pub fn restore_boats(&mut self, records: Vec<openshard_persistence::record::BoatRecord>) {
        let mut restored = 0;
        let mut shapeless = 0;
        for record in records {
            let facet = Facet(record.facet);
            let at = Point::new(record.x, record.y, record.z);
            let entity = self.state.registry.spawn();
            if self.state.registry.bind_serial(entity, record.serial).is_err() {
                warn!(serial = %record.serial, "a saved ship's serial was already taken");
                self.state.registry.despawn(entity);
                continue;
            }
            self.state.registry.insert(
                entity,
                Drawn {
                    id: openshard_protocol::wire::MultiId(record.multi).graphic(),
                    hue: Hue(0),
                },
            );
            self.state.registry.insert(entity, Position(at));
            self.state.registry.insert(
                entity,
                openshard_state::components::Boat {
                    multi: openshard_protocol::wire::MultiId(record.multi),
                    owner: record.owner,
                },
            );
            self.state.registry.insert(entity, facet);
            self.state.place_item(facet, entity, at);
            // The hull-and-deck split, recomputed. A shard with no client files
            // gets a ship that draws on every client and carries nobody — the
            // same bargain a house's walls make, and for the same reason.
            match openshard_boats::planks_of(
                &self.state,
                entity,
                at,
                openshard_protocol::wire::MultiId(record.multi),
            ) {
                Ok(berth) => self.state.facet_state_mut(facet).moor(entity, berth),
                Err(_) => shapeless += 1,
            }
            restored += 1;
        }
        if restored > 0 {
            info!(
                boats = restored,
                without_shape = shapeless,
                "ships back on the water"
            );
        }
    }

    /// Every designed house's components, flattened into one list.
    ///
    /// One row per component of every house carrying a
    /// [`HouseDesign`]. A shard where every house is a classic multi answers an
    /// empty vector, which is every shard until somebody designs one.
    pub(super) fn house_design_records(&self) -> Vec<openshard_persistence::record::HouseDesignRecord> {
        self.state
            .registry
            .query::<HouseDesign>()
            .filter_map(|(entity, design)| {
                let house = self.state.registry.serial_of(entity)?;
                Some(design.components.iter().map(move |component| {
                    openshard_persistence::record::HouseDesignRecord {
                        house,
                        revision: design.revision,
                        graphic: component.graphic.0,
                        dx: component.dx,
                        dy: component.dy,
                        dz: component.dz,
                        flags: component.flags,
                    }
                }))
            })
            .flatten()
            .collect()
    }

    /// Put the houses back at boot, walls and all.
    ///
    /// Call once, before anyone connects. Not through `openshard_housing::place`:
    /// that decides whether a house *may* go somewhere, and a house that was
    /// legal when it was built stays built even if the rules have since tightened
    /// — otherwise a shard that changed its yard size would silently demolish
    /// half of Britannia at the next restart.
    pub fn restore_houses(
        &mut self,
        records: Vec<HouseRecord>,
        designs: Vec<openshard_persistence::record::HouseDesignRecord>,
    ) {
        // Grouped by house once, rather than scanned per house: a shard with a
        // hundred designed houses would otherwise walk the whole list a hundred
        // times.
        let mut by_house: std::collections::HashMap<Serial, (u32, Vec<openshard_uofiles::multi::Component>)> =
            std::collections::HashMap::new();
        for row in designs {
            let entry = by_house.entry(row.house).or_insert((row.revision, Vec::new()));
            entry.0 = row.revision;
            entry.1.push(openshard_uofiles::multi::Component {
                graphic: Graphic(row.graphic),
                dx: row.dx,
                dy: row.dy,
                dz: row.dz,
                flags: row.flags,
            });
        }
        let mut restored = 0;
        let mut wall_less = 0;
        for record in records {
            let facet = Facet(record.facet);
            let at = Point::new(record.x, record.y, record.z);
            let entity = self.state.registry.spawn();
            if self.state.registry.bind_serial(entity, record.serial).is_err() {
                warn!(serial = %record.serial, "a saved house's serial was already taken");
                self.state.registry.despawn(entity);
                continue;
            }
            self.state.registry.insert(
                entity,
                Drawn {
                    id: openshard_protocol::wire::MultiId(record.multi).graphic(),
                    hue: Hue(0),
                },
            );
            self.state.registry.insert(entity, Position(at));
            self.state.registry.insert(
                entity,
                House {
                    multi: openshard_protocol::wire::MultiId(record.multi),
                    owner: record.owner,
                    // A saved serial that will not parse is dropped rather than
                    // refused: a name this engine cannot read is a name it cannot
                    // act on either, and a house that will not restore is worse
                    // than one missing a friend.
                    co_owners: record.co_owners.iter().copied().filter_map(Serial::new).collect(),
                    friends: record.friends.iter().copied().filter_map(Serial::new).collect(),
                    bans: record.bans.iter().copied().filter_map(Serial::new).collect(),
                    age: record.age,
                    lockdowns: record.lockdowns,
                },
            );
            self.state.registry.insert(entity, facet);
            self.state.place_item(facet, entity, at);
            // The design, if this house has one. Put on before the footprint is
            // computed, because the footprint is computed *from* it.
            let design = by_house.remove(&record.serial);
            if let Some((revision, components)) = design.clone() {
                self.state
                    .registry
                    .insert(entity, HouseDesign { components, revision });
            }
            let shape = design.as_ref().map(|(_, components)| components.as_slice());

            match openshard_housing::footprint_of(
                &self.state,
                at,
                openshard_protocol::wire::MultiId(record.multi),
                shape,
            ) {
                Ok(footprint) => {
                    openshard_housing::block(&mut self.state, entity, facet, &footprint);
                }
                // No client files, or an id this install does not know. The house
                // is still there and still owned; it simply stops nobody, which
                // is said out loud rather than left to be found by walking
                // through a wall.
                Err(_) => wall_less += 1,
            }
            // Rebuilt rather than restored, for the module header's reason: the
            // sign's spot is a pure function of the multi's box, and a saved copy
            // of it would go stale the day the operator updates their install.
            // A shard with no client files gets no sign, the same bargain the
            // walls make.
            openshard_housing::hang_sign(
                &mut self.state,
                entity,
                facet,
                at,
                openshard_protocol::wire::MultiId(record.multi),
            );
            restored += 1;
        }
        if restored > 0 {
            info!(houses = restored, wall_less, "restored the shard's houses");
        }
    }
}

impl World {
    /// A deed's cursor came back: put the house where they clicked, and spend the
    /// deed.
    ///
    /// The **deed is re-read here**, not trusted from when the cursor went up.
    /// `TargetPurpose::PlaceHouse` carries the deed rather than the multi id for
    /// exactly this: a deed sold, dropped or destroyed while the cursor was up
    /// must not still place a house, and a player with one deed and a fast hand
    /// must not place two.
    pub(super) fn place_house_from_deed(
        &mut self,
        actor: EntityId,
        deed: EntityId,
        at: openshard_protocol::world::Point,
    ) {
        let Some(&HouseDeed { multi }) = self.state.registry.get::<HouseDeed>(deed) else {
            return; // not a deed any more, or never was
        };
        // Still theirs. A deed in somebody else's pack is not a deed you hold,
        // and the walk up the containment tree is `items`' own — a deed in a bag
        // in the backpack is carried as surely as one loose in it.
        let carried = self
            .state
            .registry
            .get::<Contained>(deed)
            .and_then(|held| items::owner_of_container(&self.state, held.container))
            == Some(actor);
        if !carried {
            self.state.system_message(actor, "You no longer have that deed.");
            return;
        }
        let facet = self.state.facet_of(actor);
        let Some(owner) = self.state.registry.serial_of(actor) else {
            return;
        };
        match openshard_housing::place(&mut self.state, actor, at, facet, multi, owner) {
            Ok(_) => {
                if let Some(serial) = self.state.registry.serial_of(deed) {
                    items::consume(&mut self.state, serial, 1);
                }
                self.state
                    .system_message(actor, "The house is built. Use the sign to name it.");
            }
            // The deed is **not** spent on a refusal. ServUO puts it back in the
            // pack for the same reason: a player who picked a bad spot has lost
            // nothing but a click.
            Err(refusal) => self.state.system_message(actor, refusal.message()),
        }
    }
}

impl World {
    /// A deed was double-clicked: raise the cursor with the house drawn under it.
    ///
    /// `0x99` rather than `0x6C`, which is the whole point of the packet — a
    /// player picking a plot needs to see the walls, because five of ServUO's
    /// placement rules are about what is *around* the footprint and none of them
    /// can be judged from a crosshair on one tile.
    pub(super) fn offer_a_plot(&mut self, player: EntityId, deed: EntityId) {
        let Some(&HouseDeed { multi }) = self.state.registry.get::<HouseDeed>(deed) else {
            return;
        };
        let (Some(&Client { connection, .. }), Some(serial)) = (
            self.state.registry.get::<Client>(player),
            self.state.registry.serial_of(player),
        ) else {
            return; // a creature holding a deed has no cursor to raise
        };
        self.state
            .raise_target(player, TargetPurpose::PlaceHouse { deed });
        self.state.send_packet(
            connection,
            &ServerPacket::MultiTarget(MultiTargetRequest {
                cursor_id: CursorId(serial.raw()),
                // A house goes on the ground. An object cursor would refuse the
                // click on the grass the player is trying to build on.
                kind: TargetKind::Location,
                multi,
                offset: (0, 0, 0),
            }),
        );
        self.state
            .system_message(player, "Where would you like to place the house?");
    }
}

impl World {
    /// Pull down every house whose period is up.
    ///
    /// Beside the item decay sweep and on the same cadence, which is every tick.
    /// A scan over the houses rather than a queue of deadlines: there are a
    /// handful of them on a shard, and a deadline queue would be a second copy
    /// of `refreshed_at` to keep in step through every refresh.
    pub(super) fn collapse_houses(&mut self) {
        for house in openshard_housing::decay::age_and_collect(&mut self.state) {
            let owner = self.state.registry.get::<House>(house).map(|entry| entry.owner);
            openshard_housing::decay::demolish(&mut self.state, house);
            // The owner is told if they are here to hear it. Nothing is sent to
            // an absent one: this engine has no offline mail, and inventing one
            // for a single line is a system rather than a message.
            if let Some(owner) = owner.and_then(|serial| self.state.registry.entity_of(serial)) {
                self.state
                    .system_message(owner, "Your house has collapsed. What it held is in a crate.");
            }
            info!("a house collapsed");
        }
    }

    /// Sail every ship whose cadence is up, and tell the crew of any that
    /// stopped.
    ///
    /// Beside [`collapse_houses`](Self::collapse_houses) and `items::close_doors`
    /// — the two passes that already do the halves of this, a multi that changes
    /// and an item that moves on a clock, and never together. Every tick, with
    /// the cadence inside `boats::sail`: the gate is per ship, because two ships
    /// may be under way at different speeds.
    ///
    /// Whoever is told is worked out here rather than in `openshard-boats`, for
    /// the reason the collapse above states: that crate has no opinion about
    /// messages and this one does.
    pub(super) fn sail_boats(&mut self) {
        for boat in openshard_boats::sail(&mut self.state) {
            let owner = self
                .state
                .registry
                .get::<openshard_state::components::Boat>(boat)
                .map(|entry| entry.owner);
            if let Some(owner) = owner.and_then(|serial| self.state.registry.entity_of(serial)) {
                self.state
                    .system_message(owner, "The ship can go no further that way.");
            }
        }
    }

    /// A sign was double-clicked: open its house's window.
    ///
    /// The house is looked up by serial *now*. A sign left standing over a house
    /// that has come down opens nothing, which is what the serial buys over the
    /// entity id it was hung with.
    pub(super) fn open_house_sign(&mut self, player: EntityId, sign: EntityId) {
        let Some(&HouseSign { house }) = self.state.registry.get::<HouseSign>(sign) else {
            return;
        };
        if let Some(house) = self.state.registry.entity_of(house) {
            openshard_housing::sign::show(&mut self.state, player, house);
        }
    }

    /// The house `who` is standing in, if any.
    pub(super) fn house_at(&self, at: Point, facet: Facet) -> Option<EntityId> {
        openshard_housing::house_at(&self.state, at, facet)
    }
}

impl World {
    /// The house-list cursor came back with somebody: make the change.
    ///
    /// The house is the one the **actor** is standing in, read now rather than
    /// when the cursor went up — the same rule the deed's placement follows, and
    /// for the same reason: whoever walked out of their house between raising the
    /// cursor and answering it is no longer changing that house's lists.
    pub(super) fn change_house_list_for(
        &mut self,
        actor: EntityId,
        change: openshard_state::HouseChange,
        who: Option<Serial>,
    ) {
        let Some(who) = who else {
            self.state.system_message(actor, "That is nobody.");
            return;
        };
        let Some(&Position(at)) = self.state.registry.get::<Position>(actor) else {
            return;
        };
        let facet = self.state.facet_of(actor);
        let Some(house) = self.house_at(at, facet) else {
            self.state
                .system_message(actor, "You are not standing in a house.");
            return;
        };
        // Through the sign's own `apply`, so the cursor and the window's rows are
        // one authority check and one eviction rather than two that must agree.
        openshard_housing::sign::apply(&mut self.state, actor, house, change, who);
    }

    /// The storage cursor came back with an item: pin it, secure it, or let it
    /// go.
    ///
    /// The house rides on the cursor rather than being read off where the actor
    /// stands — see `TargetPurpose::HouseStorage` for why the two differ — and
    /// the sign is re-drawn afterwards, because the number it shows is the one
    /// that just changed.
    pub(super) fn change_house_storage(
        &mut self,
        actor: EntityId,
        house: EntityId,
        change: openshard_state::HouseStorage,
        item: Option<Serial>,
    ) {
        use openshard_state::HouseStorage as Change;

        let Some(item) = item.and_then(|serial| self.state.registry.entity_of(serial)) else {
            self.state.system_message(actor, "That is nothing.");
            return;
        };
        let outcome = match change {
            Change::LockDown => {
                openshard_housing::storage::lock_down(&mut self.state, actor, house, item, None)
            }
            Change::Secure(access) => {
                openshard_housing::storage::lock_down(&mut self.state, actor, house, item, Some(access))
            }
            Change::Release => openshard_housing::storage::release(&mut self.state, actor, house, item),
        };
        match outcome {
            Ok(()) => self.state.system_message(actor, "Done."),
            Err(refusal) => self.state.system_message(actor, refusal.message()),
        }
        openshard_housing::sign::show(&mut self.state, actor, house);
    }
}
