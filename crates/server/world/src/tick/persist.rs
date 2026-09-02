use openshard_persistence::{
    CorpseData,
    CorpseEquipmentData,
    DoneQuestRecord,
    EffectRecord,
    ItemAffixRecord,
    PetData,
    QuestRecord,
    RestockLineRecord,
    RestockRecord,
    RunebookData,
    RunebookEntryData,
    WorldRecord,
};
use openshard_protocol::containers::GridSlot;
use openshard_protocol::identity::CharacterName;
use openshard_protocol::item_kind::{
    ItemKindId,
    MaterialId,
};
use openshard_protocol::wire::{
    Graphic,
    Hue,
    Layer,
};
use openshard_protocol::world::{
    Aggression,
    PhysicalResistance,
};
use openshard_state::components::{
    Banker,
    BehaviourBuff,
    BehaviourBuffs,
    Corpse,
    CraftedBy,
    DoneQuest,
    Escortable,
    Field,
    Frozen,
    Healer,
    ItemAffix,
    ItemAffixes,
    ItemKind,
    Material,
    Moongate,
    NightHome,
    Npc,
    Pet,
    PetOrder,
    PoisonCharges,
    Poisoned,
    Price,
    Quality,
    QuestGiver,
    QuestLog,
    QuestState,
    RangedAttack,
    Restock,
    RuneMark,
    Runebook,
    RunebookEntry,
    Skills,
    Spellbook,
    StatMod,
    StatMods,
    StockRecord,
    SwingSpeed,
    Title,
    TradeWindow,
    Trap,
    TrapKind,
    Vendor,
    body_opens_doors,
    effect,
};
use openshard_state::{
    KeyValue,
    LockKind,
    QuestKey,
    WorldTick,
    kind_from_drawn,
    presentation_of,
};

use super::*;

/// The serials [`World::restore_characters`] reserved, and the proof it ran.
///
/// Boot restores in one order that works: characters, then items. The serials
/// the characters' restore reserves are the owners the item records point at, so
/// running them the other way round files a character's pack under a serial the
/// allocator is free to hand to something else. Nothing fails, then or later —
/// the pack is simply somewhere else, and the first to notice is a player.
///
/// The order used to be two doc comments and the order of two lines in
/// `run_shard`. It is a signature now: only `restore_characters` can build one of
/// these, and `restore_items` will not compile without it. The set is real rather
/// than a marker — it is exactly the serials whose ownership the second restore
/// depends on, and it is what lets that restore tell a player's pack from an
/// NPC's gear.
#[derive(Debug)]
pub struct RestoredCharacters {
    /// Every serial the restore spoke for. Private: what a caller may do with
    /// this value is hand it to `restore_items`.
    reserved: HashSet<Serial>,
}

impl RestoredCharacters {
    /// How many characters came back from the store.
    pub fn count(&self) -> usize {
        self.reserved.len()
    }
}

/// The inventories [`World::restore_items`] filed under a mobile, and the proof
/// it ran.
///
/// The same shape as [`RestoredCharacters`], one step further along the boot
/// order: a mobile is equipped out of the inventory the items' restore filed
/// under its serial, so running the mobiles first leaves every NPC and every
/// vendor naked — and, again, nothing fails. The items are in the world, filed
/// under a serial nothing will ever ask for, and the first to notice is a player
/// buying from a vendor with no stock.
///
/// So `restore_mobiles` takes one of these and only `restore_items` can build
/// one. The set is what the second restore actually depends on: the owners that
/// are not characters, which is to say the mobiles that have gear waiting. It is
/// read for the boot log — a filed inventory no restored mobile claims is the
/// failure this order exists to prevent, and counting it is the only place at
/// boot that would say so.
#[derive(Debug)]
pub struct RestoredItems {
    /// Owners whose inventory was filed and who are not among the restored
    /// characters — every one of them a mobile, since an item's owner is the
    /// mobile at the top of whatever it is nested in. Private: what a caller may
    /// do with this value is hand it to `restore_mobiles`.
    mobile_owners: HashSet<Serial>,
}

struct RestoringMobile {
    record:      MobileRecord,
    entity:      EntityId,
    serial:      Serial,
    facet:       Facet,
    position:    Point,
    facing:      Facing,
    boot_ticks:  WorldTick,
    first_think: WorldTick,
    first_beat:  WorldTick,
}

/// Narrow a live contained-item point to the persistence representation.
///
/// A client drop enters this state from unsigned 16-bit wire fields and restore
/// widens the same stored fields back to [`GumpPoint`]. Internal placements must
/// preserve that invariant too; failure here is a corrupted component, not a
/// coordinate whose meaning is "zero".
fn persisted_container_position(position: GumpPoint) -> (u16, u16) {
    let x = u16::try_from(position.x).expect("contained-item gump x must fit the persisted u16 field");
    let y = u16::try_from(position.y).expect("contained-item gump y must fit the persisted u16 field");
    (x, y)
}

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
        changed.extend(self.state.bus.read(&mut self.entered).map(|event| event.entity));
        changed.extend(self.state.bus.read(&mut self.moved).map(|event| event.entity));
        changed.extend(self.state.bus.read(&mut self.turned).map(|event| event.entity));
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

    /// End every secure trade in progress, returning both sides' offerings.
    ///
    /// The shutdown path calls this before its final snapshot: an escrow is not
    /// saved, so goods left in one when the sweep runs would be lost. Outside
    /// shutdown a trade ends by itself — see `items::validate_trades`.
    pub fn cancel_all_trades(&mut self) {
        items::cancel_all_trades(&mut self.state);
    }

    /// Say one line to every player in the world.
    ///
    /// For the shard's own voice, which is why it is here beside the rest of the
    /// shutdown path rather than in a chat system: it is not somebody speaking,
    /// and nothing about it is a gameplay rule. A stop that says nothing looks
    /// from the client exactly like a crash — the screen freezes and the
    /// connection dies — and every other visible thing this engine does says
    /// something.
    ///
    /// Walking `players` and not the registry: a `Client` component can outlive
    /// the connection that is playing it, and the player table is what the shard
    /// itself considers to be in the world. `system_message` is what makes the
    /// line the server's rather than a mobile's, and no new packet, era or
    /// feature question comes with it.
    ///
    /// The packets go into the outbound queue like anything else, so they reach
    /// the wire only when that queue is drained. On the shutdown path the drain
    /// is the very next thing — see `run_shard`, where the order is welded and
    /// tested.
    pub fn announce(&mut self, text: &str) {
        let players: Vec<EntityId> = self.state.players.values().copied().collect();
        for player in players {
            self.state.system_message(player, text);
        }
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
        let mut snapshot = self.journal.drain(ticks.raw(), |_| None).unwrap_or(Snapshot {
            tick:        ticks.raw(),
            schema:      SCHEMA_VERSION,
            characters:  Vec::new(),
            removed:     Vec::new(),
            inventories: Vec::new(),
            ground:      None,
            spawners:    None,
            mobiles:     None,
            decorations: None,
            regions:     None,
            guilds:      None,
            alliances:   None,
            houses:      None,
            designs:     None,
            boats:       None,
            world:       None,
        });

        // Every online character, whole: its record and its entire carried
        // inventory — worn gear, backpack, bank box and everything nested. A save is
        // a complete picture of who is here and what they hold, so nothing of value
        // depends on whether its owner happened to move this tick.
        let online: Vec<EntityId> = self.state.players.values().copied().collect();
        for entity in online {
            if let Some(record) = Self::record_of(&self.state.registry, entity, self.state.ticks) {
                let owner = record.serial;
                snapshot.characters.push(record);
                snapshot.inventories.push(Inventory {
                    owner,
                    items: self.inventory_of(entity),
                });
            }
        }

        // The whole ground, every save — decoration excluded (it has its own
        // sweep below). Dropped loot and stray items persist whether or not
        // anyone was active this tick.
        snapshot.ground = Some(self.ground_items());
        // And every spawn region with its timer, so populated areas stay populated
        // across a restart and a rare spawn's wait is not reset.
        snapshot.spawners = Some(self.spawner_records());

        // Every NPC mobile, whole, each with its carried inventory — worn gear and
        // a vendor's stock crate alike, through the same walk a character's takes.
        // The Sphere/ServUO model: the save IS the world, so a restart restores it
        // exactly and a killed creature (absent here) stays dead.
        let mobiles = self.mobile_records();
        for record in &mobiles {
            if let Some(entity) = self.state.registry.entity_of(record.serial) {
                snapshot.inventories.push(Inventory {
                    owner: record.serial,
                    items: self.inventory_of(entity),
                });
            }
        }
        snapshot.mobiles = Some(mobiles);
        // And every placed decoration, door state included.
        snapshot.decorations = Some(self.decoration_records());
        // And the named regions of every facet, and the world's own scalars —
        // the hour of the day and where the rolls got to. None of the three is a
        // thing a player changes, and all three are things a restart would silently
        // lose: no guards, no music, daylight in the dungeons, every night starting
        // over, and every roll of the previous run dealt again.
        snapshot.regions = Some(self.region_records());
        // And every guild. Replace-all like the regions, which is what makes a
        // disbanding stick — and the high-water mark rides in the world row
        // beside the clock, because the maximum id *in the table* is not the
        // maximum ever issued: a disbanded guild leaves no row.
        snapshot.guilds = Some(self.guild_records());
        snapshot.alliances = Some(self.alliance_records());
        snapshot.houses = Some(self.house_records());
        // Beside the houses and swept on the same pass, because a design that
        // survived a save its house did not would come back attached to nothing.
        snapshot.designs = Some(self.house_design_records());
        // And the ships, on the same pass and the same terms.
        snapshot.boats = Some(self.boat_records());
        snapshot.world = Some(WorldRecord {
            clock_minutes:       self.clock_minutes(),
            rng_state:           self.rng_state(),
            guild_high_water:    self.state.guilds.high_water(),
            alliance_high_water: self.state.alliances.high_water(),
        });

        // Skip only a genuinely empty save, so a quiet, empty shard queues nothing.
        let ground_empty = snapshot.ground.as_ref().is_none_or(Vec::is_empty);
        let spawners_empty = snapshot.spawners.as_ref().is_none_or(Vec::is_empty);
        let mobiles_empty = snapshot.mobiles.as_ref().is_none_or(Vec::is_empty);
        let decorations_empty = snapshot.decorations.as_ref().is_none_or(Vec::is_empty);
        let regions_empty = snapshot.regions.as_ref().is_none_or(Vec::is_empty);
        if snapshot.characters.is_empty()
            && snapshot.removed.is_empty()
            && ground_empty
            && spawners_empty
            && mobiles_empty
            && decorations_empty
            && regions_empty
        {
            return;
        }
        debug!(tick = ticks.raw(), rows = snapshot.len(), "snapshot taken");
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
        let mut records = Vec::new();
        let mut containers: Vec<Serial> = Vec::new();

        for (item, live) in registry.query::<LiveItemLocation>() {
            let LiveItemLocation::Settled(openshard_state::SettledItemLocation::Equipped(worn)) = *live
            else {
                continue;
            };
            if worn.mobile != owner {
                continue;
            }
            // A secure trade escrow is worn, but it is not the character's — it
            // is the transient half of a conversation, and saving it (with the
            // goods inside, since the walk below recurses into every container)
            // would restore an escrow to a trade that no longer exists and can
            // never be closed. The argument `ground_items` makes for a spell
            // field and a moongate. The items are safe because every path that
            // ends a trade puts them back in the two packs first, including the
            // logout that reaches this function.
            if registry.has::<TradeWindow>(item) {
                continue;
            }
            // The saddle *is* saved, on the mount layer like any worn item: it
            // carries the mount's graphic, and [`restore_inventory`] rebuilds the
            // ridden creature from it, so the rider logs back in still mounted.
            let location = ItemLocation::Equipped {
                mobile: owner,
                layer:  worn.layer.0,
            };
            if let Some(record) = Self::item_record(registry, item, Some(owner), location) {
                if record.container_gump.is_some() {
                    if let Some(serial) = registry.serial_of(item) {
                        containers.push(serial);
                    }
                }
                records.push(record);
            }
        }

        while let Some(container) = containers.pop() {
            for (item, live) in registry.query::<LiveItemLocation>() {
                let LiveItemLocation::Settled(openshard_state::SettledItemLocation::Contained(held)) = *live
                else {
                    continue;
                };
                if held.container != container {
                    continue;
                }
                // Container drops arrive as wire `u16`s and restored points
                // are widened from the same persisted type. An out-of-range
                // live point therefore means an internal producer corrupted
                // the component; writing one half as zero would turn that bug
                // into a plausible, permanently misplaced item.
                let (x, y) = persisted_container_position(held.position);
                let location = ItemLocation::Contained {
                    container,
                    x,
                    y,
                    grid: held.grid.0,
                };
                if let Some(record) = Self::item_record(registry, item, Some(owner), location) {
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
        for (item, live) in registry.query::<LiveItemLocation>() {
            let LiveItemLocation::Settled(openshard_state::SettledItemLocation::Ground {
                facet,
                position: at,
            }) = *live
            else {
                continue;
            };
            // A drawable thing on the ground: a graphic, not a mobile (which carries
            // a Body), not decoration (which `decorate:` re-lays), and not a
            // field tile (transient like a cast in flight — it does not persist, and
            // restoring one would leave an eternal static that no longer expires or
            // blocks).
            //
            // A spell's gate is excluded for exactly the same reason, and ServUO
            // deletes its own on deserialise saying so: restored, a half-minute
            // portal becomes a permanent one whose caster no longer exists. The
            // eight city moongates are `Decoration` and are already out by the
            // line above, so the two cases do not collide.
            //
            // A house is out because it is saved as a `HouseRecord` — it has a
            // graphic and a position like any item, so it was collected *as well
            // as*, and the restore, which puts houses back before items, then
            // found its own serial already spoken for. Its sign is out on the
            // same terms for a different reason: it is derived from the house and
            // rebuilt at restore, so an item copy would come back as a plaque
            // that no longer opens anything.
            if !registry.has::<Drawn>(item)
                || registry.has::<Body>(item)
                || registry.has::<Decoration>(item)
                || registry.has::<Field>(item)
                || registry.has::<Moongate>(item)
                || registry.has::<openshard_state::components::House>(item)
                || registry.has::<openshard_state::components::HouseSign>(item)
                // A ship is saved as a `BoatRecord` and restored with its berth
                // recomputed. An item copy would come back as a hull with no
                // deck under anybody standing on it — the house's own bug, which
                // this engine has already had once.
                || registry.has::<openshard_state::components::Boat>(item)
            {
                continue;
            }
            let location = ItemLocation::Ground {
                facet: facet.0,
                x:     at.x,
                y:     at.y,
                z:     at.z,
            };
            if let Some(record) = Self::item_record(registry, item, None, location) {
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
        owner: Option<Serial>,
        location: ItemLocation,
    ) -> Option<ItemRecord> {
        let serial = registry.serial_of(item)?;
        let graphic = registry.get::<Drawn>(item)?;
        let amount = if graphic.id == openshard_state::components::CORPSE_GRAPHIC {
            registry
                .get::<openshard_state::components::CorpseBody>(item)
                .map_or(1, |corpse| corpse.body.0)
        } else {
            registry.get::<Amount>(item).map_or(1, |amount| amount.0)
        };
        let container_gump = registry.get::<Container>(item).map(|c| c.gump.0);
        Some(ItemRecord {
            serial,
            owner,
            graphic: graphic.id.0,
            hue: graphic.hue.0,
            kind: registry.get::<ItemKind>(item).map(|kind| kind.0.0),
            material: registry.get::<Material>(item).map(|material| material.0.0),
            amount,
            stackable: registry.has::<Stackable>(item),
            container_gump,
            // Vendor stock carries a unit price and a label; without them a
            // restored shop would sell nameless wares for a single coin.
            price: registry.get::<Price>(item).map(|p| p.0),
            name: registry.get::<Name>(item).map(|n| n.0.clone()),
            // A spellbook carries its learned spells; without the mask a
            // restored book is a graphic that no longer opens.
            spellbook: registry.get::<Spellbook>(item).map(|b| b.0),
            // And a corpse carries how it came to be one, so a shard that restarts
            // inside the seven minutes a body lies does not hand the investigator
            // an anonymous one.
            corpse: registry.get::<Corpse>(item).map(|story| {
                CorpseData {
                    owner:       story.owner.clone(),
                    player:      story.player,
                    killer:      story.killer.clone(),
                    examined_by: story.examined_by.clone(),
                    looters:     story.looters.clone(),
                    carved:      story.carved,
                    // The half of the picture `amount` cannot carry — see
                    // `CorpseData::facing`. A corpse with no `CorpseBody` is the
                    // bodiless sack `lay_corpse` lays, which faces nowhere.
                    facing:      registry
                        .get::<openshard_state::components::CorpseBody>(item)
                        .map_or(0, |corpse| corpse.facing.to_bits()),
                    equipment:   story
                        .equipment
                        .iter()
                        .map(|item| {
                            CorpseEquipmentData {
                                item:  item.item,
                                layer: item.layer.0,
                            }
                        })
                        .collect(),
                }
            }),
            // And the poison on it, bottled or smeared: all four potions are the
            // same graphic, so an unsaved bottle comes back empty.
            poison: registry
                .get::<PoisonCharges>(item)
                .map(|poison| (poison.level.get(), poison.charges)),
            // And the trap on it, so a restart does not quietly disarm every chest
            // on the shard.
            trap: registry.get::<Trap>(item).map(|trap| {
                openshard_persistence::record::TrapRecord {
                    kind:  trap_kind_code(trap.kind),
                    power: trap.power,
                    level: trap.level,
                }
            }),
            // And how much is left in a thing that wears out — a tool's swings or
            // an instrument's tunes. One field for both, as they are one interface
            // in ServUO; without it a half-played lute comes back full.
            uses: items::uses_left(registry, item),
            // And whose work it is, if it is anybody's. Without this every
            // exceptional piece on the shard quietly becomes ordinary at the
            // next restart — the `Murders` bug, over property somebody spent an
            // hour earning.
            crafted: registry
                .get::<Quality>(item)
                .map(|quality| quality.exceptional)
                .or_else(|| registry.has::<CraftedBy>(item).then_some(false))
                .map(|fine| (fine, registry.get::<CraftedBy>(item).map(|maker| maker.0.clone()))),
            // And where a rune points, which is the whole of what a rune is —
            // an unsaved one comes back a blank, and the walk that marked it was
            // for nothing.
            rune: registry.get::<RuneMark>(item).map(|mark| {
                (
                    // `.0` at the record seam: a saved facet is a SQL column.
                    mark.facet.0,
                    mark.destination.x,
                    mark.destination.y,
                    mark.destination.z,
                )
            }),
            // And a runebook's whole contents, for the same reason over sixteen
            // times the work.
            runebook: registry.get::<Runebook>(item).map(|book| {
                RunebookData {
                    entries:       book
                        .entries
                        .iter()
                        .map(|entry| {
                            RunebookEntryData {
                                facet:       entry.facet.0,
                                x:           entry.destination.x,
                                y:           entry.destination.y,
                                z:           entry.destination.z,
                                description: entry.description.clone(),
                            }
                        })
                        .collect(),
                    charges:       book.charges,
                    max_charges:   book.max_charges,
                    default_entry: book.default_entry,
                }
            }),
            // And the house it is pinned in, if any. A `Standing` becomes its
            // hand-written code rather than its discriminant — see
            // `Standing::code`, and the `PetData` order it copies.
            locked_down: registry
                .get::<openshard_state::components::LockedDown>(item)
                .map(|pinned| {
                    openshard_persistence::record::LockdownData {
                        house:  pinned.house,
                        secure: pinned.secure.map(|access| access.code()),
                    }
                }),
            // And which installed addon it is a tile of, if it is one. Without it
            // a restart turns an oven back into two unrelated locked-down
            // graphics, and releasing one leaves the other half standing.
            addon: registry
                .get::<openshard_state::components::AddonPart>(item)
                .map(|part| {
                    openshard_persistence::record::AddonPartData {
                        // The deed's kind id is the addon's durable name; `.0` at
                        // the record seam, like the facet above.
                        kind: part.addon.deed_kind().0,
                        root: part.root,
                    }
                }),
            affixes: registry
                .get::<ItemAffixes>(item)
                .map(|affixes| {
                    affixes
                        .0
                        .iter()
                        .map(|affix| {
                            match *affix {
                                ItemAffix::Slayer { body, bonus_percent } => {
                                    ItemAffixRecord::Slayer { body, bonus_percent }
                                }
                                ItemAffix::DamageBonus { minimum, maximum } => {
                                    ItemAffixRecord::DamageBonus { minimum, maximum }
                                }
                                ItemAffix::HitPoison {
                                    level,
                                    chance_per_mille,
                                } => {
                                    ItemAffixRecord::HitPoison {
                                        level,
                                        chance_per_mille,
                                    }
                                }
                            }
                        })
                        .collect()
                })
                .unwrap_or_default(),
            location,
        })
    }

    /// Restore one item's typed custom properties at the persistence seam.
    fn restore_affixes(&mut self, entity: EntityId, record: &ItemRecord) {
        if record.affixes.is_empty() {
            return;
        }
        self.state.registry.insert(
            entity,
            ItemAffixes(
                record
                    .affixes
                    .iter()
                    .map(|affix| {
                        match *affix {
                            ItemAffixRecord::Slayer { body, bonus_percent } => {
                                ItemAffix::Slayer { body, bonus_percent }
                            }
                            ItemAffixRecord::DamageBonus { minimum, maximum } => {
                                ItemAffix::DamageBonus { minimum, maximum }
                            }
                            ItemAffixRecord::HitPoison {
                                level,
                                chance_per_mille,
                            } => {
                                ItemAffix::HitPoison {
                                    level,
                                    chance_per_mille,
                                }
                            }
                        }
                    })
                    .collect(),
            ),
        );
    }

    /// Put a saved item's craftsmanship back: the exceptional mark, and the name
    /// of whoever made it.
    ///
    /// One helper because both restore paths — a logged-out character's inventory
    /// and the ground sweep — want the same two components, and a second copy is
    /// the pair of hand-kept halves that lets a bank-boxed masterpiece come back
    /// ordinary while a ground one does not.
    fn restore_craftsmanship(&mut self, entity: EntityId, record: &ItemRecord) {
        let Some((exceptional, ref maker)) = record.crafted else {
            return;
        };
        if exceptional {
            self.state.registry.insert(entity, Quality { exceptional });
        }
        if let Some(maker) = maker {
            self.state.registry.insert(entity, CraftedBy(maker.clone()));
        }
    }

    /// Put a saved item's travel state back: where a rune points, and what a
    /// runebook holds — and, since they ride the same two restore paths, what
    /// house it is pinned in and which installed addon it is a tile of.
    ///
    /// One helper for the same reason [`restore_craftsmanship`] is one: both
    /// restore paths want it, and a second copy is how a rune in a bank box
    /// comes back marked while one on the floor comes back blank — a difference
    /// nothing shows until somebody casts.
    ///
    /// [`restore_craftsmanship`]: Self::restore_craftsmanship
    fn restore_travel_state(&mut self, entity: EntityId, record: &ItemRecord) {
        if let Some((facet, x, y, z)) = record.rune {
            self.state.registry.insert(
                entity,
                RuneMark {
                    facet:       Facet(facet),
                    destination: Point::new(x, y, z),
                },
            );
        }
        if let Some(book) = record.runebook.as_ref() {
            self.state.registry.insert(
                entity,
                Runebook {
                    entries:       book
                        .entries
                        .iter()
                        .map(|entry| {
                            RunebookEntry {
                                facet:       Facet(entry.facet),
                                destination: Point::new(entry.x, entry.y, entry.z),
                                description: entry.description.clone(),
                            }
                        })
                        .collect(),
                    charges:       book.charges,
                    max_charges:   book.max_charges,
                    default_entry: book.default_entry,
                    // Not saved: a couple of seconds' cooldown that a restart
                    // re-arms at zero, which errs the player's way.
                    next_use:      WorldTick::ZERO,
                },
            );
        }
        if let Some(pinned) = record.locked_down {
            self.state.set_item_lockdown(
                entity,
                Some(openshard_state::components::LockedDown {
                    house:  pinned.house,
                    // A code this engine did not write reads as a plain lockdown
                    // rather than as a secure open to anybody: the item stays
                    // pinned, which is the recoverable direction, and a container
                    // whose access level is unreadable is one nobody opens until
                    // a co-owner sets it again.
                    secure: pinned
                        .secure
                        .and_then(openshard_state::components::Standing::from_code),
                }),
            );
        }
        // The addon grouping goes back on *after* the lockdown: clearing a
        // lockdown drops the grouping by design, so the two must be restored in
        // this order. An addon kind this build does not know reads as no group at
        // all — the components stay where they stand, which is the recoverable
        // direction, and none of them claims to belong to something unnameable.
        if let Some(part) = record.addon {
            if let Some(addon) = openshard_state::components::AddonKind::from_deed_kind(ItemKindId(part.kind))
            {
                self.state.registry.insert(
                    entity,
                    openshard_state::components::AddonPart {
                        addon,
                        root: part.root,
                    },
                );
            }
        }
    }

    /// Every NPC mobile — townsperson, vendor, creature — as a saveable record:
    /// the Sphere/ServUO whole-world sweep. Players are excluded (they are
    /// [`CharacterRecord`]s), and so is a ridden mount in limbo — it has no
    /// position, and its ride persists through the saddle item instead.
    pub(super) fn mobile_records(&self) -> Vec<MobileRecord> {
        let registry = &self.state.registry;
        let mut records = Vec::new();
        for (entity, body) in registry.query::<Body>() {
            if registry.has::<Client>(entity) {
                continue;
            }
            let Some(&Position(at)) = registry.get::<Position>(entity) else {
                continue;
            };
            let Some(serial) = registry.serial_of(entity) else {
                continue;
            };
            let hits = registry.get::<Hitpoints>(entity).copied().unwrap_or(Hitpoints {
                current: 1,
                max:     1,
            });
            // No brain reads back as the values `spawn` builds no brain from.
            let (sight, aggression, beat, wander) = registry
                .get::<Brain>(entity)
                .map_or((Sight(0), Aggression::Aggressive, 0, false), |brain| {
                    (brain.sight, brain.aggression, brain.beat_ticks, brain.wander)
                });
            let (ranged, ranged_kind) = registry
                .get::<RangedAttack>(entity)
                .map_or((None, DamageType::Physical), |ranged| {
                    (Some(ranged.range), ranged.kind)
                });
            let npc = registry.get::<Npc>(entity).copied();
            records.push(MobileRecord {
                serial,
                body: body.id.0,
                hue: body.hue.0,
                facet: self.state.facet_of(entity).0,
                x: at.x,
                y: at.y,
                z: at.z,
                facing: registry
                    .get::<Heading>(entity)
                    .map_or(0, |heading| heading.0.to_bits()),
                name: registry.get::<Name>(entity).map(|n| n.0.clone()),
                hits_current: hits.current,
                hits_max: hits.max,
                notoriety: registry
                    .get::<Notoriety>(entity)
                    .copied()
                    .unwrap_or(Notoriety::Neutral),
                damage: registry.get::<MeleeDamage>(entity).map_or(0, |d| d.amount),
                resistance: registry
                    .get::<Resistance>(entity)
                    .map_or(PhysicalResistance::default(), |r| {
                        PhysicalResistance::new(r.physical)
                    }),
                swing: registry.get::<SwingSpeed>(entity).map_or(0, |s| s.ticks),
                sight,
                aggression,
                beat,
                ranged,
                ranged_kind,
                wander,
                banker: registry.has::<Banker>(entity),
                vendor: registry.has::<Vendor>(entity),
                healer: registry.has::<Healer>(entity),
                scarecrow: registry.has::<openshard_state::components::Scarecrow>(entity),
                title: registry.get::<Title>(entity).map(|t| t.0.clone()),
                npc_home: npc.map(|n| (n.home.x, n.home.y, n.home.z)),
                npc_wander: npc.map_or(0, |n| n.wander),
                night_home: registry.get::<NightHome>(entity).map(|h| (h.0.x, h.0.y, h.0.z)),
                // A tamed creature is property: a restart that quietly released
                // every pet on the shard would be the `Murders` lesson again.
                pet: registry.get::<Pet>(entity).map(|pet| {
                    PetData {
                        owner:        pet.owner,
                        slots:        pet.slots,
                        order:        pet_order_code(pet.order),
                        order_target: pet.order_target,
                    }
                }),
                restock: registry.get::<Restock>(entity).map(|shelf| {
                    RestockRecord {
                        // Seconds, not the tick: a tick counter restarts at boot, so a
                        // saved tick comes back either already due or an hour early.
                        in_seconds:  shelf.at.saturating_sub(self.state.ticks) / TICKS_PER_SECOND,
                        lines:       shelf
                            .lines
                            .iter()
                            .map(|l| (l.graphic.0, l.hue.0, l.amount.0, l.price.0, l.name.clone()))
                            .collect(),
                        typed_lines: shelf
                            .lines
                            .iter()
                            .map(|line| {
                                RestockLineRecord {
                                    graphic:   line.graphic.0,
                                    hue:       line.hue.0,
                                    item_kind: line.item_kind.map(|kind| kind.0),
                                    material:  line.material.map(|material| material.0),
                                    amount:    line.amount.0,
                                    price:     line.price.0,
                                    name:      line.name.clone(),
                                }
                            })
                            .collect(),
                    }
                }),
                // `SpawnedBy` is an index into the spawner list (0, 1, 2, ...),
                // not a wire serial — its own namespace starts at zero.
                spawned_by: registry.get::<SpawnedBy>(entity).map(|s| s.0),
                effects: Self::effects_of(registry, entity, self.state.ticks),
                skills: registry.get::<Skills>(entity).map_or_else(Vec::new, |s| {
                    s.entries().map(|(skill, value, _)| (skill.id(), value)).collect()
                }),
                quest_giver: registry.get::<QuestGiver>(entity).map_or_else(Vec::new, |giver| {
                    giver.keys.iter().map(|key| key.as_str().to_owned()).collect()
                }),
                escort_destination: registry
                    .get::<Escortable>(entity)
                    .map(|escort| escort.destination.clone()),
            });
        }
        records
    }

    /// Every placed decoration as a saveable record, door state included.
    pub(super) fn decoration_records(&self) -> Vec<DecorationRecord> {
        let registry = &self.state.registry;
        let mut records = Vec::new();
        for (entity, _) in registry.query::<Decoration>() {
            let Some(serial) = registry.serial_of(entity) else {
                continue;
            };
            let Some(&Drawn { id, hue }) = registry.get::<Drawn>(entity) else {
                continue;
            };
            let Some(&Position(at)) = registry.get::<Position>(entity) else {
                continue;
            };
            let door = registry.get::<Door>(entity).map(|door| {
                DoorState {
                    closed_graphic: door.closed.0,
                    open_graphic:   door.open.0,
                    offset_x:       door.offset_x,
                    offset_y:       door.offset_y,
                    link:           door.link,
                    is_open:        door.is_open,
                }
            });
            let lock = registry.get::<openshard_state::components::Lock>(entity);
            let (locked, key_value) = match lock.map(|lock| lock.kind) {
                None => (false, 0),
                Some(LockKind::Key(key)) => (true, key.raw()),
                Some(LockKind::Unopenable) => (true, 0),
            };
            records.push(DecorationRecord {
                serial,
                graphic: id.0,
                hue: hue.0,
                facet: self.state.facet_of(entity).0,
                x: at.x,
                y: at.y,
                z: at.z,
                door,
                container_gump: registry.get::<Container>(entity).map(|c| c.gump.0),
                key_value,
                locked,
            });
        }
        records
    }

    /// What a character looks like on disk.
    ///
    /// `None` for anything that is not a character, which is not an error: the
    /// journal tracks entities and the world will hold more than people.
    /// The active effects on a mobile, as they go to disk.
    ///
    /// Poison and the stat buffs (Bless/Curse and their kin) go to disk here, so
    /// a relog carries a debuff instead of washing it off. For poison the
    /// `remaining` is the pulse count; for a timed buff it is the ticks left until
    /// it lifts, measured from `now`. A buff's `amount` is its signed stat offset.
    pub(super) fn effects_of(registry: &Registry, entity: EntityId, now: WorldTick) -> Vec<EffectRecord> {
        let mut effects = Vec::new();
        if let Some(poison) = registry.get::<Poisoned>(entity) {
            effects.push(EffectRecord {
                kind:      effect::POISON,
                amount:    i16::from(poison.level.get()),
                remaining: u16::from(poison.pulses_left),
            });
        }
        if let Some(mods) = registry.get::<StatMods>(entity) {
            for m in &mods.active {
                effects.push(EffectRecord {
                    kind:      m.kind.as_u8(),
                    amount:    m.offset,
                    remaining: m.expires_at.saturating_sub(now).min(u64::from(u16::MAX)) as u16,
                });
            }
        }
        // The non-stat behaviour buffs ride the same list — kind, magnitude, and
        // the ticks left until it lifts. A Night Sight relights on login (see the
        // enter path); the rest are read straight off the restored component.
        if let Some(buffs) = registry.get::<BehaviourBuffs>(entity) {
            for b in &buffs.active {
                effects.push(EffectRecord {
                    kind:      b.kind.as_u8(),
                    amount:    b.amount,
                    remaining: b.expires_at.saturating_sub(now).min(u64::from(u16::MAX)) as u16,
                });
            }
        }
        // Paralysis rides the same list — a relog does not thaw it.
        if let Some(frozen) = registry.get::<Frozen>(entity) {
            effects.push(EffectRecord {
                kind:      effect::PARALYZE,
                amount:    0,
                remaining: frozen.until.saturating_sub(now).min(u64::from(u16::MAX)) as u16,
            });
        }
        effects
    }

    /// Put saved effects back on a restored mobile.
    ///
    /// The mirror of [`effects_of`](Self::effects_of). A stored poison becomes a
    /// live [`Poisoned`] again, its next pulse a full interval out from `now`
    /// (boot, or the tick a character logs in). A stat buff's *shift* is already
    /// folded into the saved stats and maxima, so its ledger entry is restored
    /// **without** re-applying — it only records how much to give back, and when.
    /// A tick count throughout, so a restored effect replays like decay.
    pub(super) fn apply_effects(
        registry: &mut Registry,
        entity: EntityId,
        effects: &[EffectRecord],
        now: WorldTick,
    ) {
        let mut mods = StatMods::default();
        let mut buffs = BehaviourBuffs::default();
        for record in effects {
            if record.kind == effect::POISON {
                registry.insert(
                    entity,
                    Poisoned {
                        level:       openshard_protocol::world::PoisonLevel::new(
                            record.amount.clamp(0, i16::from(u8::MAX)) as u8,
                        ),
                        next_pulse:  now + combat::POISON_INTERVAL,
                        pulses_left: record.remaining.min(u16::from(u8::MAX)) as u8,
                    },
                );
            } else if let Some(kind) = openshard_state::StatEffectKind::from_u8(record.kind) {
                mods.active.push(StatMod {
                    kind,
                    offset: record.amount,
                    expires_at: now + u64::from(record.remaining),
                });
            } else if let Some(kind) = openshard_state::BehaviourBuffKind::from_u8(record.kind) {
                // A behaviour buff nothing is folded into — just restore the
                // ledger entry with its time remaining out from `now`.
                buffs.active.push(BehaviourBuff {
                    kind,
                    amount: record.amount,
                    expires_at: now + u64::from(record.remaining),
                });
            } else if record.kind == effect::PARALYZE {
                registry.insert(
                    entity,
                    Frozen {
                        until: now + u64::from(record.remaining),
                    },
                );
            }
            // An unrecognised kind from a newer save is skipped, not a crash.
        }
        if !mods.active.is_empty() {
            registry.insert(entity, mods);
        }
        if !buffs.active.is_empty() {
            registry.insert(entity, buffs);
        }
    }

    pub(super) fn record_of(
        registry: &Registry,
        entity: EntityId,
        now: WorldTick,
    ) -> Option<CharacterRecord> {
        let serial = registry.serial_of(entity)?;
        let position = registry.get::<Position>(entity)?.0;
        let heading = registry.get::<Heading>(entity)?.0;
        let live_body = registry.get::<Body>(entity)?;
        // A ghost is saved as *living*: its `Ghost` marker remembers the body it
        // died in, and that is what goes on the row — the grey ghost body is
        // re-derived on login. Saving the ghost body instead would lose the living
        // one and leave a relogged ghost with nothing to resurrect back to.
        let dead = registry.get::<Ghost>(entity);
        let body = dead.map_or(*live_body, |g| g.body);
        let name = registry.get::<Name>(entity)?;
        // No account means this is not a player character — an NPC, say — so it
        // is not a `CharacterRecord`. Returning `None` drops it from the save,
        // which is the honest answer.
        let account = registry.get::<Account>(entity)?;
        let facet = registry.get::<Facet>(entity).map_or(DEFAULT_FACET, |f| f.0);
        let stats = registry.get::<Stats>(entity).copied().unwrap_or(Stats {
            strength:     100,
            dexterity:    100,
            intelligence: 100,
        });
        let skills = registry.get::<Skills>(entity).map_or_else(Vec::new, |s| {
            s.entries()
                .map(|(skill, value, lock)| {
                    openshard_persistence::SkillRecord {
                        id: skill.id(),
                        value,
                        lock: lock.to_bits(),
                        cap: s.cap(skill),
                    }
                })
                .collect()
        });
        // The stat arrows, and how long ago each stat last rose. The age is
        // relative on purpose: the tick counter restarts with the shard, so an
        // absolute stamp from the last run would sit in this run's future and
        // freeze the stat until the counter caught up.
        let locks = registry
            .get::<openshard_state::components::StatLocks>(entity)
            .copied()
            .unwrap_or_default();
        let last = registry
            .get::<openshard_state::components::LastStatGain>(entity)
            .copied()
            .unwrap_or_default();
        let age = |then: WorldTick| now.saturating_sub(then);
        let stat_locks = openshard_persistence::StatLockRecord {
            strength:         locks.strength.to_bits(),
            dexterity:        locks.dexterity.to_bits(),
            intelligence:     locks.intelligence.to_bits(),
            strength_age:     age(last.strength),
            dexterity_age:    age(last.dexterity),
            intelligence_age: age(last.intelligence),
        };
        Some(CharacterRecord {
            serial,
            account: account.0.clone(),
            name: CharacterName(name.0.clone()),
            body: body.id.0,
            hue: body.hue.0,
            facet,
            x: position.x,
            y: position.y,
            z: position.z,
            facing: heading.to_bits(),
            strength: stats.strength,
            dexterity: stats.dexterity,
            intelligence: stats.intelligence,
            skills,
            effects: Self::effects_of(registry, entity, now),
            dead: dead.is_some(),
            fame: registry
                .get::<openshard_state::components::Fame>(entity)
                .map_or(0, |f| f.0),
            karma: registry
                .get::<openshard_state::components::Karma>(entity)
                .map_or(0, |k| k.0),
            murders: registry
                .get::<openshard_state::components::Murders>(entity)
                .map_or(0, |m| m.0),
            quests: Self::quests_of(registry, entity),
            done_quests: Self::done_quests_of(registry, entity, now),
            // Off the component and not looked up in the guild table: a
            // membership naming a guild that has been disbanded is *already*
            // read as no membership by `guild_of`, and writing the id back
            // unchanged is what lets a guild survive being briefly unreadable.
            guild: registry
                .get::<openshard_state::components::GuildMember>(entity)
                .map(|member| member.guild.0),
            guild_title: registry
                .get::<openshard_state::components::GuildMember>(entity)
                .map_or_else(String::new, |member| member.title.clone()),
            guild_rank: registry
                .get::<openshard_state::components::GuildMember>(entity)
                .map_or(0, |member| member.rank.number()),
            guild_candidate: registry
                .get::<openshard_state::components::GuildCandidate>(entity)
                .map(|invitation| invitation.guild.0),
            stat_locks,
        })
    }

    /// The quests a character has in progress, as they go to disk.
    ///
    /// Progress is stored per objective, positionally against the
    /// definition — see [`QuestRecord`]. A timed objective's clock is written as
    /// the seconds *remaining*, never as the tick it ends on: the tick counter
    /// starts again from zero at every boot, so a saved deadline would mean a
    /// different moment on each restart. Same rule as `effects_of`.
    pub(super) fn quests_of(registry: &Registry, entity: EntityId) -> Vec<QuestRecord> {
        let Some(log) = registry.get::<QuestLog>(entity) else {
            return Vec::new();
        };
        log.active
            .iter()
            .map(|quest| {
                QuestRecord {
                    key:      quest.key.as_str().to_owned(),
                    progress: quest.progress.clone(),
                    seconds:  quest.seconds_left.clone(),
                    failed:   quest.failed,
                    giver:    quest.giver,
                }
            })
            .collect()
    }

    /// The quests a character has finished, with the wait before each may be
    /// taken again — again a remaining span rather than a deadline.
    pub(super) fn done_quests_of(
        registry: &Registry,
        entity: EntityId,
        now: WorldTick,
    ) -> Vec<DoneQuestRecord> {
        let Some(log) = registry.get::<QuestLog>(entity) else {
            return Vec::new();
        };
        log.done
            .iter()
            .map(|done| {
                DoneQuestRecord {
                    key:             done.key.as_str().to_owned(),
                    restart_in_secs: if done.restart_at == WorldTick::MAX {
                        u32::MAX // never again
                    } else {
                        let ticks = done.restart_at.saturating_sub(now);
                        u32::try_from(ticks / TICKS_PER_SECOND).unwrap_or(u32::MAX)
                    },
                }
            })
            .collect()
    }

    /// Put a character's saved quests back on them, with every clock re-based on
    /// the tick it is being restored at.
    pub(super) fn apply_quests(
        registry: &mut Registry,
        entity: EntityId,
        quests: &[QuestRecord],
        done: &[DoneQuestRecord],
        now: WorldTick,
    ) {
        if quests.is_empty() && done.is_empty() {
            return;
        }
        let log = QuestLog {
            active: quests
                .iter()
                .map(|record| {
                    QuestState {
                        key:          QuestKey::new(record.key.clone()),
                        progress:     record.progress.clone(),
                        seconds_left: record.seconds.clone(),
                        failed:       record.failed,
                        giver:        record.giver,
                    }
                })
                .collect(),
            done:   done
                .iter()
                .map(|record| {
                    DoneQuest {
                        key:        QuestKey::new(record.key.clone()),
                        restart_at: if record.restart_in_secs == u32::MAX {
                            WorldTick::MAX
                        } else {
                            now + u64::from(record.restart_in_secs) * TICKS_PER_SECOND
                        },
                    }
                })
                .collect(),
        };
        registry.insert(entity, log);
    }

    /// Reserve a serial read from persistence so a fresh spawn never takes it.
    ///
    /// A logged-out character is not in the world — it is a row in the database —
    /// but its serial is still spoken for. Call this at boot for every stored
    /// character, before anyone can create a new one. The record's `serial` field
    /// is already a checked [`Serial`] (validated on deserialisation), so there is
    /// no longer a corrupt-value case for this to swallow.
    pub fn reserve_serial(&mut self, serial: Serial) {
        self.state.registry.reserve_serial(serial);
    }

    /// Bring the saved characters back from the store at boot.
    ///
    /// Two things per row, and they are the same two the world does on every
    /// logout: reserve the serial so a character created later cannot take it,
    /// and file where the character was, so playing it puts it back there. Call
    /// once, before anyone connects.
    ///
    /// A row also says the character *exists*, and since S5 of
    /// `docs/connection_state.md` this is where that is recorded — the account's
    /// list is the roster's, not the login crate's. So this is the whole of what
    /// the store knows about who may be played, and the config's
    /// `[[accounts]] characters` is folded in beside it by
    /// [`enrol_character`](World::enrol_character) afterwards.
    ///
    /// The return value is what [`restore_items`](World::restore_items) needs and
    /// the only way to obtain it — see [`RestoredCharacters`] for why the order
    /// is a signature rather than a paragraph.
    pub fn restore_characters(&mut self, records: Vec<CharacterRecord>) -> RestoredCharacters {
        let mut reserved = HashSet::with_capacity(records.len());
        for record in records {
            self.reserve_serial(record.serial);
            reserved.insert(record.serial);
            self.roster.remember(record);
        }
        RestoredCharacters { reserved }
    }

    /// Bring saved items back from the store at boot.
    ///
    /// Reserves every item's serial so a live spawn cannot take it, places the
    /// loose ground items now, and files each carried item away by owner for
    /// [`enter`](Self::enter) to equip when that character logs in — or, when the
    /// owner is an NPC or a vendor, for [`restore_mobiles`](Self::restore_mobiles)
    /// to equip out of the same map. Call once, after the map is loaded and
    /// before anyone connects.
    ///
    /// Takes the [`RestoredCharacters`] the characters' restore hands back,
    /// because the serials that restore reserved are the owners these records
    /// point at, and returns the [`RestoredItems`] that
    /// [`restore_mobiles`](Self::restore_mobiles) needs for the same reason one
    /// step on.
    pub fn restore_items(
        &mut self,
        records: Vec<ItemRecord>,
        characters: &RestoredCharacters,
    ) -> RestoredItems {
        for record in &records {
            self.reserve_serial(record.serial);
        }
        // Split for the log and for the token: everything filed is filed the same
        // way, but the count is the one thing at boot that says whether the packs
        // found their owners — a pack under a serial no character claims is
        // invisible otherwise, which is the failure this ordering exists to
        // prevent — and the owners that are *not* characters are the mobiles
        // whose gear the next restore equips.
        let mut packs = 0usize;
        let mut ground = 0usize;
        let mut mobile_owners = HashSet::new();
        for record in records {
            match record.owner {
                None => {
                    ground += 1;
                    self.place_ground_item(&record);
                }
                Some(owner) => {
                    if characters.reserved.contains(&owner) {
                        packs += 1;
                    } else {
                        mobile_owners.insert(owner);
                    }
                    self.pending_inventories.entry(owner).or_default().push(record);
                }
            }
        }
        debug!(ground, packs, "items restored");
        RestoredItems { mobile_owners }
    }

    /// Put one restored item on the ground, bound to its saved serial.
    pub(super) fn place_ground_item(&mut self, record: &ItemRecord) {
        let ItemLocation::Ground { facet, x, y, z } = record.location else {
            return;
        };
        if !valid_saved_amount(record.graphic, record.amount) {
            warn!(serial = %record.serial, amount = record.amount, "refusing a corrupt saved item amount");
            return;
        }
        let serial = record.serial;
        let facet = if self.state.facets.contains_key(&Facet(facet)) {
            Facet(facet)
        } else {
            self.state.default_facet
        };
        let entity = self.state.registry.spawn();
        if self.state.registry.bind_serial(entity, serial).is_err() {
            self.state.registry.despawn(entity);
            return;
        }
        let position = Point::new(x, y, z);
        let drawn = Drawn {
            id:  Graphic(record.graphic),
            hue: Hue(record.hue),
        };
        self.state.registry.insert(entity, drawn);
        self.restore_item_identity(entity, record, drawn);
        establish_item_location(&mut self.state, entity, LiveItemLocation::ground(facet, position))
            .expect("a restored ground item has one valid location");
        if record.graphic == openshard_state::components::CORPSE_GRAPHIC.0 {
            self.state.registry.insert(
                entity,
                openshard_state::components::CorpseBody {
                    body:   Graphic(record.amount),
                    facing: restored_facing(record),
                },
            );
        } else if record.amount > 1 {
            self.state.registry.insert(entity, Amount(record.amount));
        }
        // Gold has always been currency, including a pile of one.  Older
        // records may have saved a lone coin without the flag, so repair it as
        // it is restored rather than preserving a permanently unstackable coin.
        if record.stackable || record.graphic == openshard_items::GOLD_GRAPHIC.0 {
            self.state.registry.insert(entity, Stackable);
        }
        if let Some(gump) = record.container_gump {
            self.state
                .registry
                .insert(entity, Container { gump: Graphic(gump) });
        }
        if let Some(price) = record.price {
            self.state.registry.insert(entity, Price(price));
        }
        if let Some(name) = &record.name {
            self.state.registry.insert(entity, Name(name.clone()));
        }
        // An addon deed's identity is its `ItemKind` (installed above by
        // `restore_item_identity`), never its display name — several deeds
        // share the same generic scroll art and only the kind tells them
        // apart.
        if let Some(&ItemKind(kind)) = self.state.registry.get::<ItemKind>(entity) {
            if let Some(deed) = openshard_state::AddonDeed::from_item_kind(kind) {
                self.state.registry.insert(entity, deed);
            }
        }
        if let Some(mask) = record.spellbook {
            self.state.registry.insert(entity, Spellbook(mask));
        }
        if let Some(story) = &record.corpse {
            self.state.registry.insert(entity, corpse_from(story));
        }
        if let Some((level, charges)) = record.poison {
            self.state.registry.insert(
                entity,
                PoisonCharges {
                    level: openshard_protocol::world::PoisonLevel::new(level),
                    charges,
                },
            );
        }
        if let Some(trap) = record.trap {
            self.state.registry.insert(
                entity,
                Trap {
                    kind:  trap_kind_from(trap.kind),
                    power: trap.power,
                    level: trap.level,
                },
            );
        }
        // The graphic says whether a saved use count is an instrument's tunes or a
        // tool's swings, so `items` decides which component it comes back as.
        if let Some(uses) = record.uses {
            items::restore_uses(&mut self.state, entity, Graphic(record.graphic), uses);
        }
        self.restore_craftsmanship(entity, record);
        self.restore_travel_state(entity, record);
        self.restore_affixes(entity, record);
        // Loose clutter resumes rotting unless cleanup is disabled. A container
        // does not (mark_decay skips it); a corpse gets its own seven-minute
        // timer when decay is enabled. The tick itself is not saved, so a
        // restored corpse counts down from the restore.
        items::mark_decay(&mut self.state, entity);
        if self.state.gameplay.decay_ticks != 0
            && Graphic(record.graphic) == openshard_state::components::CORPSE_GRAPHIC
        {
            self.state.registry.insert(
                entity,
                openshard_state::components::Decays {
                    at_tick: self.state.ticks + 7 * 60 * TICKS_PER_SECOND,
                },
            );
        }
        self.state.place_item(facet, entity, position);
    }

    /// Restore semantic identity from a post-ItemKind record, or migrate an
    /// audited legacy drawing when the record predates those fields.
    ///
    /// A stored semantic identity must project to the drawing stored beside it.
    /// On disagreement we keep the visible record but do not substitute a
    /// guessed kind: that is a corrupt/missing migration row to diagnose, not a
    /// reason to make a nearby item behave as this one.
    fn restore_item_identity(&mut self, entity: EntityId, record: &ItemRecord, drawn: Drawn) {
        let saved = record.kind.and_then(ItemKindId::new).and_then(|kind| {
            let material = match record.material {
                Some(material) => Some(MaterialId::new(material)?),
                None => None,
            };
            Some((kind, material))
        });
        let identity = match saved {
            Some((kind, material)) if presentation_of(kind, material) == Some(drawn) => {
                Some((kind, material))
            }
            Some((kind, material)) => {
                warn!(
                    serial = %record.serial,
                    kind = kind.0,
                    material = material.map(|material| material.0),
                    graphic = drawn.id.0,
                    hue = drawn.hue.0,
                    "saved item identity does not match its registry presentation"
                );
                None
            }
            None => kind_from_drawn(drawn),
        };
        if let Some((kind, material)) = identity {
            self.state.registry.insert(entity, ItemKind(kind));
            if let Some(material) = material {
                self.state.registry.insert(entity, Material(material));
            }
        }
    }

    /// Equip a logging-in character's saved inventory, if any is waiting.
    ///
    /// Two passes so nesting resolves whatever order the records are in: first
    /// spawn every item bound to its saved serial with its graphic and container
    /// mark, then place each — worn on the mobile, or inside the container its
    /// record names, now that every container entity exists. Returns whether an
    /// inventory was restored, so [`enter`](Self::enter) knows not to hand out a
    /// starter backpack.
    pub(super) fn restore_inventory(&mut self, owner: Serial) -> bool {
        let Some(records) = self.pending_inventories.remove(&owner) else {
            return false;
        };
        let mut refused: HashSet<Serial> = records
            .iter()
            .filter(|record| !valid_saved_amount(record.graphic, record.amount))
            .map(|record| record.serial)
            .collect();
        loop {
            let before = refused.len();
            for record in &records {
                if let ItemLocation::Contained { container, .. } = record.location {
                    if refused.contains(&container) {
                        refused.insert(record.serial);
                    }
                }
            }
            if refused.len() == before {
                break;
            }
        }
        for record in records.iter().filter(|record| refused.contains(&record.serial)) {
            warn!(serial = %record.serial, amount = record.amount, "refusing a corrupt saved item or its dependent subtree");
        }
        let records: Vec<_> = records
            .into_iter()
            .filter(|record| !refused.contains(&record.serial))
            .collect();
        // Only entities this restore actually bound may be placed in pass two.
        // Looking a serial up in the whole registry there would mistake a stale
        // or otherwise colliding live entity for the one whose spawn failed.
        let mut restored = HashMap::with_capacity(records.len());
        // Pass one: the entities, so a container exists before its contents point
        // at it.
        for record in &records {
            let serial = record.serial;
            let entity = self.state.registry.spawn();
            if self.state.registry.bind_serial(entity, serial).is_err() {
                self.state.registry.despawn(entity);
                continue;
            }
            restored.insert(serial, entity);
            let drawn = Drawn {
                id:  Graphic(record.graphic),
                hue: Hue(record.hue),
            };
            self.state.registry.insert(entity, drawn);
            self.restore_item_identity(entity, record, drawn);
            if record.graphic == openshard_state::components::CORPSE_GRAPHIC.0 {
                self.state.registry.insert(
                    entity,
                    openshard_state::components::CorpseBody {
                        body:   Graphic(record.amount),
                        facing: restored_facing(record),
                    },
                );
            } else if record.amount > 1 {
                self.state.registry.insert(entity, Amount(record.amount));
            }
            // See `place_ground_item`: a historical lone gold coin still
            // restores as stackable.
            if record.stackable || record.graphic == openshard_items::GOLD_GRAPHIC.0 {
                self.state.registry.insert(entity, Stackable);
            }
            if let Some(gump) = record.container_gump {
                self.state
                    .registry
                    .insert(entity, Container { gump: Graphic(gump) });
            }
            if let Some(price) = record.price {
                self.state.registry.insert(entity, Price(price));
            }
            if let Some(name) = &record.name {
                self.state.registry.insert(entity, Name(name.clone()));
            }
            // See the ground-item restore above: identity is `ItemKind`, not
            // the display name.
            if let Some(&ItemKind(kind)) = self.state.registry.get::<ItemKind>(entity) {
                if let Some(deed) = openshard_state::AddonDeed::from_item_kind(kind) {
                    self.state.registry.insert(entity, deed);
                }
            }
            if let Some(mask) = record.spellbook {
                self.state.registry.insert(entity, Spellbook(mask));
            }
            if let Some(story) = &record.corpse {
                self.state.registry.insert(entity, corpse_from(story));
            }
            if let Some((level, charges)) = record.poison {
                self.state.registry.insert(
                    entity,
                    PoisonCharges {
                        level: openshard_protocol::world::PoisonLevel::new(level),
                        charges,
                    },
                );
            }
            if let Some(trap) = record.trap {
                self.state.registry.insert(
                    entity,
                    Trap {
                        kind:  trap_kind_from(trap.kind),
                        power: trap.power,
                        level: trap.level,
                    },
                );
            }
            if let Some(uses) = record.uses {
                items::restore_uses(&mut self.state, entity, Graphic(record.graphic), uses);
            }
            self.restore_craftsmanship(entity, record);
            self.restore_travel_state(entity, record);
            self.restore_affixes(entity, record);
        }
        // Pass two: where each item goes.
        for record in &records {
            // `remove` also makes a duplicate input record harmless: one serial
            // describes one entity and receives one canonical ownership edge.
            let Some(entity) = restored.remove(&record.serial) else {
                continue;
            };
            match record.location {
                ItemLocation::Equipped { mobile, layer } => {
                    let equipped = Equipped {
                        mobile,
                        layer: Layer(layer),
                    };
                    establish_item_location(&mut self.state, entity, LiveItemLocation::equipped(equipped))
                        .expect("restored worn item has one valid wearer");
                    // A saved mount: rebuild the ridden creature the saddle
                    // stands for and put the rider back in the saddle.
                    if Layer(layer) == items::MOUNT_LAYER {
                        self.remount_saved(mobile, entity, Graphic(record.graphic), Hue(record.hue));
                    }
                }
                ItemLocation::Contained {
                    container,
                    x,
                    y,
                    grid,
                } => {
                    let contained = Contained {
                        container,
                        position: GumpPoint::new(i32::from(x), i32::from(y)),
                        grid: GridSlot(grid),
                    };
                    establish_item_location(&mut self.state, entity, LiveItemLocation::contained(contained))
                        .expect("restored contained item has one valid container");
                }
                // An owned item is never on the ground; ignore a stray one rather
                // than drop it into the world at 0,0.
                ItemLocation::Ground { .. } => {}
            }
        }
        true
    }

    /// Bring the world's NPC mobiles back from the store at boot — each exactly
    /// as saved, wounded or well, at the tile it stood on, with its gear and a
    /// vendor's stock equipped from the already-restored item records. Runs after
    /// [`restore_items`](Self::restore_items), which filed each mobile's inventory
    /// under its serial — that is what the [`RestoredItems`] argument is, and it
    /// is the only way to obtain one — and before anyone connects.
    ///
    /// The component list mirrors `npc::spawn` — the record-to-component
    /// conversion is exactly the seam this module exists to hold (see
    /// `persistence::record`) — with the differences a restore wants: the saved
    /// z and facing are honoured verbatim (no `stand_z` re-drop), no
    /// `MobileSpawned` is announced (the pack stocked this vendor in its first
    /// life; the stock is in the save), and a fresh stock crate is not equipped
    /// (the saved one is restored with the rest of the inventory).
    pub fn restore_mobiles(&mut self, records: Vec<MobileRecord>, items: &RestoredItems) {
        // What the token is read for: an inventory filed under a serial no
        // restored mobile claims is gear that exists, is bound, and is reachable
        // by nobody — the shape of failure this ordering prevents, and at boot
        // this count is the only thing that would say it had happened.
        let mut equipped = 0usize;
        for record in records {
            let Some(mut mobile) = self.prepare_mobile_restore(record) else {
                continue;
            };
            self.restore_mobile_core(&mobile);
            self.restore_mobile_combat(&mobile);
            self.restore_mobile_identity(&mut mobile);
            self.restore_mobile_schedule(&mut mobile);
            self.restore_mobile_live_state(&mobile);
            self.restore_mobile_bindings(&mobile);
            if self.finish_mobile_restore(mobile) {
                equipped += 1;
            }
        }
        // `equipped` counts the mobiles that found gear; the token counts the
        // gear that was filed for one. They are equal on a healthy boot, and the
        // difference is inventories nobody came for.
        let unclaimed = items.mobile_owners.len().saturating_sub(equipped);
        debug!(equipped, unclaimed, "mobiles restored");
    }

    fn prepare_mobile_restore(&mut self, record: MobileRecord) -> Option<RestoringMobile> {
        let serial = record.serial;
        let entity = self.state.registry.spawn();
        if self.state.registry.bind_serial(entity, serial).is_err() {
            self.state.registry.despawn(entity);
            return None;
        }
        let facet = if self.state.facets.contains_key(&Facet(record.facet)) {
            Facet(record.facet)
        } else {
            self.state.default_facet
        };
        let position = Point::new(record.x, record.y, record.z);
        let facing = Facing::from_bits(record.facing);
        // A saved timer counts from the tick the world came back at. Both first
        // beats are rolled before any registry borrow, in their original order.
        let boot_ticks = self.state.ticks;
        let brain_interval = if record.beat > 0 {
            record.beat
        } else {
            self.state.gameplay.creature_step_ticks.max(1)
        };
        let first_think = openshard_npc::first_beat(&mut self.state.rng, boot_ticks, brain_interval);
        let first_beat =
            openshard_npc::first_beat(&mut self.state.rng, boot_ticks, openshard_npc::BEAT_TICKS);
        Some(RestoringMobile {
            record,
            entity,
            serial,
            facet,
            position,
            facing,
            boot_ticks,
            first_think,
            first_beat,
        })
    }

    fn restore_mobile_core(&mut self, mobile: &RestoringMobile) {
        let record = &mobile.record;
        let registry = &mut self.state.registry;
        registry.insert(
            mobile.entity,
            Body {
                id:  Graphic(record.body),
                hue: openshard_protocol::wire::Hue(record.hue),
            },
        );
        registry.insert(mobile.entity, Position(mobile.position));
        registry.insert(mobile.entity, Heading(mobile.facing));
        registry.insert(mobile.entity, mobile.facet);
        registry.insert(
            mobile.entity,
            Hitpoints {
                current: record.hits_current.max(1),
                max:     record.hits_max.max(1),
            },
        );
        registry.insert(mobile.entity, record.notoriety);
        registry.insert(
            mobile.entity,
            MeleeDamage {
                amount: record.damage,
            },
        );
    }

    fn restore_mobile_combat(&mut self, mobile: &RestoringMobile) {
        let record = &mobile.record;
        let registry = &mut self.state.registry;
        registry.insert(
            mobile.entity,
            Resistance {
                physical: record.resistance.get(),
                fire:     0,
                cold:     0,
                poison:   0,
                energy:   0,
            },
        );
        if record.swing != 0 {
            registry.insert(mobile.entity, SwingSpeed { ticks: record.swing });
        }
        if let Some(range) = record.ranged {
            registry.insert(
                mobile.entity,
                RangedAttack {
                    range,
                    kind: record.ranged_kind,
                },
            );
        }
        // The same brain rule `spawn` applies: anything that hunts, drifts,
        // or must answer or flee a blow.
        let aggression = record.aggression;
        if record.sight.0 > 0 || record.wander || aggression != Aggression::Aggressive {
            registry.insert(
                mobile.entity,
                Brain {
                    sight: record.sight,
                    wander: record.wander,
                    next_think: mobile.first_think,
                    guard_until: WorldTick::ZERO,
                    opens_doors: body_opens_doors(Graphic(record.body)),
                    aggression,
                    beat_ticks: record.beat,
                },
            );
        }
    }

    fn restore_mobile_identity(&mut self, mobile: &mut RestoringMobile) {
        let record = &mut mobile.record;
        let registry = &mut self.state.registry;
        if let Some(name) = record.name.take() {
            registry.insert(mobile.entity, Name(name));
        }
        if record.banker {
            registry.insert(mobile.entity, Banker);
        }
        if record.vendor {
            registry.insert(mobile.entity, Vendor);
        }
        if record.healer {
            registry.insert(mobile.entity, Healer);
        }
        if record.scarecrow {
            registry.insert(mobile.entity, openshard_state::components::Scarecrow);
        }
        // The trade, without which a restored NPC is a mute statue.
        if let Some(title) = record.title.take() {
            registry.insert(mobile.entity, Title(title));
        }
        if let Some(pet) = &record.pet {
            registry.insert(
                mobile.entity,
                Pet {
                    owner:        pet.owner,
                    slots:        pet.slots,
                    order:        pet_order_from(pet.order),
                    order_target: pet.order_target,
                },
            );
        }
        if let Some((x, y, z)) = record.night_home {
            registry.insert(mobile.entity, NightHome(Point::new(x, y, z)));
        }
    }

    fn restore_mobile_schedule(&mut self, mobile: &mut RestoringMobile) {
        let record = &mut mobile.record;
        let registry = &mut self.state.registry;
        if let Some(shelf) = record.restock.take() {
            registry.insert(
                mobile.entity,
                Restock {
                    at:    mobile.boot_ticks + shelf.in_seconds * TICKS_PER_SECOND,
                    lines: Self::restore_restock_lines(shelf),
                },
            );
        }
        if let Some((x, y, z)) = record.npc_home {
            registry.insert(
                mobile.entity,
                Npc {
                    home:       Point::new(x, y, z),
                    wander:     record.npc_wander,
                    next_beat:  mobile.first_beat,
                    // Eligible to greet at once; `attend` still rolls for it.
                    next_greet: WorldTick::ZERO,
                },
            );
        }
        // Re-tie it to the region that maintains it, so it is counted.
        if let Some(region) = record.spawned_by {
            registry.insert(mobile.entity, SpawnedBy(region));
        }
        registry.insert(
            mobile.entity,
            Movement(Walker::new(mobile.position, mobile.facing)),
        );
    }

    fn restore_mobile_live_state(&mut self, mobile: &RestoringMobile) {
        let record = &mobile.record;
        // A wounded creature comes back wounded; timed effects resume at boot.
        let now = self.state.ticks;
        Self::apply_effects(&mut self.state.registry, mobile.entity, &record.effects, now);
        if !record.skills.is_empty() {
            let mut sheet = Skills::default();
            for (id, value) in &record.skills {
                if let Some(skill) = openshard_state::skill::Skill::from_id(*id) {
                    sheet.set(skill, *value);
                }
            }
            self.state.registry.insert(mobile.entity, sheet);
        }
        self.state
            .place_mobile(mobile.facet, mobile.entity, mobile.position);
    }

    fn restore_mobile_bindings(&mut self, mobile: &RestoringMobile) {
        let record = &mobile.record;
        if !record.quest_giver.is_empty() {
            self.state.registry.insert(
                mobile.entity,
                QuestGiver {
                    keys: record.quest_giver.iter().cloned().map(QuestKey::new).collect(),
                },
            );
        }
        if let Some(destination) = record.escort_destination.clone() {
            self.state.registry.insert(
                mobile.entity,
                Escortable {
                    destination,
                    // An escort in progress ends with the session it ran in.
                    escorter: None,
                    last_seen: WorldTick::ZERO,
                },
            );
        }
    }

    /// Restore a vendor's remembered full shelf.
    ///
    /// Typed lines are authoritative: their display is re-projected from durable
    /// ids, so a future art alias cannot turn one stock kind into another. The old
    /// tuple remains an explicit compatibility path for snapshots that predate the
    /// additive `typed_lines` field.
    fn restore_restock_lines(shelf: RestockRecord) -> Vec<StockRecord> {
        if !shelf.typed_lines.is_empty() {
            return shelf
                .typed_lines
                .into_iter()
                .filter_map(|line| {
                    let kind = line.item_kind.map(ItemKindId);
                    let material = line.material.map(MaterialId);
                    let drawn = match kind {
                        Some(kind) => presentation_of(kind, material)?,
                        None => {
                            Drawn {
                                id:  Graphic(line.graphic),
                                hue: Hue(line.hue),
                            }
                        }
                    };
                    let legacy_identity = kind_from_drawn(drawn);
                    Some(StockRecord {
                        graphic:   drawn.id,
                        hue:       drawn.hue,
                        item_kind: kind.or_else(|| legacy_identity.map(|(kind, _)| kind)),
                        material:  material.or_else(|| legacy_identity.and_then(|(_, material)| material)),
                        amount:    Amount(line.amount),
                        price:     Price(line.price),
                        name:      line.name,
                    })
                })
                .collect();
        }
        shelf
            .lines
            .into_iter()
            .map(|(graphic, hue, amount, price, name)| {
                let drawn = Drawn {
                    id:  Graphic(graphic),
                    hue: Hue(hue),
                };
                let identity = kind_from_drawn(drawn);
                StockRecord {
                    graphic: drawn.id,
                    hue: drawn.hue,
                    item_kind: identity.map(|(kind, _)| kind),
                    material: identity.and_then(|(_, material)| material),
                    amount: Amount(amount),
                    price: Price(price),
                    name,
                }
            })
            .collect()
    }

    fn finish_mobile_restore(&mut self, mobile: RestoringMobile) -> bool {
        // Gear was filed by `restore_items` under this serial.
        let equipped = self.restore_inventory(mobile.serial);
        // Restored is deliberately distinct from a fresh `MobileSpawned`.
        self.state.bus.send(crate::events::MobileRestored {
            entity: mobile.entity,
            serial: mobile.serial,
            body:   Graphic(mobile.record.body),
            at:     mobile.position,
            // A pack binds to the post, not wherever the NPC wandered to.
            home:   mobile
                .record
                .npc_home
                .map_or(mobile.position, |(x, y, z)| Point::new(x, y, z)),
        });
        equipped
    }

    /// Re-lay the saved decoration at boot: statics, doors (their open/shut state
    /// honoured) and town containers, each re-registered with the obstruction
    /// index over its own z-span — the part [`place_ground_item`](Self::place_ground_item)
    /// never does, and why decoration cannot ride the ground-item path.
    pub fn restore_decorations(&mut self, records: Vec<DecorationRecord>) {
        for record in records {
            let serial = record.serial;
            let entity = self.state.registry.spawn();
            if self.state.registry.bind_serial(entity, serial).is_err() {
                self.state.registry.despawn(entity);
                continue;
            }
            let facet = if self.state.facets.contains_key(&Facet(record.facet)) {
                Facet(record.facet)
            } else {
                self.state.default_facet
            };
            let position = Point::new(record.x, record.y, record.z);
            self.state.registry.insert(
                entity,
                Drawn {
                    id:  Graphic(record.graphic),
                    hue: Hue(record.hue),
                },
            );
            establish_item_location(&mut self.state, entity, LiveItemLocation::ground(facet, position))
                .expect("restored decoration has one valid ground location");
            self.state.registry.insert(entity, Decoration);
            // The lock, on either kind: a door that was locked at the save comes back
            // locked, or a shard's set-piece unbars itself at every reboot.
            if record.locked || record.key_value != 0 {
                self.state.registry.insert(
                    entity,
                    openshard_state::components::Lock {
                        kind:           KeyValue::new(record.key_value)
                            .map(LockKind::Key)
                            .unwrap_or(LockKind::Unopenable),
                        required_skill: 0,
                        max_skill:      0,
                    },
                );
            }
            if let Some(gump) = record.container_gump {
                self.state
                    .registry
                    .insert(entity, Container { gump: Graphic(gump) });
            }
            match record.door {
                Some(door) => {
                    self.state.registry.insert(
                        entity,
                        Door {
                            closed:   Graphic(door.closed_graphic),
                            open:     Graphic(door.open_graphic),
                            offset_x: door.offset_x,
                            offset_y: door.offset_y,
                            link:     door.link,
                            is_open:  door.is_open,
                            close_at: WorldTick::ZERO,
                        },
                    );
                    // A shut door seals its doorway again; an open one blocks
                    // nobody until it swings shut.
                    if !door.is_open {
                        self.state.facet_state_mut(facet).block(
                            position.x,
                            position.y,
                            entity,
                            openshard_map::overlay::Cover::door(position.z, openshard_state::DOOR_HEIGHT),
                        );
                    }
                }
                None => {
                    // Plain art covers its tiledata z-span, exactly as
                    // `place_decoration` registered it the first time — through
                    // the same `Cover::of_static`, so a decoration reloaded
                    // from a save is the same thing it was before the reboot.
                    // This used to read the blocking flag itself, which was a
                    // third copy of that rule and would have been the one left
                    // behind when the platform arm landed.
                    let covers = openshard_map::overlay::Cover::of_static(
                        self.state.tiles().static_tile(record.graphic),
                    )
                    .based_at(position.z);
                    for cover in covers {
                        self.state
                            .facet_state_mut(facet)
                            .block(position.x, position.y, entity, cover);
                    }
                }
            }
            self.state.place_item(facet, entity, position);
        }
    }

    /// Rebuild a saved ride: recreate the ridden creature the mount item was
    /// drawn as, and put its rider back in the saddle, so a character that logged
    /// out mounted logs back in mounted. The creature lives only in limbo (no
    /// position) until the rider dismounts, exactly as a live mount does — its
    /// stats do not matter while ridden, so a fresh serial and the body the
    /// saddle names are all it needs.
    fn remount_saved(&mut self, rider_serial: Serial, item: EntityId, graphic: Graphic, hue: Hue) {
        let Some(rider) = self.state.registry.entity_of(rider_serial) else {
            return;
        };
        let Some(body) = openshard_protocol::mounts::mount_body_for(graphic) else {
            return;
        };
        let Ok((mount, _)) = self.state.registry.spawn_with_serial(SerialKind::Mobile) else {
            return;
        };
        let facet = self.state.facet_of(rider);
        self.state.registry.insert(mount, Body { id: body, hue });
        self.state.registry.insert(mount, facet);
        self.state.registry.insert(mount, Ridden { rider });
        self.state.registry.insert(rider, Riding { mount, item });
    }
}

fn valid_saved_amount(graphic: u16, amount: u16) -> bool {
    graphic == openshard_state::components::CORPSE_GRAPHIC.0 || openshard_items::is_valid_stack_amount(amount)
}

/// Which way a restored corpse lies. Saved with the story rather than in a column
/// of its own — see [`CorpseData::facing`], which is also where the north a save
/// written before facings were saved comes back as.
fn restored_facing(record: &ItemRecord) -> Direction {
    record
        .corpse
        .as_ref()
        .map_or(Direction::North, |story| Direction::from_bits(story.facing))
}

/// A saved corpse story back into the component. The two shapes are deliberately
/// separate types (see `persistence::record`), so one conversion, in one place.
fn corpse_from(story: &CorpseData) -> Corpse {
    Corpse {
        owner:       story.owner.clone(),
        player:      story.player,
        killer:      story.killer.clone(),
        examined_by: story.examined_by.clone(),
        looters:     story.looters.clone(),
        carved:      story.carved,
        equipment:   story
            .equipment
            .iter()
            .map(|item| {
                openshard_protocol::items::CorpseEquipmentItem {
                    item:  item.item,
                    layer: Layer(item.layer),
                }
            })
            .collect(),
    }
}

/// A trap kind as one saved byte, and back. Written out rather than derived so the
/// on-disk numbering cannot drift when the enum gains a variant — the same reason
/// the effect kinds are numbered by hand in `state::effect`.
const fn trap_kind_code(kind: TrapKind) -> u8 {
    match kind {
        TrapKind::Magic => 0,
        TrapKind::Explosion => 1,
        TrapKind::Dart => 2,
        TrapKind::Poison => 3,
    }
}

/// The inverse. An unknown code reads as a magic trap rather than dropping the
/// trap entirely: a chest that quietly stops being trapped is the failure this
/// column exists to prevent.
const fn trap_kind_from(code: u8) -> TrapKind {
    match code {
        1 => TrapKind::Explosion,
        2 => TrapKind::Dart,
        3 => TrapKind::Poison,
        _ => TrapKind::Magic,
    }
}

/// A pet's standing order as one saved byte, and back. Written out by hand for the
/// same reason the trap kinds and the effect kinds are: the on-disk numbering must
/// not drift when the enum gains a variant.
const fn pet_order_code(order: PetOrder) -> u8 {
    match order {
        PetOrder::Follow => 0,
        PetOrder::Come => 1,
        PetOrder::Stay => 2,
        PetOrder::Guard => 3,
        PetOrder::Attack => 4,
        PetOrder::Stop => 5,
    }
}

/// The inverse. An unknown code reads as "follow", the harmless order.
const fn pet_order_from(code: u8) -> PetOrder {
    match code {
        1 => PetOrder::Come,
        2 => PetOrder::Stay,
        3 => PetOrder::Guard,
        4 => PetOrder::Attack,
        5 => PetOrder::Stop,
        _ => PetOrder::Follow,
    }
}

#[cfg(test)]
mod persisted_container_position_tests {
    use super::*;

    #[test]
    fn preserves_the_entire_persisted_coordinate_domain() {
        assert_eq!(
            persisted_container_position(GumpPoint::new(0, 65_535)),
            (0, 65_535)
        );
    }

    #[test]
    #[should_panic(expected = "contained-item gump x must fit the persisted u16 field")]
    fn rejects_a_coordinate_below_the_persisted_domain() {
        persisted_container_position(GumpPoint::new(-1, 0));
    }

    #[test]
    #[should_panic(expected = "contained-item gump y must fit the persisted u16 field")]
    fn rejects_a_coordinate_above_the_persisted_domain() {
        persisted_container_position(GumpPoint::new(0, 65_536));
    }

    #[test]
    fn saved_stack_amounts_must_fit_the_live_physical_pile_domain() {
        assert!(!valid_saved_amount(0x0EED, 0));
        assert!(valid_saved_amount(0x0EED, 1));
        assert!(valid_saved_amount(0x0EED, openshard_items::MAX_STACK));
        assert!(!valid_saved_amount(0x0EED, openshard_items::MAX_STACK + 1,));
    }

    #[test]
    fn a_saved_corpse_amount_is_its_body_not_a_stack_quantity() {
        assert!(valid_saved_amount(
            openshard_state::components::CORPSE_GRAPHIC.0,
            0,
        ));
    }
}
