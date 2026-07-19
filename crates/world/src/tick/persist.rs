use super::*;

impl World {
    // -- persistence -------------------------------------------------------

    /// Mark what changed, from what the tick said happened.
    ///
    /// # Why this reads the bus instead of being called from each mutation
    ///
    /// The obvious version is a `journal.touch(entity)` next to every
    /// `registry.insert`. It works, and it decays: the day someone adds a system
    /// that moves a mobile — a teleport, a knockback, a script — they have to
    /// know that persistence exists and remember a line that nothing will fail
    /// without. The bug is silent, it survives every test that does not restart
    /// the shard, and it looks like the disk lost something.
    ///
    /// Emitting the event *is* the touch. A system that moves a mobile already
    /// has to say so, because that is how the client hears about it, and the
    /// same event now also means "and write it down". There is nothing left to
    /// forget.
    pub(super) fn mark_dirty(&mut self) {
        // Collected first: `read` borrows the bus, and the journal is a
        // different field but the iterator holds the borrow across the loop.
        let mut changed: Vec<EntityId> = Vec::new();
        changed.extend(
            self.state
                .bus
                .read(&mut self.entered)
                .map(|event| event.entity),
        );
        changed.extend(
            self.state
                .bus
                .read(&mut self.moved)
                .map(|event| event.entity),
        );
        changed.extend(
            self.state
                .bus
                .read(&mut self.turned)
                .map(|event| event.entity),
        );
        for entity in changed {
            self.journal.touch(entity);
        }
    }

    /// Every `save_every` ticks, hand what changed to whoever is collecting.
    pub(super) fn offer_snapshot(&mut self) {
        if self.save_every == 0 || !self.state.ticks.is_multiple_of(self.save_every) {
            return;
        }
        self.take_snapshot();
    }

    /// Take a snapshot now, whatever the cadence says.
    ///
    /// For shutdown, for a GM save command, and for tests that would rather not
    /// tick four hundred times to see one row.
    pub fn take_snapshot(&mut self) {
        let ticks = self.state.ticks;

        // Start from the journal's logged-out records, their kept inventories, and
        // deletions. Dirty *online*-character records are dropped (the `|_| None`)
        // because every online character is saved in full below regardless — an
        // item picked up without a step never marks the character dirty, so the
        // dirty set is not a safe basis for saving what a character holds.
        let mut snapshot = self.journal.drain(ticks, |_| None).unwrap_or(Snapshot {
            tick: ticks,
            schema: SCHEMA_VERSION,
            characters: Vec::new(),
            removed: Vec::new(),
            inventories: Vec::new(),
            ground: None,
            spawners: None,
        });

        // Every online character, whole: its record and its entire carried
        // inventory — worn gear, backpack, bank box and everything nested. A save is
        // a complete picture of who is here and what they hold, so nothing of value
        // depends on whether its owner happened to move this tick.
        let online: Vec<EntityId> = self.state.players.values().copied().collect();
        for entity in online {
            if let Some(record) = Self::record_of(&self.state.registry, entity) {
                let owner = record.serial;
                snapshot.characters.push(record);
                snapshot.inventories.push(Inventory {
                    owner,
                    items: self.inventory_of(entity),
                });
            }
        }

        // The whole ground, every save — decoration excluded. Dropped loot and
        // stray items persist whether or not anyone was active this tick.
        snapshot.ground = Some(self.ground_items());
        // And every spawn region with its timer, so populated areas stay populated
        // across a restart and a rare spawn's wait is not reset.
        snapshot.spawners = Some(self.spawner_records());

        // Skip only a genuinely empty save, so a quiet, empty shard queues nothing.
        let ground_empty = snapshot.ground.as_ref().is_none_or(Vec::is_empty);
        let spawners_empty = snapshot.spawners.as_ref().is_none_or(Vec::is_empty);
        if snapshot.characters.is_empty()
            && snapshot.removed.is_empty()
            && ground_empty
            && spawners_empty
        {
            return;
        }
        debug!(tick = ticks, rows = snapshot.len(), "snapshot taken");
        self.saves.push(snapshot);
    }

    /// Every item a character is carrying — worn, and inside anything worn, at any
    /// depth — as saveable records owned by that character.
    ///
    /// A breadth-first walk: the worn items first, then the contents of every
    /// container found, and their containers in turn. `owner` is the character on
    /// every record however deep, because that is the key a store replaces a whole
    /// inventory by.
    pub(super) fn inventory_of(&self, entity: EntityId) -> Vec<ItemRecord> {
        let registry = &self.state.registry;
        let Some(owner) = registry.serial_of(entity) else {
            return Vec::new();
        };
        let owner_raw = owner.raw();
        let mut records = Vec::new();
        let mut containers: Vec<Serial> = Vec::new();

        for (item, worn) in registry.query::<Equipped>() {
            if worn.mobile != owner {
                continue;
            }
            // The saddle *is* saved, on the mount layer like any worn item: it
            // carries the mount's graphic, and [`restore_inventory`] rebuilds the
            // ridden creature from it, so the rider logs back in still mounted.
            let location = ItemLocation::Equipped {
                mobile: owner_raw,
                layer: worn.layer,
            };
            if let Some(record) = Self::item_record(registry, item, owner_raw, location) {
                if record.container_gump.is_some() {
                    if let Some(serial) = registry.serial_of(item) {
                        containers.push(serial);
                    }
                }
                records.push(record);
            }
        }

        while let Some(container) = containers.pop() {
            for (item, held) in registry.query::<Contained>() {
                if held.container != container {
                    continue;
                }
                let location = ItemLocation::Contained {
                    container: container.raw(),
                    x: held.x,
                    y: held.y,
                    grid: held.grid,
                };
                if let Some(record) = Self::item_record(registry, item, owner_raw, location) {
                    if record.container_gump.is_some() {
                        if let Some(serial) = registry.serial_of(item) {
                            containers.push(serial);
                        }
                    }
                    records.push(record);
                }
            }
        }
        records
    }

    /// Every loose item on the ground — the dropped and the spawned, but not the
    /// [`Decoration`] a pack re-places and not a mobile — as ownerless records.
    pub(super) fn ground_items(&self) -> Vec<ItemRecord> {
        let registry = &self.state.registry;
        let mut records = Vec::new();
        for (item, Position(at)) in registry.query::<Position>() {
            // A drawable thing on the ground: a graphic, not a mobile (which carries
            // a Body), and not decoration (which the pack owns and re-lays).
            if !registry.has::<Graphic>(item)
                || registry.has::<Body>(item)
                || registry.has::<Decoration>(item)
            {
                continue;
            }
            let facet = self.state.facet_of(item);
            let location = ItemLocation::Ground {
                facet,
                x: at.x,
                y: at.y,
                z: at.z,
            };
            if let Some(record) = Self::item_record(registry, item, 0, location) {
                records.push(record);
            }
        }
        records
    }

    /// Turn one item entity into a saveable record, or `None` if it is not a
    /// drawable item (no graphic or no serial).
    pub(super) fn item_record(
        registry: &Registry,
        item: EntityId,
        owner: u32,
        location: ItemLocation,
    ) -> Option<ItemRecord> {
        let serial = registry.serial_of(item)?;
        let graphic = registry.get::<Graphic>(item)?;
        let amount = registry.get::<Amount>(item).map_or(1, |a| a.0);
        let container_gump = registry.get::<Container>(item).map(|c| c.gump);
        Some(ItemRecord {
            serial: serial.raw(),
            owner,
            graphic: graphic.id,
            hue: graphic.hue,
            amount,
            stackable: registry.has::<Stackable>(item),
            container_gump,
            location,
        })
    }

    /// What a character looks like on disk.
    ///
    /// `None` for anything that is not a character, which is not an error: the
    /// journal tracks entities and the world will hold more than people.
    pub(super) fn record_of(registry: &Registry, entity: EntityId) -> Option<CharacterRecord> {
        let serial = registry.serial_of(entity)?;
        let position = registry.get::<Position>(entity)?.0;
        let heading = registry.get::<Heading>(entity)?.0;
        let body = registry.get::<Body>(entity)?;
        let name = registry.get::<Name>(entity)?;
        // No account means this is not a player character — an NPC, say — so it
        // is not a `CharacterRecord`. Returning `None` drops it from the save,
        // which is the honest answer.
        let account = registry.get::<Account>(entity)?;
        let facet = registry.get::<Facet>(entity).map_or(DEFAULT_FACET, |f| f.0);
        Some(CharacterRecord {
            serial: serial.raw(),
            account: account.0.clone(),
            name: name.0.clone(),
            body: body.id,
            hue: body.hue,
            facet,
            x: position.x,
            y: position.y,
            z: position.z,
            facing: heading.to_bits(),
        })
    }

    /// Reserve a serial read from persistence so a fresh spawn never takes it.
    ///
    /// A logged-out character is not in the world — it is a row in the database —
    /// but its serial is still spoken for. Call this at boot for every stored
    /// character, before anyone can create a new one. Values outside the serial
    /// range are ignored: a corrupt row should not stop the shard from starting.
    pub fn reserve_serial(&mut self, raw: u32) {
        if let Some(serial) = Serial::new(raw) {
            self.state.registry.reserve_serial(serial);
        }
    }

    /// Bring saved items back from the store at boot.
    ///
    /// Reserves every item's serial so a live spawn cannot take it, places the
    /// loose ground items now, and files each character's carried items away by
    /// owner for [`enter`](Self::enter) to equip when that character logs in. Call
    /// once, after the map is loaded and before anyone connects.
    pub fn restore_items(&mut self, records: Vec<ItemRecord>) {
        for record in &records {
            self.reserve_serial(record.serial);
        }
        for record in records {
            if record.owner == 0 {
                self.place_ground_item(&record);
            } else {
                self.pending_inventories
                    .entry(record.owner)
                    .or_default()
                    .push(record);
            }
        }
    }

    /// Put one restored item on the ground, bound to its saved serial.
    pub(super) fn place_ground_item(&mut self, record: &ItemRecord) {
        let ItemLocation::Ground { facet, x, y, z } = record.location else {
            return;
        };
        let Some(serial) = Serial::new(record.serial) else {
            return;
        };
        let facet = if self.state.facets.contains_key(&facet) {
            facet
        } else {
            self.state.default_facet
        };
        let entity = self.state.registry.spawn();
        if self.state.registry.bind_serial(entity, serial).is_err() {
            self.state.registry.despawn(entity);
            return;
        }
        let position = Point::new(x, y, z);
        self.state.registry.insert(
            entity,
            Graphic {
                id: record.graphic,
                hue: record.hue,
            },
        );
        self.state.registry.insert(entity, Position(position));
        self.state.registry.insert(entity, Facet(facet));
        if record.amount > 1 {
            self.state.registry.insert(entity, Amount(record.amount));
        }
        if record.stackable {
            self.state.registry.insert(entity, Stackable);
        }
        if let Some(gump) = record.container_gump {
            self.state.registry.insert(entity, Container { gump });
        }
        // Loose clutter resumes rotting; a container does not (mark_decay skips it).
        items::mark_decay(&mut self.state, entity);
        self.state
            .facet_state_mut(facet)
            .sectors
            .insert(entity, position);
    }

    /// Equip a logging-in character's saved inventory, if any is waiting.
    ///
    /// Two passes so nesting resolves whatever order the records are in: first
    /// spawn every item bound to its saved serial with its graphic and container
    /// mark, then place each — worn on the mobile, or inside the container its
    /// record names, now that every container entity exists. Returns whether an
    /// inventory was restored, so [`enter`](Self::enter) knows not to hand out a
    /// starter backpack.
    pub(super) fn restore_inventory(&mut self, owner: u32) -> bool {
        let Some(records) = self.pending_inventories.remove(&owner) else {
            return false;
        };
        // Pass one: the entities, so a container exists before its contents point
        // at it.
        for record in &records {
            let Some(serial) = Serial::new(record.serial) else {
                continue;
            };
            let entity = self.state.registry.spawn();
            if self.state.registry.bind_serial(entity, serial).is_err() {
                self.state.registry.despawn(entity);
                continue;
            }
            self.state.registry.insert(
                entity,
                Graphic {
                    id: record.graphic,
                    hue: record.hue,
                },
            );
            if record.amount > 1 {
                self.state.registry.insert(entity, Amount(record.amount));
            }
            if record.stackable {
                self.state.registry.insert(entity, Stackable);
            }
            if let Some(gump) = record.container_gump {
                self.state.registry.insert(entity, Container { gump });
            }
        }
        // Pass two: where each item goes.
        for record in &records {
            let Some(entity) =
                Serial::new(record.serial).and_then(|s| self.state.registry.entity_of(s))
            else {
                continue;
            };
            match record.location {
                ItemLocation::Equipped { mobile, layer } => {
                    if let Some(mobile) = Serial::new(mobile) {
                        self.state
                            .registry
                            .insert(entity, Equipped { mobile, layer });
                        // A saved mount: rebuild the ridden creature the saddle
                        // stands for and put the rider back in the saddle.
                        if layer == items::MOUNT_LAYER {
                            self.remount_saved(mobile, entity, record.graphic, record.hue);
                        }
                    }
                }
                ItemLocation::Contained {
                    container,
                    x,
                    y,
                    grid,
                } => {
                    if let Some(container) = Serial::new(container) {
                        self.state.registry.insert(
                            entity,
                            Contained {
                                container,
                                x,
                                y,
                                grid,
                            },
                        );
                    }
                }
                // An owned item is never on the ground; ignore a stray one rather
                // than drop it into the world at 0,0.
                ItemLocation::Ground { .. } => {}
            }
        }
        true
    }

    /// Rebuild a saved ride: recreate the ridden creature the mount item was
    /// drawn as, and put its rider back in the saddle, so a character that logged
    /// out mounted logs back in mounted. The creature lives only in limbo (no
    /// position) until the rider dismounts, exactly as a live mount does — its
    /// stats do not matter while ridden, so a fresh serial and the body the
    /// saddle names are all it needs.
    fn remount_saved(&mut self, rider_serial: Serial, item: EntityId, graphic: u16, hue: u16) {
        let Some(rider) = self.state.registry.entity_of(rider_serial) else {
            return;
        };
        let Some(body) = openshard_state::components::mount_body_for(graphic) else {
            return;
        };
        let Ok((mount, _)) = self.state.registry.spawn_with_serial(SerialKind::Mobile) else {
            return;
        };
        let facet = self.state.facet_of(rider);
        self.state.registry.insert(mount, Body { id: body, hue });
        self.state.registry.insert(mount, Facet(facet));
        self.state.registry.insert(mount, Ridden { rider });
        self.state.registry.insert(rider, Riding { mount, item });
    }
}
