use super::*;

impl World {
    /// Put a mobile in the world. See [`Command::SpawnMobile`].
    ///
    /// The same bundle a player is built from — a body, a position, a facing, a
    /// walker, hit points — minus the [`Client`]. That absence is the whole
    /// difference between a creature and a person; everything that draws or moves
    /// a mobile already treats "has a client" as the question, so a spawned one
    /// falls out of the machinery already there.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn spawn_mobile(&mut self, spec: SpawnMobile) -> Option<EntityId> {
        let SpawnMobile {
            body,
            hue,
            hits,
            notoriety,
            damage,
            resistance,
            swing,
            sight,
            wander,
            position,
            facet,
            name,
            banker,
            equipment,
        } = spec;
        let facet = if self.state.facets.contains_key(&facet) {
            facet
        } else {
            warn!(facet, "unloaded facet; spawning the mobile on the default");
            self.state.default_facet
        };
        // Drop the mobile onto the ground, the way a client's spawner does: the
        // pack gives x/y and a rough height, and the floor it stands on — the top
        // of the static surface there, a building's raised floor and all — is the
        // map's to say. Without this a banker sinks to the given z and reads as
        // "inside a wall".
        let position = match self
            .state
            .facet_state(facet)
            .terrain
            .as_ref()
            .and_then(|t| t.stand_z(position.x, position.y, i32::from(position.z)))
            .and_then(|z| i8::try_from(z).ok())
        {
            Some(z) => Point::new(position.x, position.y, z),
            None => position,
        };
        let (entity, serial) = match self.state.registry.spawn_with_serial(SerialKind::Mobile) {
            Ok(pair) => pair,
            Err(error) => {
                warn!(?error, "out of mobile serials; not spawning");
                return None;
            }
        };
        let hits = hits.max(1);
        let facing = Facing::walking(Direction::South);
        self.state.registry.insert(entity, Body { id: body, hue });
        self.state.registry.insert(entity, Position(position));
        self.state.registry.insert(entity, Heading(facing));
        self.state.registry.insert(entity, Facet(facet));
        self.state.registry.insert(
            entity,
            Hitpoints {
                current: hits,
                max: hits,
            },
        );
        self.state
            .registry
            .insert(entity, Notoriety::from_bits(notoriety));
        self.state
            .registry
            .insert(entity, MeleeDamage { amount: damage });
        self.state.registry.insert(
            entity,
            Resistance {
                physical: resistance.min(100),
                ..Default::default()
            },
        );
        // Zero means "derive from dexterity", so a script that does not care about
        // pace names no number and gets the wrestling formula. A non-zero value
        // pins an exact cadence — a special creature that ignores its stats.
        if swing != 0 {
            self.state
                .registry
                .insert(entity, SwingSpeed { ticks: swing });
        }
        // A brain only for a creature that needs one — something that hunts or
        // wanders. A pure prop (a shopkeeper standing still) gets none and never
        // enters `think`. `Combat` it earns when it first picks a fight.
        if sight > 0 || wander {
            self.state.registry.insert(
                entity,
                Brain {
                    sight,
                    wander,
                    next_think: 0,
                },
            );
        }
        // A banker earns a generated name and title ("Rowena the banker") when the
        // spawn did not name it, the townsperson AI base (so it greets, faces and
        // keeps near its post), and the service mark that answers "bank".
        let name = if banker && name.is_none() {
            Some(npc::banker_name(&mut self.state.rng))
        } else {
            name
        };
        if let Some(name) = name {
            self.state.registry.insert(entity, Name(name));
        }
        if banker {
            self.state.registry.insert(entity, Banker { next_greet: 0 });
            self.state.registry.insert(
                entity,
                Npc {
                    home: position,
                    wander: BANKER_WANDER,
                    next_beat: 0,
                },
            );
        }
        // Dress it before the reveal, so the clothing rides in the `0x78` that
        // draws it — a naked banker is a bug that looks like nudity.
        for (graphic, layer, item_hue) in equipment {
            items::equip_worn_item(&mut self.state, serial, graphic, item_hue, layer);
        }
        self.state
            .registry
            .insert(entity, Movement(Walker::new(position, facing)));
        self.state
            .facet_state_mut(facet)
            .sectors
            .insert(entity, position);
        self.state.reveal(entity);
        // Say who and where, so a script can take control of it: the mobile
        // counterpart of `PlayerEntered`, and how `op_control` learns a serial.
        self.state.bus.send(MobileSpawned {
            entity,
            serial,
            position,
        });
        debug!(%serial, body, "mobile spawned");
        Some(entity)
    }
}
