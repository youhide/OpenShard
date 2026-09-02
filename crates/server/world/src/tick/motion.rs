use super::*;

#[derive(Clone, Copy)]
struct PlayerWalk {
    connection:   ConnectionId,
    entity:       EntityId,
    serial:       Serial,
    facet:        Facet,
    from:         Point,
    request:      WalkRequest,
    mounted:      bool,
    leaving_seat: bool,
}

impl World {
    /// Remember the continuous crossing clients draw after an accepted step.
    ///
    /// The shard has already committed the destination tile by the time this is
    /// called. The deadline is the other half of that fact: until it arrives,
    /// the body is moving between tiles rather than standing on the new one.
    fn record_step(&mut self, entity: EntityId, running: bool, mounted: bool) {
        let hold = openshard_movement::step_hold(running, mounted);
        let milliseconds = u64::try_from(hold.as_millis()).expect("a movement hold fits in u64 milliseconds");
        self.state.registry.insert(
            entity,
            LastStep {
                finishes_at: self
                    .state
                    .ticks
                    .saturating_add(Gameplay::ticks_from_ms(milliseconds)),
            },
        );
    }

    /// The body a step from `from` toward `direction` would walk into, when the
    /// thing refusing that step is a body at all.
    ///
    /// **The same step asked of the same ground with nobody standing on it.**
    /// [`Refusal::Blocked`](openshard_movement::Refusal::Blocked) is the ground
    /// and the crowd together — `movement` is handed a
    /// [`Footing`](openshard_movement::Footing) and cannot see who is in it — so
    /// telling the two apart is one more lookup, and this is it. A `None` from
    /// `step_allowed` here is the ground saying no, and the shove has no opinion
    /// about ground: a wall does not move for ten stamina.
    ///
    /// A `Some` landing with nobody standing on it is the third case and it is
    /// real: the crowd blocks *flanks* as well as landings — the corner rule
    /// asks about both — so a diagonal squeezed between two bodies is refused by
    /// a body and has nobody in its landing to shove. ServUO does not check
    /// flanks at all, so it never meets this; here it comes back `None` and the
    /// step stays refused, which is the conservative half of the divergence.
    ///
    /// The `doors` are the mover's own reading and not
    /// [`Doors::AsTheyStand`](openshard_map::overlay::Doors::AsTheyStand): "the
    /// same step with nobody standing in it" has to be the same step in every
    /// other respect, or a ghost blocked by a body in a doorway would be told
    /// the door refused it and never offered the shove.
    fn shove_target(
        &self,
        facet: Facet,
        from: Point,
        direction: Direction,
        doors: Doors,
    ) -> Option<EntityId> {
        let landed = {
            let footing = self.state.footing(facet, doors);
            openshard_movement::step_allowed(&footing, from, direction)?
        };
        self.state.body_standing_at(facet, landed)
    }

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
        if self.frozen_refuses_walk(connection, entity, request) {
            return;
        }
        if self.stamina_refuses_walk(connection, entity, request) {
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
        // A horse is the fastest a player legitimately moves, and the pace budget has
        // to know: charging a mounted runner the on-foot rate spends credit twice as
        // fast as it earns and rubber-bands a long gallop.
        let mounted = self
            .state
            .registry
            .has::<openshard_state::components::Riding>(entity);
        let walked = PlayerWalk {
            connection,
            entity,
            serial,
            facet,
            from: walker.position,
            request,
            mounted,
            leaving_seat,
        };
        let (walker, outcome) = self.request_player_walk(walked, walker, now);
        self.state.registry.insert(entity, Movement(walker));
        self.finish_player_walk(walked, walker, outcome);
    }

    /// Apply an OpenShard turn request that is not allowed to become a step.
    pub(super) fn turn(&mut self, connection: ConnectionId, request: TurnRequest) {
        let Some(&entity) = self.state.players.get(&connection) else {
            debug!(%connection, "typed turn from a connection with no character");
            return;
        };
        let Some(serial) = self.state.registry.serial_of(entity) else {
            return;
        };
        let Some(Movement(mut walker)) = self.state.registry.get::<Movement>(entity).copied() else {
            return;
        };

        // Reuse the ordinary acknowledgement/rejection wire. The fastwalk key
        // is irrelevant, and the request remains typed until after its outcome
        // has been decided, so this conversion cannot reintroduce the race.
        let legacy = WalkRequest {
            facing:       request.facing,
            sequence:     request.sequence,
            fastwalk_key: openshard_protocol::world::RawFastwalkKey(0),
        };
        let turned = PlayerWalk {
            connection,
            entity,
            serial,
            facet: self.state.facet_of(entity),
            from: walker.position,
            request: legacy,
            mounted: self
                .state
                .registry
                .has::<openshard_state::components::Riding>(entity),
            leaving_seat: false,
        };

        let frozen = self
            .state
            .registry
            .get::<openshard_state::components::Frozen>(entity)
            .is_some_and(|frozen| self.state.ticks < frozen.until);
        if frozen {
            self.send_walk_reject(connection, legacy, walker);
            self.notify_self(entity, "You are frozen and cannot move.");
            return;
        }

        let outcome = walker.turn(request);
        self.state.registry.insert(entity, Movement(walker));
        self.finish_player_walk(turned, walker, outcome);
    }

    /// Paralysis refuses the walk before anything else — so it does not even
    /// break a cast the player cannot then follow with a step.
    fn frozen_refuses_walk(
        &mut self,
        connection: ConnectionId,
        entity: EntityId,
        request: WalkRequest,
    ) -> bool {
        let frozen = self
            .state
            .registry
            .get::<openshard_state::components::Frozen>(entity)
            .is_some_and(|frozen| self.state.ticks < frozen.until);
        if !frozen {
            return false;
        }
        self.reject_walk_at_current_position(connection, entity, request);
        self.notify_self(entity, "You are frozen and cannot move.");
        true
    }

    /// Charge the prospective step before asking the walk state machine.
    /// A refusal here costs the sequence nothing.
    fn stamina_refuses_walk(
        &mut self,
        connection: ConnectionId,
        entity: EntityId,
        request: WalkRequest,
    ) -> bool {
        let Some(refusal) = self.spend_step_stamina(entity, request.facing.running) else {
            return false;
        };
        self.reject_walk_at_current_position(connection, entity, request);
        self.notify_self(entity, refusal);
        true
    }

    fn reject_walk_at_current_position(
        &mut self,
        connection: ConnectionId,
        entity: EntityId,
        request: WalkRequest,
    ) {
        let Some(Movement(walker)) = self.state.registry.get::<Movement>(entity).copied() else {
            return;
        };
        self.send_walk_reject(connection, request, walker);
    }

    fn send_walk_reject(&mut self, connection: ConnectionId, request: WalkRequest, walker: Walker) {
        // A rejection snaps the client to this authoritative tile and ends any
        // crossing it had predicted. The movement history must make the same
        // cut or combat can mistake the corrected body for a runner.
        if let Some(&entity) = self.state.players.get(&connection) {
            self.state.registry.remove::<LastStep>(entity);
        }
        self.state.send_packet(
            connection,
            &ServerPacket::WalkReject(WalkReject {
                sequence: request.sequence.interpret(),
                position: walker.position,
                facing:   walker.facing,
            }),
        );
    }

    fn request_player_walk(
        &mut self,
        walked: PlayerWalk,
        mut walker: Walker,
        now: Instant,
    ) -> (Walker, Walk) {
        // Kept so the shove below can put the walker back the way it found it.
        // `request` writes into its own copy — the position it accepts, the
        // sequence it resets, the pace credit it spends — and none of that has
        // reached the registry yet, so a rewind here is exact and a re-ask is a
        // first ask. Charging the pace twice for one `0x02` would rubber-band a
        // player for shoving.
        let before = walker;
        // The live terrain, not the bare map: a closed door blocks a walk the
        // statics would allow — and neither is the whole ground, because
        // somebody may be standing on it.
        //
        // The crowd is built for this one step and thrown away: `reach` of 1 is
        // the eight tiles a step can reach, which is every tile
        // `steps_out_of` will ask about, flanks included. This used to be a
        // `mobile_occupies` call *after* `request` had already answered, which
        // meant restoring the walker by hand — `request` commits an accepted
        // position to its private copy — and, worse, that the rule the shard
        // stepped by was not the rule it planned by. See `footing.rs`.
        //
        // And how this walker reads the doors, which is a fact about the walker
        // rather than about the step: **a ghost walks through a shut leaf**, and
        // asked once here so the shove's re-ask below cannot answer differently.
        // See `WorldState::walking_doors`.
        let doors = self.state.walking_doors(walked.entity);
        let mut outcome = {
            let crowd = self
                .state
                .crowd_near(walked.facet, walked.entity, walker.position, 1);
            let footing = self
                .state
                .footing(walked.facet, doors)
                .among(openshard_movement::Bodies::standing(&crowd));
            walker.request(walked.request, &footing, now, walked.mounted)
        };
        // **A body in the way is not a wall.** A rested player shoves past for
        // ten stamina and a line — ServUO's `Mobile.CheckShove`, and the mirror
        // of what the stock client already predicts, so refusing here is a
        // rubber-band a player sees rather than a rule they feel.
        //
        // Asked only of a step something refused, and answered only where that
        // something turns out to be a person: `shove_target` returns `None` for
        // ground, which is every refusal that is not this one.
        if outcome == Walk::Refused(openshard_movement::Refusal::Blocked) {
            if let Some(shoved) = self.shove_target(
                walked.facet,
                before.position,
                walked.request.facing.direction,
                doors,
            ) {
                if self.state.shove(walked.entity, shoved) {
                    // The step is re-asked with **nobody** in the way, and that
                    // is ServUO's `m_Pushing`, not a shortcut: the flag is set
                    // once per `Move` and cleared once per `Move`, so walking
                    // over two overlapping bodies costs one shove rather than
                    // two. Here one step is one `Move`, so one paid shove
                    // clears the whole crowd for the length of it.
                    walker = before;
                    let footing = self.state.footing(walked.facet, doors);
                    outcome = walker.request(walked.request, &footing, now, walked.mounted);
                }
            }
        }
        (walker, outcome)
    }

    fn finish_player_walk(&mut self, walked: PlayerWalk, walker: Walker, outcome: Walk) {
        match outcome {
            Walk::Moved { position, facing } => {
                self.finish_moved_player(walked, position, facing);
            }
            Walk::Turned { facing } => {
                self.finish_turned_player(walked, facing);
            }
            Walk::Refused(refusal) => {
                self.finish_refused_player(walked, walker, refusal);
            }
        }
    }

    fn finish_moved_player(&mut self, walked: PlayerWalk, position: Point, facing: Facing) {
        if walked.leaving_seat {
            self.state
                .registry
                .remove::<openshard_state::Seated>(walked.entity);
        }
        // A step breaks concentration — ServUO's `Mobile.Move` ends with
        // `DisruptiveAction`, and a trance is over the moment you walk out
        // of it. A *turn* is not a step and does not. Hiding is spent one
        // step at a time instead of broken outright, which is what Stealth
        // buys; running or riding gives you away whatever your budget.
        self.state.disrupt(walked.entity);
        self.state
            .step_while_hidden(walked.entity, walked.request.facing.running, walked.mounted);
        self.state.registry.insert(walked.entity, Position(position));
        items::occupy_chair(&mut self.state, walked.entity);
        self.state.registry.insert(walked.entity, Heading(facing));
        self.record_step(walked.entity, walked.request.facing.running, walked.mounted);
        if !walked.mounted {
            combat::record_wrestling_step(&mut self.state, walked.entity);
        }
        // Facts of *this* step are pushed into combat before the later passes.
        combat::stepped(
            &mut self.state,
            walked.entity,
            walked.request.facing.running,
            walked.mounted,
        );
        // The index is a second copy of the position; this keeps it honest.
        self.state.place_mobile(walked.facet, walked.entity, position);
        self.send_walk_ack(walked.connection, walked.request);
        self.state.bus.send(MobileMoved {
            entity: walked.entity,
            serial: walked.serial,
            from: walked.from,
            to: position,
            facing,
        });
        self.state.refresh_around(walked.entity);
        // A ghost entering a healer's reach gets an offer after observers refresh.
        self.offer_resurrection_nearby(walked.entity);
    }

    fn finish_turned_player(&mut self, walked: PlayerWalk, facing: Facing) {
        self.state.registry.insert(walked.entity, Heading(facing));
        self.send_walk_ack(walked.connection, walked.request);
        self.state.bus.send(MobileTurned {
            entity: walked.entity,
            serial: walked.serial,
            facing,
        });
        // A turn moves nobody, but it changes what everyone watching draws.
        self.state.broadcast_move(walked.entity);
    }

    fn send_walk_ack(&mut self, connection: ConnectionId, request: WalkRequest) {
        self.state.send_packet(
            connection,
            &ServerPacket::WalkAck(WalkAck {
                sequence:  request.sequence.interpret(),
                notoriety: Notoriety::Innocent,
            }),
        );
    }

    fn finish_refused_player(
        &mut self,
        walked: PlayerWalk,
        walker: Walker,
        refusal: openshard_movement::Refusal,
    ) {
        // The map is not one-to-one: stepping off the coordinate space and
        // walking into an obstacle are both `Blocked` to observers.
        let reason = match refusal {
            openshard_movement::Refusal::OutOfSequence => RefusedReason::OutOfSequence,
            openshard_movement::Refusal::TooFast => RefusedReason::TooFast,
            openshard_movement::Refusal::OffTheMap | openshard_movement::Refusal::Blocked => {
                RefusedReason::Blocked
            }
        };
        self.send_walk_reject(walked.connection, walked.request, walker);
        self.state.bus.send(StepRefused {
            entity: walked.entity,
            serial: walked.serial,
            reason,
        });
        debug!(serial = %walked.serial, ?reason, "step refused");
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

        // **A decreed step turns and moves in the same beat.** Turn-as-step is a
        // rule about a *client's* walk: the client sends a direction, and a
        // mobile not yet facing that way is answered "you turned" so the two
        // ends stay in step over a lossy sequence. Nothing about that applies to
        // a mobile the shard is moving itself — there is no request, no
        // acknowledgement and no sequence — and applying it anyway cost a
        // creature one beat out of every direction change.
        //
        // What that bought was visible the moment a shot began turning its
        // shooter: a kiting archer, turned back at each commit, spent the next
        // beat spinning instead of retreating and never opened the gap. The
        // wander walk carries the same finding from the other end, and works
        // around it by re-using the facing it already had.
        //
        // The reference does it this way too: `BaseAI.DoMove` sets the
        // creature's direction and moves in one call, which is why a monster in
        // the original game walks a diagonal rather than pirouetting on to it.
        // A turn with no step left is still a turn — that is the `landed`
        // refusal below, which broadcasts the new facing and stays put.
        let turned = walker.facing.direction != direction;
        if turned {
            let facing = Facing::walking(direction);
            walker.facing = facing;
            self.state.registry.insert(entity, Movement(walker));
            self.state.registry.insert(entity, Heading(facing));
            self.state.bus.send(MobileTurned {
                entity,
                serial,
                facing,
            });
        }

        // `step_allowed` and not `can_step`: a diagonal may not clip the corner
        // where two blockers meet, and that half of the rule lives in
        // `steps_out_of` rather than in one landing. A mobile the shard moves is
        // held to the rule its own planner uses — `find_path` refuses to *plan*
        // a corner cut, and a creature stepping straight at its quarry used to
        // walk one. See `docs/world/evidence/2026-08-25-the-span-layer.md`'s N3.
        //
        // It answers `None` off the edge of the coordinate space too, where
        // there is nowhere to step at all: the same refusal, and there is no
        // client to snap back either way.
        //
        // And the crowd, for the same reason and on the same reach as
        // [`walk`](Self::walk): a decreed step is held to the rule a walked one
        // is. It used to be a `mobile_occupies` call below this one, which is
        // how the two ends of the same rule came to be written twice.
        //
        // And by the doors its own body reads, for the fourth time the same
        // reason: a decreed step through a shut leaf is allowed to exactly the
        // mobiles a walked one is — the dead. See `WorldState::walking_doors`.
        let doors = self.state.walking_doors(entity);
        let landed = {
            let crowd = self.state.crowd_near(facet, entity, walker.position, 1);
            let footing = self
                .state
                .footing(facet, doors)
                .among(openshard_movement::Bodies::standing(&crowd));
            openshard_movement::step_allowed(&footing, walker.position, direction)
        };
        // And the shove, for the third time the same reason: a decreed step is
        // held to the rule a walked one is, so the rule lives in one place and
        // both callers ask it. In practice this almost never fires — a shove is
        // paid for in stamina and a creature carries no [`Stamina`] pool, so
        // [`WorldState::shove`] refuses it and a creature stopped by a body goes
        // on being stopped. It is here anyway rather than as a comment saying it
        // would not matter, because the day a creature grows a pool is not a day
        // anybody will come back to this function.
        //
        // [`Stamina`]: openshard_state::components::Stamina
        let landed = landed.or_else(|| {
            let shoved = self.shove_target(facet, walker.position, direction, doors)?;
            if !self.state.shove(entity, shoved) {
                return None;
            }
            let footing = self.state.footing(facet, doors);
            openshard_movement::step_allowed(&footing, walker.position, direction)
        });
        let Some(landed) = landed else {
            // A step that turned and then found a wall is still a turn, and every
            // screen is owed it: the facing was written above, so this is the one
            // place that has to say so out loud. A step that was already facing
            // the right way and found the wall changed nothing and says nothing.
            //
            // Both halves, because `0x77` is deliberately ignored by its own
            // owner — a scripted body that turned into a wall would otherwise go
            // on being drawn facing the old way on the one screen that matters.
            // The same pairing `WorldState::face_point` makes.
            if turned {
                self.state.broadcast_move(entity);
                self.state.send_player_update(entity, walker.position);
            }
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
        // Same as a client's walk: moving breaks concentration, whoever ordered
        // it, and spends a step of anyone's cover. A server-side step is never a
        // run, and a decree does not put anyone on a horse.
        self.state.disrupt(entity);
        self.state.step_while_hidden(entity, false, false);
        self.state.registry.insert(entity, Movement(walker));
        self.state.registry.insert(entity, Heading(facing));
        combat::record_wrestling_step(&mut self.state, entity);
        // A fighter that was moved is a fighter that moved, so a decreed step
        // spoils an action exactly as a walked one does. Never a run and never
        // mounted, for the reason above it.
        combat::stepped(&mut self.state, entity, false, false);
        // A script may control a player as well as an NPC. `move_to` sends that
        // player's own client the 0x20 position update a client-side prediction
        // would otherwise supply, then refreshes both sides' interest sets. A
        // bare position write leaves the player camera at the old tile while
        // the server has already moved its world around them.
        self.state.move_to(entity, facet, landed);
        self.record_step(entity, false, false);
        self.state.bus.send(MobileMoved {
            entity,
            serial,
            from: was,
            to: landed,
            facing,
        });
    }
}
