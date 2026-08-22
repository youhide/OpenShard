use super::*;
use openshard_protocol::wire::{Graphic, Hue};

impl World {
    /// Place a batch of decoration: script-added statics the shard puts on top of
    /// the map's art, plus the interactive kinds — doors and containers. Each is an
    /// item — a `Drawn` and a `Position`, drawn to clients through the same
    /// `0x1A`/interest path as any item — but marked [`Decoration`], so it never
    /// decays and cannot be picked up. A door also carries [`Door`] (toggled by
    /// double-click) and a container [`Container`] (opened by double-click). See
    /// [`crate::gm`] and `items::pick_up`.
    ///
    /// # Laying the same batch twice
    ///
    /// A row already standing — same graphic, same tile, same facet — is skipped.
    /// Decoration is additive and **persisted**, so without this a second press of
    /// the staff button, or a boot that seeds `decorate:` on a restored shard, laid
    /// a second Britain inside the first: two of every sign, two of every chest,
    /// and a door that opened into its own twin.
    ///
    /// **Against the world as it stood when the batch began, not against the batch.**
    /// Thirty-nine of the shipped statics repeat an exact graphic and position, and
    /// 1,471 tiles hold several — an ordinary thing in UO decoration, where a tile
    /// carries a floor, a rug, and what stands on the rug. Skipping a row because an
    /// *earlier row of the same batch* placed it would quietly drop those, so the
    /// snapshot is taken once, up front, and never added to. A first lay is
    /// therefore byte for byte what it always was.
    pub(super) fn decorate(
        &mut self,
        facet: Facet,
        statics: &[(Graphic, Hue, Point)],
        doors: &[DecorDoor],
        containers: &[DecorContainer],
    ) {
        let facet = if self.state.facets.contains_key(&facet) {
            facet
        } else {
            self.state.default_facet
        };
        let standing = self.decoration_already_placed(facet);
        // A closure that spawns one decoration item at a tile and reveals it,
        // returning the entity so the caller can hang a `Door` or `Container` on
        // it. `None` when the serial pool is empty.
        for &(graphic, hue, position) in statics {
            if standing.contains(&(graphic, position)) {
                continue;
            }
            if self.place_decoration(facet, graphic, hue, position).is_none() {
                return;
            }
        }
        for door in doors {
            if standing.contains(&(door.closed, door.position)) {
                continue;
            }
            let Some(entity) = self.place_decoration(facet, door.closed, Hue(0), door.position) else {
                return;
            };
            self.state.registry.insert(
                entity,
                Door {
                    closed: door.closed,
                    open: door.open,
                    offset_x: door.offset_x,
                    offset_y: door.offset_y,
                    is_open: false,
                    close_at: 0,
                },
            );
            if door.key_value != 0 {
                self.state.registry.insert(
                    entity,
                    openshard_state::components::Lock {
                        key_value: door.key_value,
                        ..Default::default()
                    },
                );
            }
            self.state.facet_state_mut(facet).block(
                door.position.x,
                door.position.y,
                entity,
                true,
                door.position.z,
                openshard_state::DOOR_HEIGHT,
            );
        }
        for container in containers {
            if standing.contains(&(container.graphic, container.position)) {
                continue;
            }
            let Some(entity) =
                self.place_decoration(facet, container.graphic, container.hue, container.position)
            else {
                return;
            };
            self.state
                .registry
                .insert(entity, Container { gump: container.gump });
            if container.key_value != 0 {
                self.state.registry.insert(
                    entity,
                    openshard_state::components::Lock {
                        key_value: container.key_value,
                        ..Default::default()
                    },
                );
            }
        }
    }

    /// Every decoration standing on `facet` right now, as the `(graphic, tile)`
    /// pair [`decorate`](World::decorate) de-duplicates by.
    ///
    /// Built once per batch rather than queried per row: twenty-five thousand rows
    /// against a growing world is a quadratic scan, and this is one pass.
    ///
    /// The key is deliberately narrow. Hue is not in it — a hued copy of a static
    /// on the same tile is the same decoration recoloured, not a second one — and
    /// neither is the door or container hanging off it, because a door and a static
    /// never share a graphic.
    fn decoration_already_placed(&self, facet: Facet) -> std::collections::HashSet<(Graphic, Point)> {
        self.state
            .registry
            .query::<Decoration>()
            .filter_map(|(entity, _)| {
                (self.state.facet_of(entity) == facet).then(|| {
                    let drawn = self.state.registry.get::<Drawn>(entity)?;
                    let &Position(at) = self.state.registry.get::<Position>(entity)?;
                    Some((drawn.id, at))
                })?
            })
            .collect()
    }

    /// Spawn one decoration item — a `Drawn`, `Position`, `Facet` and the
    /// [`Decoration`] marker — index it and draw it to everyone in range. Returns
    /// its entity, or `None` if the item-serial pool is empty (the caller stops the
    /// batch).
    pub(super) fn place_decoration(
        &mut self,
        facet: Facet,
        graphic: Graphic,
        hue: Hue,
        position: Point,
    ) -> Option<EntityId> {
        let Ok((entity, _serial)) = self.state.registry.spawn_with_serial(SerialKind::Item) else {
            warn!("out of item serials; stopping decoration");
            return None;
        };
        self.state.registry.insert(entity, Drawn { id: graphic, hue });
        self.state.registry.insert(entity, Position(position));
        self.state.registry.insert(entity, facet);
        self.state.registry.insert(entity, Decoration);
        // Placed art with impassable tiledata blocks its tile, the way ServUO
        // treats any non-movable impassable item; doors refine this to a door
        // obstacle right after. It blocks only its own z-span — its base z and
        // tiledata height — so an upper-floor wall does not seal the ground floor
        // beneath it (the Britain-library bug).
        let height = Some(self.state.tiles.static_tile(graphic.0))
            .filter(|tile| tile.flags.is_blocking())
            .map(|tile| tile.height);
        if let Some(height) = height {
            self.state
                .facet_state_mut(facet)
                .block(position.x, position.y, entity, false, position.z, height);
        }
        self.state.facet_state_mut(facet).sectors.insert(entity, position);
        self.state.reveal(entity);
        Some(entity)
    }

    /// Generate functional doors from the map's static door frames in a region.
    ///
    /// ServUO's `DoorGenerator`, ported (see [`crate::doorgen`]): where a west
    /// frame faces an east frame across a one- or two-tile gap — or a north faces a
    /// south — a `DarkWoodDoor` (single) or a linked pair (double) is dropped into
    /// the gap, so a building's implied shop door becomes one that opens. Reading
    /// the terrain and placing entities cannot overlap borrows, so the scan
    /// collects every placement first and lays them down after.
    pub(super) fn generate_doors(&mut self, facet: Facet, x: u16, y: u16, width: u16, height: u16) {
        let facet = if self.state.facets.contains_key(&facet) {
            facet
        } else {
            self.state.default_facet
        };

        // Tiles that already hold a door — the named metal/special doors placed
        // from decoration data, and doors generated earlier in this same pass. A
        // generated door never lands on one of these, which is what stops the bank
        // door being doubled and a doorway being filled twice.
        let door_entities: Vec<EntityId> = self
            .state
            .registry
            .query::<Door>()
            .map(|(entity, _)| entity)
            .collect();
        let mut occupied: HashSet<(u16, u16)> = HashSet::new();
        for entity in door_entities {
            if self.state.facet_of(entity) == facet {
                if let Some(&Position(p)) = self.state.registry.get::<Position>(entity) {
                    occupied.insert((p.x, p.y));
                }
            }
        }

        // (closed, open, offset_x, offset_y, where-it-sits-closed).
        let mut placements: Vec<(Graphic, Graphic, i16, i16, Point)> = Vec::new();
        {
            let Some(terrain) = self.state.map_terrain(facet) else {
                warn!(facet = %facet, "no map on this facet; no doors to generate");
                return;
            };
            // Is there a frame of the given side at (tx, ty) sharing height z?
            let frame_at = |tx: u16, ty: u16, tz: i8, pred: fn(Graphic) -> bool| -> bool {
                let mut here = Vec::new();
                terrain.statics_at(Tile::new(tx, ty), &mut here);
                here.iter().any(|&(id, z)| z == tz && pred(id))
            };
            // Place a door in the gap, but only if a door actually fits there — an
            // open doorway with a floor, not a solid wall or thin air — and it is
            // not already doored. `can_fit` is ServUO's `CanFit` guard (16 tall);
            // the `occupied` set is our own de-dup.
            let mut try_place = |gap: Point, door: (Graphic, Graphic, i16, i16)| {
                let key = (gap.x, gap.y);
                if occupied.contains(&key) || !terrain.can_fit(Tile::new(gap.x, gap.y), i32::from(gap.z), 16)
                {
                    return;
                }
                occupied.insert(key);
                let (c, o, ox, oy) = door;
                placements.push((c, o, ox, oy, gap));
            };
            let east = |vx: u16| vx.checked_add(2);
            let mut here = Vec::new();
            for ry in 0..height {
                for rx in 0..width {
                    let (Some(vx), Some(vy)) = (x.checked_add(rx), y.checked_add(ry)) else {
                        continue;
                    };
                    here.clear();
                    terrain.statics_at(Tile::new(vx, vy), &mut here);
                    for &(id, z) in &here {
                        if doorgen::is_west_frame(id) {
                            // A single door: one gap tile to an east frame two away.
                            if east(vx).is_some_and(|e| frame_at(e, vy, z, doorgen::is_east_frame)) {
                                try_place(Point::new(vx + 1, vy, z), doorgen::GenFacing::WestCw.door());
                            } else if vx
                                .checked_add(3)
                                .is_some_and(|e| frame_at(e, vy, z, doorgen::is_east_frame))
                            {
                                // A double door fills the two-tile gap.
                                try_place(Point::new(vx + 1, vy, z), doorgen::GenFacing::WestCw.door());
                                try_place(Point::new(vx + 2, vy, z), doorgen::GenFacing::EastCcw.door());
                            }
                        } else if doorgen::is_north_frame(id) {
                            if vy
                                .checked_add(2)
                                .is_some_and(|s| frame_at(vx, s, z, doorgen::is_south_frame))
                            {
                                try_place(Point::new(vx, vy + 1, z), doorgen::GenFacing::SouthCw.door());
                            } else if vy
                                .checked_add(3)
                                .is_some_and(|s| frame_at(vx, s, z, doorgen::is_south_frame))
                            {
                                try_place(Point::new(vx, vy + 1, z), doorgen::GenFacing::NorthCcw.door());
                                try_place(Point::new(vx, vy + 2, z), doorgen::GenFacing::SouthCw.door());
                            }
                        }
                    }
                }
            }
        }

        let count = placements.len();
        for (closed, open, offset_x, offset_y, position) in placements {
            if let Some(entity) = self.place_decoration(facet, closed, Hue(0), position) {
                self.state.registry.insert(
                    entity,
                    Door {
                        closed,
                        open,
                        offset_x,
                        offset_y,
                        is_open: false,
                        close_at: 0,
                    },
                );
                self.state.facet_state_mut(facet).block(
                    position.x,
                    position.y,
                    entity,
                    true,
                    position.z,
                    openshard_state::DOOR_HEIGHT,
                );
            }
        }
        debug!(facet = %facet, count, "generated doors from static frames");
    }

    /// Remove every script-placed decoration — "Clear deco".
    pub(super) fn clear_decorations(&mut self) {
        let placed: Vec<EntityId> = self
            .state
            .registry
            .query::<Decoration>()
            .map(|(entity, _)| entity)
            .collect();
        for entity in placed {
            let serial = self.state.registry.serial_of(entity);
            let facet = self.state.facet_of(entity);
            if let Some(serial) = serial {
                for watcher in self.state.watchers_of(entity) {
                    self.state.forget(watcher, entity, serial);
                }
            }
            if let Some(&Position(at)) = self.state.registry.get::<Position>(entity) {
                self.state.facet_state_mut(facet).unblock(at.x, at.y, entity);
            }
            self.state.facet_state_mut(facet).sectors.remove(entity);
            self.state.registry.despawn(entity);
        }
    }
}
