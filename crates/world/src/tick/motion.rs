use super::*;

impl World {
    pub(super) fn walk(&mut self, connection: ConnectionId, request: WalkRequest, now: Instant) {
        let Some(&entity) = self.state.players.get(&connection) else {
            // A walk before a character. Not fatal — a stray packet from a
            // client that reconnected — but nothing to act on either.
            debug!(%connection, "0x02 from a connection with no character");
            return;
        };
        let Some(serial) = self.state.registry.serial_of(entity) else {
            return;
        };
        // Paralysis refuses the walk before anything else — so it does not even
        // break a cast the player cannot then follow with a step.
        if let Some(&openshard_state::components::Frozen { until }) =
            self.state
                .registry
                .get::<openshard_state::components::Frozen>(entity)
        {
            if self.state.ticks < until {
                if let Some(Movement(walker)) = self.state.registry.get::<Movement>(entity).copied()
                {
                    self.state.send(
                        connection,
                        encode_walk_reject(request.sequence, walker.position, walker.facing),
                    );
                }
                self.notify_self(entity, "You are frozen and cannot move.");
                return;
            }
        }
        // A step breaks a spell mid-cast: the ServUO style roots the caster, so
        // stepping is choosing the walk over the spell. (The Sphere style never
        // sets `Casting`, so this is a no-op there.)
        if self
            .state
            .registry
            .remove::<openshard_state::components::Casting>(entity)
            .is_some()
        {
            self.notify_self(entity, "Your concentration is broken.");
        }
        let Some(Movement(mut walker)) = self.state.registry.get::<Movement>(entity).copied()
        else {
            return;
        };

        let facet = self.state.facet_of(entity);
        let was = walker.position;
        let out_of_sequence = walker.sequence.is_fresh() && request.sequence != 0;
        // The live terrain, not the bare map: a closed door blocks a walk the
        // statics would allow.
        let outcome = walker.request(request, &self.state.facet_state(facet).live_terrain(), now);
        self.state.registry.insert(entity, Movement(walker));

        match outcome {
            Walk::Moved { position, facing } => {
                self.state.registry.insert(entity, Position(position));
                self.state.registry.insert(entity, Heading(facing));
                // The index is a second copy of the position; this is the line
                // that keeps it honest.
                self.state
                    .facet_state_mut(facet)
                    .sectors
                    .insert(entity, position);
                self.state.send(
                    connection,
                    encode_walk_ack(request.sequence, NOTORIETY_INNOCENT),
                );
                self.state.bus.send(MobileMoved {
                    entity,
                    serial,
                    from: was,
                    to: position,
                    facing,
                });
                self.state.refresh_around(entity);
            }
            Walk::Turned { facing } => {
                self.state.registry.insert(entity, Heading(facing));
                self.state.send(
                    connection,
                    encode_walk_ack(request.sequence, NOTORIETY_INNOCENT),
                );
                self.state.bus.send(MobileTurned {
                    entity,
                    serial,
                    facing,
                });
                // A turn moves nobody, but it changes what everyone watching
                // draws — the client animates a facing it is told about.
                self.state.broadcast_move(entity);
            }
            Walk::Refused => {
                // Which of the three it was is not something `Walk` says, and
                // teaching it to would put the reasons in the wrong crate. The
                // sequence is checked before anything else, so a fresh walker
                // with a non-zero sequence can only have failed that; past it,
                // the pace and the terrain are the two left and this cannot yet
                // tell them apart. Better a coarse reason than a wrong one.
                let reason = if out_of_sequence {
                    RefusedReason::OutOfSequence
                } else {
                    RefusedReason::Blocked
                };
                self.state.send(
                    connection,
                    encode_walk_reject(request.sequence, walker.position, walker.facing),
                );
                self.state.bus.send(StepRefused {
                    entity,
                    serial,
                    reason,
                });
                debug!(%serial, ?reason, "step refused");
            }
        }
    }

    /// Move a mobile one step by server decree. See [`Command::Step`].
    ///
    /// Shares the interest-management tail with [`walk`](Self::walk) —
    /// [`refresh_around`](Self::refresh_around) and
    /// [`broadcast_move`](Self::broadcast_move) — because a mobile the server
    /// moved has to appear on the same screens, and leave the same ones, as a
    /// mobile that walked itself. What it does not share is the client half:
    /// there is no `0x22`/`0x21` ack, because there may be no client, and the
    /// mobile might be an NPC nobody is driving.
    pub(super) fn step(&mut self, serial: u32, direction: u8) {
        let Some(serial) = Serial::new(serial) else {
            return;
        };
        let Some(entity) = self.state.registry.entity_of(serial) else {
            return;
        };
        // A frozen mobile does not move — its AI, an NPC routine, or a decree alike.
        if self
            .state
            .registry
            .get::<openshard_state::components::Frozen>(entity)
            .is_some_and(|frozen| self.state.ticks < frozen.until)
        {
            return;
        }
        let Some(Movement(mut walker)) = self.state.registry.get::<Movement>(entity).copied()
        else {
            return;
        };
        let direction = Direction::from_bits(direction);
        let facet = self.state.facet_of(entity);
        let was = walker.position;

        // Turn-as-step: a mobile not yet facing this way turns and stays put.
        if walker.facing.direction != direction {
            let facing = Facing::walking(direction);
            walker.facing = facing;
            self.state.registry.insert(entity, Movement(walker));
            self.state.registry.insert(entity, Heading(facing));
            self.state.bus.send(MobileTurned {
                entity,
                serial,
                facing,
            });
            self.state.broadcast_move(entity);
            return;
        }

        let Some(target) = step_from(walker.position, direction) else {
            // Off the edge of the coordinate space — nowhere to go, and no client
            // to snap back, so it is simply refused.
            self.state.bus.send(StepRefused {
                entity,
                serial,
                reason: RefusedReason::Blocked,
            });
            return;
        };
        let landed = self
            .state
            .facet_state(facet)
            .live_terrain()
            .can_step(walker.position, target);
        let Some(landed) = landed else {
            self.state.bus.send(StepRefused {
                entity,
                serial,
                reason: RefusedReason::Blocked,
            });
            return;
        };

        let facing = Facing::walking(direction);
        walker.position = landed;
        walker.facing = facing;
        self.state.registry.insert(entity, Movement(walker));
        self.state.registry.insert(entity, Position(landed));
        self.state.registry.insert(entity, Heading(facing));
        self.state
            .facet_state_mut(facet)
            .sectors
            .insert(entity, landed);
        self.state.bus.send(MobileMoved {
            entity,
            serial,
            from: was,
            to: landed,
            facing,
        });
        self.state.refresh_around(entity);
    }
}
