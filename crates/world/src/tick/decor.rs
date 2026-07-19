use super::*;

impl World {
    /// Place a batch of decoration: script-added statics the shard puts on top of
    /// the map's art, plus the interactive kinds — doors and containers. Each is an
    /// item — a `Graphic` and a `Position`, drawn to clients through the same
    /// `0x1A`/interest path as any item — but marked [`Decoration`], so it never
    /// decays and cannot be picked up. A door also carries [`Door`] (toggled by
    /// double-click) and a container [`Container`] (opened by double-click). See
    /// [`crate::gm`] and `items::pick_up`.
    pub(super) fn decorate(
        &mut self,
        facet: u8,
        statics: &[(u16, u16, Point)],
        doors: &[DecorDoor],
        containers: &[DecorContainer],
    ) {
        let facet = if self.state.facets.contains_key(&facet) {
            facet
        } else {
            self.state.default_facet
        };
        // A closure that spawns one decoration item at a tile and reveals it,
        // returning the entity so the caller can hang a `Door` or `Container` on
        // it. `None` when the serial pool is empty.
        for &(graphic, hue, position) in statics {
            if self
                .place_decoration(facet, graphic, hue, position)
                .is_none()
            {
                return;
            }
        }
        for door in doors {
            let Some(entity) = self.place_decoration(facet, door.closed, 0, door.position) else {
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
            self.state.facet_state_mut(facet).obstructions.block(
                door.position.x,
                door.position.y,
                entity,
                true,
            );
        }
        for container in containers {
            let Some(entity) =
                self.place_decoration(facet, container.graphic, container.hue, container.position)
            else {
                return;
            };
            self.state.registry.insert(
                entity,
                Container {
                    gump: container.gump,
                },
            );
        }
    }

    /// Spawn one decoration item — a `Graphic`, `Position`, `Facet` and the
    /// [`Decoration`] marker — index it and draw it to everyone in range. Returns
    /// its entity, or `None` if the item-serial pool is empty (the caller stops the
    /// batch).
    pub(super) fn place_decoration(
        &mut self,
        facet: u8,
        graphic: u16,
        hue: u16,
        position: Point,
    ) -> Option<EntityId> {
        let Ok((entity, _serial)) = self.state.registry.spawn_with_serial(SerialKind::Item) else {
            warn!("out of item serials; stopping decoration");
            return None;
        };
        self.state
            .registry
            .insert(entity, Graphic { id: graphic, hue });
        self.state.registry.insert(entity, Position(position));
        self.state.registry.insert(entity, Facet(facet));
        self.state.registry.insert(entity, Decoration);
        // Placed art with impassable tiledata blocks its tile, the way ServUO
        // treats any non-movable impassable item; doors refine this to a door
        // obstacle right after.
        let blocks = self
            .state
            .facet_state(facet)
            .terrain
            .as_deref()
            .is_some_and(|t| t.item_blocks(graphic));
        if blocks {
            self.state
                .facet_state_mut(facet)
                .obstructions
                .block(position.x, position.y, entity, false);
        }
        self.state
            .facet_state_mut(facet)
            .sectors
            .insert(entity, position);
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
    pub(super) fn generate_doors(&mut self, facet: u8, x: u16, y: u16, width: u16, height: u16) {
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
        let mut placements: Vec<(u16, u16, i16, i16, Point)> = Vec::new();
        {
            let Some(terrain) = self.state.facet_state(facet).terrain.as_ref() else {
                warn!(facet, "no map on this facet; no doors to generate");
                return;
            };
            // Is there a frame of the given side at (tx, ty) sharing height z?
            let frame_at = |tx: u16, ty: u16, tz: i8, pred: fn(u16) -> bool| -> bool {
                let mut here = Vec::new();
                terrain.statics_at(tx, ty, &mut here);
                here.iter().any(|&(id, z)| z == tz && pred(id))
            };
            // Place a door in the gap, but only if a door actually fits there — an
            // open doorway with a floor, not a solid wall or thin air — and it is
            // not already doored. `can_fit` is ServUO's `CanFit` guard (16 tall);
            // the `occupied` set is our own de-dup.
            let mut try_place = |gap: Point, door: (u16, u16, i16, i16)| {
                let key = (gap.x, gap.y);
                if occupied.contains(&key) || !terrain.can_fit(gap.x, gap.y, i32::from(gap.z), 16) {
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
                    terrain.statics_at(vx, vy, &mut here);
                    for &(id, z) in &here {
                        if doorgen::is_west_frame(id) {
                            // A single door: one gap tile to an east frame two away.
                            if east(vx).is_some_and(|e| frame_at(e, vy, z, doorgen::is_east_frame))
                            {
                                try_place(
                                    Point::new(vx + 1, vy, z),
                                    doorgen::GenFacing::WestCw.door(),
                                );
                            } else if vx
                                .checked_add(3)
                                .is_some_and(|e| frame_at(e, vy, z, doorgen::is_east_frame))
                            {
                                // A double door fills the two-tile gap.
                                try_place(
                                    Point::new(vx + 1, vy, z),
                                    doorgen::GenFacing::WestCw.door(),
                                );
                                try_place(
                                    Point::new(vx + 2, vy, z),
                                    doorgen::GenFacing::EastCcw.door(),
                                );
                            }
                        } else if doorgen::is_north_frame(id) {
                            if vy
                                .checked_add(2)
                                .is_some_and(|s| frame_at(vx, s, z, doorgen::is_south_frame))
                            {
                                try_place(
                                    Point::new(vx, vy + 1, z),
                                    doorgen::GenFacing::SouthCw.door(),
                                );
                            } else if vy
                                .checked_add(3)
                                .is_some_and(|s| frame_at(vx, s, z, doorgen::is_south_frame))
                            {
                                try_place(
                                    Point::new(vx, vy + 1, z),
                                    doorgen::GenFacing::NorthCcw.door(),
                                );
                                try_place(
                                    Point::new(vx, vy + 2, z),
                                    doorgen::GenFacing::SouthCw.door(),
                                );
                            }
                        }
                    }
                }
            }
        }

        let count = placements.len();
        for (closed, open, offset_x, offset_y, position) in placements {
            if let Some(entity) = self.place_decoration(facet, closed, 0, position) {
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
                self.state
                    .facet_state_mut(facet)
                    .obstructions
                    .block(position.x, position.y, entity, true);
            }
        }
        debug!(facet, count, "generated doors from static frames");
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
                self.state
                    .facet_state_mut(facet)
                    .obstructions
                    .unblock(at.x, at.y, entity);
            }
            self.state.facet_state_mut(facet).sectors.remove(entity);
            self.state.registry.despawn(entity);
        }
    }
}
