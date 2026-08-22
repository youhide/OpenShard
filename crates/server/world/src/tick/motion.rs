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
                if let Some(Movement(walker)) = self.state.registry.get::<Movement>(entity).copied() {
                    self.state.send_packet(
                        connection,
                        &ServerPacket::WalkReject(WalkReject {
                            sequence: request.sequence.interpret(),
                            position: walker.position,
                            facing: walker.facing,
                        }),
                    );
                }
                self.notify_self(entity, "You are frozen and cannot move.");
                return;
            }
        }
        // What the step costs in stamina, and whether there is any left to pay
        // with. Checked before the walk is attempted, like ServUO's movement
        // event, so a refusal costs the sequence nothing.
        if let Some(refusal) = self.spend_step_stamina(entity, request.facing.running) {
            if let Some(Movement(walker)) = self.state.registry.get::<Movement>(entity).copied() {
                self.state.send_packet(
                    connection,
                    &ServerPacket::WalkReject(WalkReject {
                        sequence: request.sequence.interpret(),
                        position: walker.position,
                        facing: walker.facing,
                    }),
                );
            }
            self.notify_self(entity, refusal);
            return;
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
        let Some(Movement(mut walker)) = self.state.registry.get::<Movement>(entity).copied() else {
            return;
        };

        // Sitting is a placement, not a different kind of locomotion.  The
        // first directional request should therefore leave the chair in that
        // direction, rather than spending a key press merely turning the
        // seated sprite.  Keep the reservation until a step is actually
        // accepted: a wall beside the chair must not make its occupant vanish
        // from the seat while the client still draws them there.
        let leaving_seat = self.state.registry.has::<openshard_state::Seated>(entity);
        if leaving_seat {
            walker.facing = openshard_protocol::direction::Facing::walking(request.facing.direction);
        }

        let facet = self.state.facet_of(entity);
        let was = walker.position;
        let out_of_sequence = walker.sequence.is_fresh() && request.sequence != RawStepSequence(0);
        // A horse is the fastest a player legitimately moves, and the pace budget has
        // to know: charging a mounted runner the on-foot rate spends credit twice as
        // fast as it earns and rubber-bands a long gallop.
        let mounted = self
            .state
            .registry
            .has::<openshard_state::components::Riding>(entity);
        // The live terrain, not the bare map: a closed door blocks a walk the
        // statics would allow.
        let before = walker;
        let outcome = walker.request(
            request,
            &self.state.facet_state(facet).live_terrain(),
            now,
            mounted,
        );
        // `Walker::request` commits an accepted position to its private copy.
        // A body on that tile is another kind of obstruction, so restore the
        // whole walker before replying with the ordinary refusal. This keeps the
        // walk sequence and the authoritative position in lockstep for the next
        // client request.
        if matches!(outcome, Walk::Moved { position, .. } if self.state.mobile_occupies(facet, position, entity))
        {
            self.state.registry.insert(entity, Movement(before));
            self.state.send_packet(
                connection,
                &ServerPacket::WalkReject(WalkReject {
                    sequence: request.sequence.interpret(),
                    position: before.position,
                    facing: before.facing,
                }),
            );
            self.state.bus.send(StepRefused {
                entity,
                serial,
                reason: RefusedReason::Blocked,
            });
            debug!(%serial, reason = ?RefusedReason::Blocked, "step refused: mobile occupies the destination");
            return;
        }
        self.state.registry.insert(entity, Movement(walker));

        match outcome {
            Walk::Moved { position, facing } => {
                if leaving_seat {
                    self.state.registry.remove::<openshard_state::Seated>(entity);
                }
                // A step breaks concentration — ServUO's `Mobile.Move` ends with
                // `DisruptiveAction`, and a trance is over the moment you walk out
                // of it. A *turn* is not a step and does not. Hiding is spent one
                // step at a time instead of broken outright, which is what Stealth
                // buys; running or riding gives you away whatever your budget.
                self.state.disrupt(entity);
                self.state
                    .step_while_hidden(entity, request.facing.running, mounted);
                self.state.registry.insert(entity, Position(position));
                self.state.registry.insert(entity, Heading(facing));
                if !mounted {
                    combat::record_wrestling_step(&mut self.state, entity);
                }
                // The index is a second copy of the position; this is the line
                // that keeps it honest.
                self.state.facet_state_mut(facet).sectors.insert(entity, position);
                self.state.send_packet(
                    connection,
                    &ServerPacket::WalkAck(WalkAck {
                        sequence: request.sequence.interpret(),
                        notoriety: Notoriety::Innocent,
                    }),
                );
                self.state.bus.send(MobileMoved {
                    entity,
                    serial,
                    from: was,
                    to: position,
                    facing,
                });
                self.state.refresh_around(entity);
                // A ghost that just walked into a healer's reach is offered a
                // free resurrection — ServUO's `BaseHealer.OnMovement`.
                self.offer_resurrection_nearby(entity);
            }
            Walk::Turned { facing } => {
                self.state.registry.insert(entity, Heading(facing));
                self.state.send_packet(
                    connection,
                    &ServerPacket::WalkAck(WalkAck {
                        sequence: request.sequence.interpret(),
                        notoriety: Notoriety::Innocent,
                    }),
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
                self.state.send_packet(
                    connection,
                    &ServerPacket::WalkReject(WalkReject {
                        sequence: request.sequence.interpret(),
                        position: walker.position,
                        facing: walker.facing,
                    }),
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
    pub(super) fn step(&mut self, serial: Serial, direction: Direction) {
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
        let Some(Movement(mut walker)) = self.state.registry.get::<Movement>(entity).copied() else {
            return;
        };
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
        if self.state.mobile_occupies(facet, landed, entity) {
            self.state.bus.send(StepRefused {
                entity,
                serial,
                reason: RefusedReason::Blocked,
            });
            return;
        }

        let facing = Facing::walking(direction);
        walker.position = landed;
        walker.facing = facing;
        // Same as a client's walk: moving breaks concentration, whoever ordered
        // it, and spends a step of anyone's cover. A server-side step is never a
        // run, and a decree does not put anyone on a horse.
        self.state.disrupt(entity);
        self.state.step_while_hidden(entity, false, false);
        self.state.registry.insert(entity, Movement(walker));
        self.state.registry.insert(entity, Heading(facing));
        combat::record_wrestling_step(&mut self.state, entity);
        // A script may control a player as well as an NPC. `move_to` sends that
        // player's own client the 0x20 position update a client-side prediction
        // would otherwise supply, then refreshes both sides' interest sets. A
        // bare position write leaves the player camera at the old tile while
        // the server has already moved its world around them.
        self.state.move_to(entity, facet, landed);
        self.state.bus.send(MobileMoved {
            entity,
            serial,
            from: was,
            to: landed,
            facing,
        });
    }
}
