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
use openshard_map::grid::Tile;
use openshard_map::overlay::Doors;
use openshard_persistence::record::HouseRecord;
use openshard_protocol::serial::{
    RawSerial,
    Serial,
};
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::target::{
    MultiOffset,
    MultiTargetRequest,
    TargetCursor,
    TargetKind,
};
use openshard_protocol::wire::{
    CursorId,
    Graphic,
    Hue,
};
use openshard_protocol::world::{
    Facet,
    Point,
};
use openshard_state::components::{
    AddonDeed,
    AddonKind,
    AddonPart,
    Client,
    Drawn,
    House,
    HouseDeed,
    HouseDesign,
    HouseSign,
    Position,
};
use openshard_state::{
    ItemLocation as LiveItemLocation,
    TargetPurpose,
    establish_item_location,
};
use tracing::{
    info,
    warn,
};

use super::World;

impl World {
    /// Install a crafted addon at the selected house tile and consume its deed.
    pub(super) fn place_addon_from_deed(&mut self, actor: EntityId, deed: EntityId, at: Point) {
        let Some(&AddonDeed { addon }) = self.state.registry.get::<AddonDeed>(deed) else {
            return;
        };
        if !self.deed_still_carried(actor, deed) {
            return;
        }
        let facet = self.state.facet_of(actor);
        let Some(house) = self.house_at(at, facet) else {
            self.state
                .system_message(actor, "That must be placed inside a house.");
            return;
        };
        // Read from `deco_addons.json` through the same table the world's own
        // pre-placed decoration is flattened from, so this geometry cannot drift
        // from what already stands on the map — see docs/crafting.md's review.
        let parts: Vec<crate::decoration::AddonComponent> = match addon {
            AddonKind::StoneOvenEast => {
                crate::decoration::addon_components("StoneOvenEastAddon")
                    .expect("StoneOvenEastAddon is in deco_addons.json")
                    .to_vec()
            }
            AddonKind::StoneOvenSouth => {
                crate::decoration::addon_components("StoneOvenSouthAddon")
                    .expect("StoneOvenSouthAddon is in deco_addons.json")
                    .to_vec()
            }
            AddonKind::LoomEast => {
                crate::decoration::addon_components("LoomEastAddon")
                    .expect("LoomEastAddon is in deco_addons.json")
                    .to_vec()
            }
            AddonKind::LoomSouth => {
                crate::decoration::addon_components("LoomSouthAddon")
                    .expect("LoomSouthAddon is in deco_addons.json")
                    .to_vec()
            }
            // Nothing below is pre-placed on this facet, so `deco_addons.json`
            // never imported one and there is no generated row to read. The
            // geometry is ServUO's own addon constructor: one tile at the origin,
            // the facing being the graphic and nothing else.
            AddonKind::ElvenOvenEast => vec![(Graphic(0x2DDB), 0, 0, 0)],
            AddonKind::ElvenOvenSouth => vec![(Graphic(0x2DDC), 0, 0, 0)],
            // A wheel is installed at rest, and its resting art is read from
            // `wheel_arts` rather than written here a second time — the two
            // would then have to agree, which is #5's own defect one shelf over.
            AddonKind::SpinningWheelEast
            | AddonKind::SpinningWheelSouth
            | AddonKind::ElvenSpinningWheelEast
            | AddonKind::ElvenSpinningWheelSouth => {
                let (idle, _) = addon.wheel_arts().expect("every spinning wheel has its two arts");
                vec![(idle, 0, 0, 0)]
            }
        };
        if !openshard_housing::storage::has_room_for(&self.state, house, parts.len()) {
            self.state
                .system_message(actor, "This house cannot hold any more.");
            return;
        }
        // Resolve and validate every component's absolute tile before spawning
        // anything, so a bad tile refuses the whole placement rather than leaving
        // half an oven behind. `dz` is honoured now rather than dropped: today's
        // two ovens both carry zero, but a future addon's `deco_addons.json` row
        // need not.
        let mut tiles = Vec::with_capacity(parts.len());
        for &(graphic, dx, dy, dz) in &parts {
            let x = i32::from(at.x) + i32::from(dx);
            let y = i32::from(at.y) + i32::from(dy);
            let z = i32::from(at.z) + i32::from(dz);
            let (Ok(x), Ok(y), Ok(z)) = (u16::try_from(x), u16::try_from(y), i8::try_from(z)) else {
                self.state.system_message(actor, "That cannot be placed there.");
                return;
            };
            let point = Point::new(x, y, z);
            if self.house_at(point, facet) != Some(house) {
                let name = addon.name();
                self.state
                    .system_message(actor, &format!("The whole {name} must fit inside the house."));
                return;
            }
            if !self.addon_tile_is_free(facet, house, graphic, point) {
                self.state
                    .system_message(actor, "There is no room for that there.");
                return;
            }
            tiles.push((graphic, point));
        }
        let mut installed = Vec::with_capacity(tiles.len());
        for &(graphic, point) in &tiles {
            let Some(item) = items::spawn_item(&mut self.state, graphic, Hue(0), 1, false, point, facet)
            else {
                for item in installed {
                    self.state.unplace(facet, item);
                    openshard_state::despawn_item(&mut self.state, item);
                }
                self.state
                    .system_message(actor, "There is no room to place that now.");
                return;
            };
            if let Err(refusal) =
                openshard_housing::storage::lock_down(&mut self.state, actor, house, item, None)
            {
                self.state.unplace(facet, item);
                openshard_state::despawn_item(&mut self.state, item);
                for item in installed {
                    self.state.unplace(facet, item);
                    openshard_state::despawn_item(&mut self.state, item);
                }
                self.state.system_message(actor, refusal.message());
                return;
            }
            installed.push(item);
        }
        // The parts become one addon. The group's name is the first component's
        // serial, which the root carries too, so `AddonPart` alone answers both
        // "what am I part of" and "what else is part of it" — see the component's
        // own docs. A component with no serial cannot be named by the group and
        // cannot name it either; that is impossible for a freshly spawned item,
        // so it is asserted rather than tolerated.
        let root = self
            .state
            .registry
            .serial_of(installed[0])
            .expect("a freshly spawned addon component has a serial");
        for &item in &installed {
            self.state.registry.insert(item, AddonPart { addon, root });
        }
        if let Some(serial) = self.state.registry.serial_of(deed) {
            items::consume(&mut self.state, serial, 1);
        }
        let name = addon.name();
        self.state
            .system_message(actor, &format!("The {name} is installed and locked down."));
    }

    /// Take a whole installed addon down and hand its deed back.
    ///
    /// A stone oven is two locked-down items, and releasing one of them used to
    /// unpin exactly that tile: half an oven left standing, nothing refunded, and
    /// no way to put it back together. ServUO's `BaseAddon` answers a release by
    /// deleting itself whole and giving the deed back, and this is that rule —
    /// the group being [`AddonPart`], not a guess from what happens to stand
    /// next to what.
    ///
    /// **Order is load-bearing.** Permission is asked first, then the deed goes
    /// into the pack, and only then does the oven stop existing: giving the deed
    /// can fail on a full pack, and a player who cannot carry it must keep the
    /// oven rather than lose both. Removing the parts afterwards cannot fail, so
    /// there is no window where the deed and the oven both exist.
    fn release_addon(&mut self, actor: EntityId, house: EntityId, part: AddonPart) {
        if let Err(refusal) = openshard_housing::storage::may_change(&self.state, actor, house) {
            self.state.system_message(actor, refusal.message());
            return;
        }
        let Some(owner) = self.state.registry.serial_of(actor) else {
            return;
        };
        // Every tile of *this* addon, asked of this house's own lockdown list:
        // the group is only meaningful among things pinned here, and a component
        // that has gone loose has already dropped its `AddonPart`.
        let parts: Vec<EntityId> = openshard_housing::storage::locked_down(&self.state, house)
            .into_iter()
            .filter(|&item| {
                self.state
                    .registry
                    .get::<AddonPart>(item)
                    .is_some_and(|other| other.root == part.root)
            })
            .collect();
        if parts.is_empty() {
            return;
        }
        if !items::give_kind_to_backpack(&mut self.state, owner, part.addon.deed_kind(), None, 1, false) {
            self.state
                .system_message(actor, "Your pack has no room for the deed.");
            return;
        }
        let facet = self.state.facet_of(house);
        for item in parts {
            // Unpinned before it is removed, so the house's own lockdown count and
            // its storage projection are told; the grouping goes with the pin.
            self.state.set_item_lockdown(item, None);
            self.state.unplace(facet, item);
            openshard_state::despawn_item(&mut self.state, item);
        }
        let name = part.addon.name();
        self.state.system_message(
            actor,
            &format!("The {name} is taken up, and the deed is in your pack."),
        );
    }

    /// Whether an addon component may stand at `point`: nothing solid already
    /// there, and no other locked-down item already sitting on the same spot.
    ///
    /// `housing::place` asks the wall/floor half of this question for a whole
    /// building's footprint through [`openshard_movement::can_fit`]; an addon
    /// grows a house that already passed that check, one or two tiles at a
    /// time, so this is the per-tile version. Reused rather than reimplemented:
    /// a wall, a door, or the world's own decoration all register themselves
    /// into the facet's obstruction index the same way, so `can_fit` already
    /// knows about every one of them.
    ///
    /// **What `can_fit` cannot see**: an ordinary locked-down item never
    /// registers there (see docs/crafting.md's review) — a second oven, or any
    /// other piece of house furniture, stacks on the first invisibly as far as
    /// the obstruction index is concerned. Asked directly against the house's
    /// own storage list instead, which is small and already loaded to check
    /// the allowance a few lines up.
    fn addon_tile_is_free(&self, facet: Facet, house: EntityId, graphic: Graphic, point: Point) -> bool {
        let tile = Tile::new(point.x, point.y);
        let height = i32::from(self.state.tiles().static_tile(graphic.0).height.max(1));
        if !openshard_movement::can_fit(
            &self.state.footing(facet, Doors::AsTheyStand),
            tile,
            i32::from(point.z),
            height,
        ) {
            return false;
        }
        openshard_housing::storage::locked_down(&self.state, house)
            .into_iter()
            .all(|other| self.state.registry.get::<Position>(other) != Some(&Position(point)))
    }

    /// Whether a deed is still theirs to spend. A deed in somebody else's pack
    /// is not a deed you hold, and the walk up the containment tree is `items`'
    /// own — a deed in a bag in the backpack is carried as surely as one loose
    /// in it. Sends the refusal itself on `false`, so both deed placements
    /// (house and addon) answer it identically with one call.
    fn deed_still_carried(&mut self, actor: EntityId, deed: EntityId) -> bool {
        let carried = match openshard_state::item_location(&self.state, deed) {
            Some(LiveItemLocation::Settled(openshard_state::SettledItemLocation::Contained(held))) => {
                items::owner_of_container(&self.state, held.container) == Some(actor)
            }
            _ => false,
        };
        if !carried {
            self.state.system_message(actor, "You no longer have that deed.");
        }
        carried
    }

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
                    id:  openshard_protocol::wire::MultiId(record.multi).graphic(),
                    hue: Hue(0),
                },
            );
            self.state.registry.insert(
                entity,
                openshard_state::components::Boat {
                    multi: openshard_protocol::wire::MultiId(record.multi),
                    owner: record.owner,
                },
            );
            establish_item_location(&mut self.state, entity, LiveItemLocation::ground(facet, at))
                .expect("a restored boat has one valid berth");
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
                dx:      row.dx,
                dy:      row.dy,
                dz:      row.dz,
                flags:   row.flags,
            });
        }
        let mut restored = 0;
        let mut wall_less = 0;
        for record in records {
            let facet = Facet(record.facet);
            let at = Point::new(record.x, record.y, record.z);
            // A design packet borrows its grid dimensions from the foundation
            // multi. Older imports always used 0x13EC, the smallest one, which
            // made the client silently lose the middle of wider buildings.
            // Repair that old choice while no client can yet see this restored
            // item; its next ordinary save records the fitting foundation.
            let mut design = by_house.remove(&record.serial);
            let saved_multi = openshard_protocol::wire::MultiId(record.multi);
            let saved_bounds = openshard_uofiles::multi::bounds(self.state.multis.components(saved_multi.0));
            let design_bounds = design
                .as_ref()
                .and_then(|(_, components)| openshard_uofiles::multi::bounds(components));
            let fits_saved_foundation = matches!(
                (saved_bounds, design_bounds),
                (Some(foundation), Some(design))
                    if foundation.min_x <= design.min_x
                        && foundation.max_x >= design.max_x
                        && foundation.min_y <= design.min_y
                        && foundation.max_y >= design.max_y
            );
            let multi = if fits_saved_foundation {
                saved_multi
            } else if let Some((revision, components)) = design.take() {
                match openshard_housing::fit_design_to_foundation(&self.state.multis, components.clone()) {
                    Some((multi, components)) => {
                        design = Some((revision, components));
                        multi
                    }
                    None => {
                        design = Some((revision, components));
                        saved_multi
                    }
                }
            } else {
                saved_multi
            };
            if multi != saved_multi {
                info!(
                    serial = %record.serial,
                    from = saved_multi.0,
                    to = multi.0,
                    "resized a restored custom-house foundation for its design"
                );
            }
            let entity = self.state.registry.spawn();
            if self.state.registry.bind_serial(entity, record.serial).is_err() {
                warn!(serial = %record.serial, "a saved house's serial was already taken");
                self.state.registry.despawn(entity);
                continue;
            }
            self.state.registry.insert(
                entity,
                Drawn {
                    id:  multi.graphic(),
                    hue: Hue(0),
                },
            );
            self.state.registry.insert(
                entity,
                House {
                    multi,
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
            establish_item_location(&mut self.state, entity, LiveItemLocation::ground(facet, at))
                .expect("a restored house has one valid plot");
            self.state.place_item(facet, entity, at);
            // The design, if this house has one. Put on before the footprint is
            // computed, because the footprint is computed *from* it.
            if let Some((revision, components)) = design.clone() {
                self.state
                    .registry
                    .insert(entity, HouseDesign { components, revision });
            }
            let shape = design.as_ref().map(|(_, components)| components.as_slice());

            match openshard_housing::footprint_of(&self.state, at, multi, shape) {
                Ok(footprint) => {
                    openshard_housing::block(&mut self.state, entity, facet, &footprint);
                }
                // No client files, or an id this install does not know. The house
                // is still there and still owned; it simply stops nobody, which
                // is said out loud rather than left to be found by walking
                // through a wall.
                Err(_) => wall_less += 1,
            }
            // Classic doors are separate saved decoration, not multi pieces.
            // Decoration was restored before houses, so this adopts the saved
            // leaves and creates only fixtures absent from an older save.
            openshard_housing::install_doors(&mut self.state, entity, facet, at, multi);
            // Rebuilt rather than restored, for the module header's reason: the
            // sign's spot is a pure function of the classic house type or a
            // designed foundation's box, and a saved copy would go stale when a
            // design changes.
            // A shard with no client files gets no sign, the same bargain the
            // walls make.
            openshard_housing::hang_sign_for_design(&mut self.state, entity, facet, at, multi);
            restored += 1;
        }
        if restored > 0 {
            info!(houses = restored, wall_less, "restored the shard's houses");
        }
    }
}

impl World {
    /// Raise a normal location cursor for a crafted house addon.
    pub(super) fn offer_addon_placement(&mut self, player: EntityId, deed: EntityId) {
        let Some(addon_deed) = self.state.registry.get::<AddonDeed>(deed).copied().or_else(|| {
            self.state
                .registry
                .get::<openshard_state::components::ItemKind>(deed)
                .and_then(|kind| AddonDeed::from_item_kind(kind.0))
        }) else {
            return;
        };
        // Vendor and admin-created items carry their `ItemKind` from creation,
        // unlike crafted deeds, which receive this transient component
        // immediately. Make the identity explicit before the target cursor
        // carries the entity.
        self.state.registry.insert(deed, addon_deed);
        let (Some(&Client { connection, .. }), Some(serial)) = (
            self.state.registry.get::<Client>(player),
            self.state.registry.serial_of(player),
        ) else {
            return;
        };
        self.state
            .raise_target(player, TargetPurpose::PlaceAddon { deed });
        self.state.send_packet(
            connection,
            &ServerPacket::TargetCursor(TargetCursor {
                cursor_id: CursorId(serial.raw()),
                kind:      TargetKind::Location,
            }),
        );
        let name = addon_deed.addon.name();
        self.state
            .system_message(player, &format!("Where would you like to place the {name}?"));
    }

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
        if !self.deed_still_carried(actor, deed) {
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
                offset: MultiOffset::default(),
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
            if let Err(error) = openshard_housing::decay::demolish(&mut self.state, house) {
                warn!(?house, ?error, "a collapsed house could not be demolished");
                continue;
            }
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
        // A release aimed at one tile of an installed addon is a release of the
        // whole addon — see [`release_addon`](Self::release_addon). Asked before
        // the ordinary paths, because the ordinary path would unpin that one tile
        // and leave the rest of the oven standing.
        if matches!(change, Change::Release) {
            if let Some(&part) = self.state.registry.get::<AddonPart>(item) {
                self.release_addon(actor, house, part);
                openshard_housing::sign::show(&mut self.state, actor, house);
                return;
            }
        }
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
