//! State mutation driven by the shard: folding a `WorldView` snapshot into
//! [`App::entered`], and keeping the eye on the body it names in
//! [`App::follow_player`].
//!
//! Both read the wire's own words as ground truth — a correction is trusted
//! outright, same as an ordinary update is — which is the property that
//! keeps this file apart from `ui_command.rs`: everything here answers to a
//! packet, and everything there answers to a key or a click.

use std::time::Instant;

use openshard_client_net::view::{Heard, WorldView};
use openshard_client_render::control::Follow;
use openshard_client_render::items::GroundItem;
use openshard_client_render::mobiles;
use openshard_movement::Terrain;
use openshard_protocol::items::{CORPSE_GRAPHIC, WorldItemPayload};
use openshard_protocol::localized;
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::speech::LocalizedMessage;
use openshard_protocol::wire::{Graphic, Hue};
use openshard_protocol::world::Point;
use openshard_uofiles::anim::is_ghost;
use openshard_uofiles::cliloc::{Cliloc, ClilocNumber};

use crate::app::App;
use crate::world::{MotionRenderState, advance_presentation_to, cluttered};
use crate::{clutter, crowd, link};

/// Fold one locally predicted step into the presentation that ages it.
///
/// A prediction is not merely a new tile for the static `Mobile` snapshot.
/// `Crowd` owns the step's clock, walking group and drawn position; leaving it
/// at the last acknowledged tile makes a freshly sent step wait for its round
/// trip before either its glide or its animation can start. The wire's later
/// `0x22` names this same predicted tile, so its call through this helper is a
/// no-op rather than a second step.
///
/// Equipment belongs to the authoritative mobile view, not to a predicted
/// step, so preserve its shared allocation while replacing the clocked fields.
pub(crate) fn project_motion(
    crowd: &mut crowd::Crowd,
    who: crowd::Who,
    player: &mut mobiles::Mobile,
    motion: MotionRenderState,
    war: bool,
) {
    let equipment = std::mem::take(&mut player.equipment);
    let command = match (motion.corrected, motion.transition) {
        (true, _) => crowd::CommandedMove::Snap {
            at: motion.rendered.position,
        },
        (false, Some((from, to))) => crowd::CommandedMove::Transition { from, to },
        (false, None) => crowd::CommandedMove::Standing {
            at: motion.rendered.position,
        },
    };
    *player = crowd.command(who, command, player.body, motion.rendered.facing, player.hue, war);
    player.equipment = equipment;
}

impl App {
    /// Rebuild the presentation from the same authoritative snapshot after a
    /// local item-transfer state change. The transfer is a projection, not a
    /// mutation of `WorldView`, so its source subtraction is visible now,
    /// rather than waiting for an unrelated inbound packet.
    pub(crate) fn reproject_item_drag(&mut self) {
        let Some(view) = self.world.authoritative.view.as_deref().cloned() else {
            return;
        };
        self.entered(view, None);
    }

    /// Refresh the renderer adapter from the current motion snapshot.
    ///
    /// This is also called after a frame advances: a stalled application may
    /// have several numbered transitions queued, and completing one must hand
    /// the next explicit command to Crowd without waiting for another packet.
    pub(crate) fn project_player_motion(&mut self) {
        let me = self.world.me();
        let war = self
            .world
            .authoritative
            .view
            .as_ref()
            .is_some_and(|view| view.player.war && !view.player.dead);
        project_motion(
            &mut self.world.presentation.crowd,
            me,
            &mut self.world.presentation.player,
            self.world.motion.render_state(),
            war,
        );
    }

    /// Reduce one cross-thread update at the event-loop boundary.
    pub(crate) fn on_update(&mut self, update: link::Update) -> bool {
        let now = Instant::now();
        if self.observe_stationary_soak_update(&update) {
            // `zoom-soak-freeze-server` keeps the last whole WorldView as a
            // diagnostic baseline. The socket thread still runs normally; this
            // only declines to fold server state into this client-side scene.
            return true;
        }
        advance_presentation_to(
            &mut self.world.presentation,
            &mut self.world.motion,
            &mut self.last_advance,
            now,
        );
        self.project_player_motion();
        let (trace_event, trace_detail) = match &update {
            link::Update::World { body, .. } => (
                "world",
                format!(
                    "predicted={} corrected={}",
                    crate::movement_trace::point(body.predicted.position),
                    body.corrected
                ),
            ),
            link::Update::Mutation { packet } => (
                "mutation",
                format!("packet={}", crate::movement_trace::packet_kind(packet)),
            ),
            link::Update::Movement { packet, movement } => (
                "movement",
                format!(
                    "packet={} movement={movement:?}",
                    crate::movement_trace::packet_kind(packet),
                ),
            ),
            link::Update::Prediction { body, sequence } => (
                "prediction",
                format!(
                    "sequence={sequence:?} predicted={} corrected={}",
                    crate::movement_trace::point(body.predicted.position),
                    body.corrected
                ),
            ),
            link::Update::Animation(_) => ("animation", String::new()),
            link::Update::NewAnimation(_) => ("new animation", String::new()),
            link::Update::Design(bytes) => ("design", format!("bytes={}", bytes.len())),
            link::Update::Lost(_) => ("lost", String::new()),
        };
        match update {
            link::Update::World { view, body } => {
                self.world.motion.reset(body);
                // A whole fresh view is a `0x1B`, and a `0x1B` restarts the
                // session: the tooltips it carries are a new table, so the
                // questions outstanding against the old one would never be
                // answered and would block the same objects being asked about
                // again. See `Tooltips::reset`.
                self.tooltips.reset();
                self.entered(*view, None);
            }
            link::Update::Mutation { packet } => self.apply_mutation(&packet),
            link::Update::Movement { packet, movement } => self.apply_movement(&packet, movement),
            link::Update::Prediction { body, sequence } => self.apply_prediction(body, sequence),
            link::Update::Animation(animation) => self.world.presentation.crowd.play(animation),
            link::Update::NewAnimation(animation) => self.world.presentation.crowd.play_new(animation),
            // The connection ended, for any of the reasons the shard thread
            // returns: the socket closed, a packet would not frame, the player
            // logged out. Three things happen and the reason for all three is
            // one: nothing here is a statement about the world any more.
            //
            // It used to be an `eprintln!` and a `None`, which is how a
            // disconnect came to look like a game that had gone strange — the
            // strip still said "in world", the last frame still stood, and the
            // arrows still walked because the map viewer's own arm answers to
            // "no link". Whoever adds a fourth reason to end a connection needs
            // none of that repeated: it is the one arm.
            // A designed house's picture, decoded here because here is where the
            // client's own files are: a `0xD8` carries no width or height, and
            // the box comes out of the foundation's own multi.
            link::Update::Design(bytes) => self.fold_design(&bytes),
            link::Update::Lost(reason) => {
                eprintln!("disconnected: {reason}");
                if let Some(view) = self.world.authoritative.view.as_mut() {
                    view.shard_lost(&reason);
                }
                // Windows over a world that is gone, and the presses they were
                // holding: the reconcile drops them from the view's side, and
                // these are the local halves it cannot answer for. A press a
                // pane was holding — a doll's button, a dialog's — goes with
                // its window, since `shard_lost` above empties everything the
                // reconcile reads.
                //
                // **The local windows go by predicate and not by name.** A
                // skill sheet and a status frame are open because the player
                // asked, so there is nothing in the view for the reconcile to
                // drop them by — and a list of the kinds spelled out here is a
                // list that has to be kept in step with the one in
                // `open_local_window`. Each goes with its pane, which is the
                // tree, the held control and nothing else.
                self.windows
                    .own_windows
                    .retain(|window| !window.subject.is_local());
                self.windows.hand = None;
                self.windows.world_press = None;
                self.windows.prompt = None;
                self.windows.dragging = None;
                self.tooltips.reset();
                self.world.shard = crate::world::Shard::Lost(reason);
                return false;
            }
        }
        let soon = now + crate::GLIDE_INTERVAL;
        if self.world.anyone_gliding() && self.next_tick > soon {
            self.next_tick = soon;
        }
        if let Some(trace) = self.movement_trace.as_mut() {
            trace.record_detail(trace_event, &trace_detail, &self.world, self.control.camera());
        }
        true
    }

    /// Apply a local UI mutation on the same thread that owns `WorldView`.
    pub(crate) fn apply_close_window(&mut self, target: link::CloseTarget) {
        let Some(view) = self.world.authoritative.view.as_mut() else {
            return;
        };
        match target {
            link::CloseTarget::Paperdoll(serial) => view.paperdoll_closed(serial),
            link::CloseTarget::Container(serial) => view.container_closed(serial),
            link::CloseTarget::Gump(gump_id) => view.gump_closed(gump_id),
        };
    }

    /// Apply one network mutation on the event-loop thread. This is the only
    /// place after connection setup that mutates the client-owned view.
    pub(crate) fn apply_mutation(&mut self, packet: &ServerPacket) {
        self.apply_packet(packet, None);
    }

    /// Apply one of the rare packet kinds that changes the player anchor.
    /// Ordinary world packets have no value that can call this method.
    pub(crate) fn apply_movement(&mut self, packet: &ServerPacket, movement: link::Movement) {
        self.apply_packet(packet, Some(movement));
    }

    fn apply_packet(&mut self, packet: &ServerPacket, movement: Option<link::Movement>) {
        // A vendor catalogue has no wire-level close packet: closing it is a
        // local decision, so `locally_closed` keeps the stale catalogue in the
        // last view snapshot from immediately reappearing.  Conversely every
        // fresh `0x24` from a shopkeeper is an explicit request to show it
        // again, even when its stock has not changed since the previous open.
        if let ServerPacket::OpenContainer(open) = packet {
            const SHOP_GUMP: openshard_protocol::wire::Graphic = openshard_protocol::wire::Graphic(0x0030);
            if open.gump == SHOP_GUMP {
                self.windows
                    .locally_closed
                    .remove(&crate::windows::WindowSubject::Vendor(open.container));
            }
        }
        // And the same for a dialog, for the same reason one layer over: a reply
        // button closes the window at this end (see `App::answer_gump`), so a
        // shard that means its menu to survive the click sends the `0xB0`
        // again — the admin menu does exactly that. The overlay entry is a
        // prediction that the window is gone, and a fresh `0xB0` is the shard
        // saying it is not; without this the re-drawn gump would be suppressed
        // until the view forgot it, which for a re-opened gump is never.
        if let ServerPacket::GumpDisplay(display) = packet {
            self.windows
                .locally_closed
                .remove(&crate::windows::WindowSubject::Dialog(display.gump_id));
        }
        // A `0x20` is authoritative for the locally controlled body even if a
        // caller delivers it as an ordinary mutation rather than through the
        // socket thread's `link::fold`.  In particular, combat retaliation is
        // a turn in place: without this fallback the view records its new
        // facing but `PlayerMotion` keeps projecting the old one to the crowd.
        let movement = movement.or(match packet {
            ServerPacket::PlayerUpdate(update) => Some(link::Movement::Relocation {
                confirmed: openshard_client_net::walk::Predicted {
                    position: update.position,
                    facing: update.facing,
                },
            }),
            _ => None,
        });
        // A refused lift or drop bounces the server-held item back. The cursor
        // preview is purely local, so it must follow that authoritative cancel
        // instead of leaving a ghost icon under the pointer.
        if let Some(hand) = self.windows.hand {
            let held = hand.drag().item.serial;
            let confirmed = match packet {
                ServerPacket::DragCancel(_) => true,
                // A script or another authoritative system may remove the
                // cursor item instead of landing it. That is terminal too:
                // keeping the local transaction would block every new drag.
                ServerPacket::Remove(removed) => removed.serial == held,
                ServerPacket::AddToContainer(added) => {
                    matches!(
                        hand.pending_drop(),
                        Some(crate::hand::PendingDrop::Container { .. })
                    ) && added.item.serial == held
                }
                ServerPacket::WorldItem(item) => {
                    matches!(hand.pending_drop(), Some(crate::hand::PendingDrop::Ground(_)))
                        && item.serial == held
                }
                ServerPacket::EquipUpdate(update) => {
                    matches!(
                        hand.pending_drop(),
                        Some(crate::hand::PendingDrop::Equipment { mobile, layer })
                            if update.mobile == mobile && update.layer == layer
                    ) && update.item == held
                }
                _ => false,
            };
            if confirmed {
                self.windows.hand = None;
            }
        }
        // A death is an event for the same reason: it says which corpse a body
        // becomes, and the fall it hands over is playing *now*. Taken before the
        // view is folded, because the fold is what removes the falling mobile.
        if let ServerPacket::DeathAnimation(death) = packet {
            self.world.presentation.crowd.died(death.killed, death.corpse);
        }
        // Sound and music are events, not facts in `WorldView`: a second
        // identical packet must play twice, while a view fold can correctly be
        // a no-op.  Take the listener from the same authoritative anchor the
        // camera follows, before this packet has a chance to replace it.
        self.audio
            .play_packet(packet, self.world.motion.planning_state().position);
        let Some(mut view) = self.world.authoritative.view.take() else {
            return;
        };
        // Health bars are the wire-level result of every kind of damage.  Keep
        // the previous value only long enough to turn a falling value into a
        // presentation event; the authoritative view remains the sole record
        // of the current hit points.
        let health_before = match packet {
            ServerPacket::Health(bar) => match bar.serial == view.player.serial {
                true => view.player.hits.map(|hits| (bar.serial, hits.current)),
                false => view
                    .mobiles
                    .get(&bar.serial)
                    .and_then(|mobile| mobile.hits)
                    .map(|hits| (bar.serial, hits.current)),
            },
            _ => None,
        };
        let damage = match packet {
            ServerPacket::Health(bar) => match bar.serial == view.player.serial {
                true => view.player.hits,
                false => view.mobiles.get(&bar.serial).and_then(|mobile| mobile.hits),
            }
            .and_then(|before| before.current.checked_sub(bar.vitals.current))
            .filter(|amount| *amount > 0)
            .map(|amount| (bar.serial, amount)),
            _ => None,
        };
        let previous_latest = view.journal.back().cloned();
        let localized = match packet {
            ServerPacket::LocalizedMessage(message) => {
                Some((message, self.resolve_localized_message(message)))
            }
            _ => None,
        };
        view.apply(packet);
        if let Some((serial, before)) = health_before {
            let current = match serial == view.player.serial {
                true => view.player.hits,
                false => view.mobiles.get(&serial).and_then(|mobile| mobile.hits),
            }
            .map(|hits| hits.current);
            if let Some(current) = current.filter(|current| *current != before) {
                self.world.presentation.health_changed(serial, before, current);
            }
        }
        if let Some((message, text)) = localized {
            view.localized_message(message, text);
        }
        if let Some((serial, amount)) = damage {
            // Damage over our own head is incoming and therefore red; damage
            // over any other mobile is shown in blue.
            let hue = if serial == view.player.serial {
                Hue(0x0022)
            } else {
                Hue::SKILL_CHANGED
            };
            self.world.presentation.damage(serial, amount, hue);
        }
        // `WalkAck` and `WalkReject` do not carry a position in the decoded
        // view. Their `Movement` counterpart does; every other packet leaves
        // this authoritative anchor untouched.
        if let Some(movement) = movement {
            let confirmed = movement.confirmed();
            view.player_stepped(confirmed.position, confirmed.facing);
        }
        self.world.motion.accept_network(movement);
        self.entered(*view, previous_latest);
        self.sync_target_cursor();
    }

    /// Turn a `0xC1` cliloc packet into the text the player can read in the
    /// journal.  Server feedback for tools (including a bandage awaiting a
    /// patient and a patient already at full health) uses this packet family.
    fn resolve_localized_message(&self, message: &LocalizedMessage) -> String {
        resolve_localized_message(self.resources.cliloc.as_ref(), message)
    }

    /// Apply prediction without changing authoritative server state.
    pub(crate) fn apply_prediction(
        &mut self,
        body: link::Body,
        sequence: openshard_protocol::world::StepSequence,
    ) {
        self.world.motion.accept_local(body, sequence);
        self.world.presentation.crowd.commanding(self.world.me());
        self.project_player_motion();
        // Keep the roof decision on the same predicted body *only* when the
        // map and the live item layer already agree that this is a legal step.
        // A held key pressed into a known wall still leaves `cutaway_at` where
        // it was, while a real step round a building cannot be culled by the
        // previous tile's roof threshold for the whole server round trip.
        self.advance_cutaway(false);
        self.follow_player(std::time::Duration::ZERO);
        self.assert_motion_projection();
    }

    /// Move the cutaway source to the current player prediction when that move
    /// is locally known to be possible.
    ///
    /// The same guard is used for predictions and packet folds. It prevents a
    /// roof from popping for a direction this client can already prove will hit
    /// a wall, while keeping the threshold in lockstep with a normal predicted
    /// walk — the body the cutaway exists to reveal must not be hidden by its
    /// previous tile while its step is in flight.
    fn advance_cutaway(&mut self, corrected: bool) {
        let next = self.world.motion.planning_state().position;
        if corrected {
            self.world.presentation.cutaway_at = next;
            return;
        }
        let current = self.world.presentation.cutaway_at;
        let reachable = cluttered(&self.world, &self.resources)
            .can_step(current, next)
            .is_some();
        if reachable {
            self.world.presentation.cutaway_at = next;
        }
    }

    /// Redraw from what the server has shown us.
    ///
    /// A projection of the whole [`WorldView`], rebuilt each time rather than
    /// patched: the view is the record of what arrived, and anything kept in
    /// step with it by hand would be a second record that could disagree.
    pub(crate) fn entered(&mut self, view: WorldView, previous_latest: Option<Heard>) {
        // Movement updates arrive while the player is standing still too.  The
        // route HUD depends on the item layer, not on every packet in the view;
        // invalidating it unconditionally made the same expensive plan run on
        // every update (and therefore effectively every frame) at a house.
        // Keep the cache across player and mobile-detail updates, but discard
        // terrain-derived pictures when an item was added, removed, or moved.
        let items_changed = self
            .world
            .authoritative
            .view
            .as_ref()
            .is_none_or(|old| old.items != view.items);
        let mobile_obstacles_changed = self.world.authoritative.view.as_ref().is_none_or(|old| {
            old.mobiles.len() != view.mobiles.len()
                || old.mobiles.iter().any(|(serial, old_mobile)| {
                    view.mobiles
                        .get(serial)
                        .is_none_or(|mobile| mobile.position != old_mobile.position)
                })
        });
        if items_changed || mobile_obstacles_changed {
            self.steer.clear_plan_cache();
            self.route_cache = None;
        }
        if items_changed {
            self.terrain_cache = None;
            self.occluder_cache = None;
        }
        // NPCs are routing obstacles in the client. Discard any remainder of
        // an order made before the latest mobile snapshot so it replans before
        // sending the next step.
        if mobile_obstacles_changed {
            self.steer.clear_route();
        }
        // Movement has already been applied through `PlayerMotion` at the
        // mailbox boundary.  Rebuilding world presentation must not be a
        // second writer of either confirmed or predicted state.
        // The facet is chosen at startup and `0x1B` names only its size, so a
        // shard serving a different one draws this client the wrong ground with
        // no complaint from either end. Said once, because it is a
        // misconfiguration and not an event.
        if !self.world.authoritative.facet_checked {
            self.world.authoritative.facet_checked = true;
            if u32::from(view.map.width) != self.resources.map.map().width()
                || u32::from(view.map.height) != self.resources.map.map().height()
            {
                eprintln!(
                    "the shard's facet is {}x{} and {} is {}x{}: the ground drawn is not the ground you are standing on",
                    view.map.width,
                    view.map.height,
                    self.resources.map.map().facet_name(),
                    self.resources.map.map().width(),
                    self.resources.map.map().height(),
                );
            }
        }

        // Our own body is drawn where this end *predicted* it, not where the
        // last ack put it: the step leaves the moment the player asks for it and
        // the `0x22` confirming it arrives a round trip later, so a body drawn
        // from the view stands still for the latency and then crosses its tile
        // in a hurry. See `link::Body`.
        //
        // A correction is the one thing that is not walked into: the tile it
        // puts the body back on was never crossed.
        let me = Some(view.player.serial);
        // Ours is the one body whose pace is not guessed at: we send its steps.
        // Said every update rather than once, because the serial is the shard's
        // to name and nothing here is told when it does.
        self.world.presentation.crowd.commanding(me);
        // A rollback is also the one thing that makes `steer.rs`'s idea of which
        // way this body was last sent a lie — it is a step ahead of the shard on
        // purpose, and a refusal is the shard saying that step never happened.
        // Left uncorrected, the step after a `0x21` is decided against a facing
        // nobody has: it is timed as a turn when it is a step, or as a step when
        // it is a turn, and either is a beat of the walk in the wrong place.
        if self.world.motion.corrected() {
            self.steer
                .corrected(self.world.motion.planning_state().facing.direction);
        }
        // A ghost stands with no sword drawn even if `war` is still set —
        // D9's `!InWarMode || IsDead`.  The facing and all movement endpoints
        // come only from `PlayerMotion` above.
        project_motion(
            &mut self.world.presentation.crowd,
            me,
            &mut self.world.presentation.player,
            self.world.motion.render_state(),
            view.player.war && !view.player.dead,
        );
        self.world.presentation.player.equipment =
            crowd::worn(&view.player.equipment, &self.resources.tiledata).into();
        // Sorted by serial for the same reason, and for one more: two items on
        // one tile at one height are drawn in the order they arrive here, so an
        // order that changed every frame would flicker.
        //
        // Before the cutaway guard below, and not with the other projections
        // further down, because that guard asks what this client can already see
        // in its way — and a barrel it was told about in the very packet being
        // folded in is part of that.
        let mut items: Vec<_> = view.items.iter().collect();
        items.sort_unstable_by_key(|(serial, _)| serial.raw());
        // Every house the shard has named a revision for that this client does
        // not hold the shape of. Asked here, where both halves of the comparison
        // are in hand, and once per fold rather than once per frame — the answer
        // sets `designs`, so a house asked about stops being stale the moment it
        // arrives.
        self.ask_for_stale_designs(&view);
        // Bound once outside the loop: a designed house's shape lives here
        // rather than in the view, because a `Component` is a client-file type
        // and the view is the wire. See `AuthoritativeWorld::designs`.
        let designs = &self.world.authoritative.designs;
        self.world.presentation.items.clear();
        self.world.presentation.item_serials.clear();
        self.world.presentation.corpses.clear();
        let transaction_drag = self.windows.hand.map(crate::hand::Hand::drag);
        let lifted_ground = transaction_drag
            .filter(|drag| drag.origin == crate::hand::DragOrigin::Ground)
            .map(|drag| drag.item.serial);
        for (serial, item) in items {
            if Some(*serial) == lifted_ground {
                continue;
            }
            match item.payload {
                WorldItemPayload::Corpse { body, facing } => {
                    self.world.presentation.corpses.push((
                        Some(*serial),
                        self.world.presentation.crowd.corpse(
                            Some(*serial),
                            item.position,
                            body,
                            facing,
                            item.hue,
                        ),
                    ));
                }
                WorldItemPayload::Stack(amount) => {
                    // A house is one item and a hundred statics. Expanded here,
                    // at the seam where the view becomes a draw list, so the
                    // renderer never learns what a multi is — it is handed more
                    // items and nothing else changes.
                    //
                    // Every component takes the *house's* serial, which is what
                    // makes clicking any wall pick the house: `item_serials`
                    // runs parallel to `items` and picking reads it by index.
                    match multi_pieces(
                        self.resources.multis.as_deref(),
                        designs.get(serial).map(|shape| shape.components.as_slice()),
                        item.graphic,
                        item.position,
                        item.hue,
                    ) {
                        MultiDraw::Pieces(pieces) => {
                            for piece in pieces {
                                self.world.presentation.items.push(piece);
                                self.world.presentation.item_serials.push(*serial);
                            }
                        }
                        // Drawn as nothing, and picked as nothing with it: the
                        // two lists run parallel and pushing to neither is what
                        // keeps them so. A house this client has no shape for is
                        // one it cannot show, and one unrelated static in its
                        // place is worse than an empty tile.
                        MultiDraw::Unknown => {}
                        MultiDraw::NotAMulti => {
                            self.world.presentation.items.push(GroundItem {
                                at: item.position,
                                // The shard's own graphic, not the pile art it
                                // draws as: `GroundItem::displayed` chooses that
                                // every time the list is drawn, and the base
                                // graphic is what says whether the pile is
                                // counted — see that field's doc.
                                graphic: item.graphic,
                                hue: item.hue,
                                amount,
                            });
                            self.world.presentation.item_serials.push(*serial);
                        }
                    }
                }
            }
        }
        // The authoritative lift deliberately does not echo a removal back to
        // its owner. While a ground drop is pending, replace that suppressed
        // source with its transactional destination so there is no old-place
        // flash or gap before `WorldItem` confirms it.
        if let Some((drag, crate::hand::PendingDrop::Ground(at))) = self
            .windows
            .hand
            .and_then(|hand| Some((hand.drag(), hand.pending_drop()?)))
        {
            self.world.presentation.items.push(GroundItem {
                at,
                graphic: drag.item.graphic,
                hue: drag.item.hue,
                amount: drag.item.amount,
            });
            self.world.presentation.item_serials.push(drag.item.serial);
        }
        // The same list read for a second question — not what to draw, but what
        // a step cannot go through. Rebuilt here rather than per decision: one
        // click plans a route over hundreds of tiles, and each of them would
        // otherwise rescan everything on screen. See `clutter.rs`.
        self.world.presentation.clutter = clutter::Clutter::of(
            &self.world.presentation.items,
            view.mobiles.values(),
            &self.resources.tiledata,
        );
        // The cutaway has already followed each locally valid prediction. An
        // acknowledgement repeats that answer; a correction is the one case
        // that has to replace it unconditionally.
        self.advance_cutaway(self.world.motion.corrected());
        // Sorted by serial: a `HashMap`'s order is not one, and an atlas built
        // in a different order every frame is a rebuild every frame.
        let mut others: Vec<_> = view.mobiles.iter().collect();
        others.sort_unstable_by_key(|(serial, _)| serial.raw());
        self.world.presentation.others = others
            .into_iter()
            .map(|(serial, mobile)| {
                let who = Some(*serial);
                // Their stance is a bit of the flag byte the same packet
                // carried — `view::Mobile::war` — so a shopkeeper who draws a
                // sword changes how they stand on the next `0x77` about them.
                // A ghost is drawn no sword regardless: nothing on the wire
                // says a stranger died, but their body id does — see
                // `is_ghost` — the same D9 gate the player's own body gets.
                let mut drawn = self.world.presentation.crowd.see(
                    who,
                    mobile.position,
                    mobile.body,
                    mobile.facing,
                    mobile.hue,
                    mobile.war() && !is_ghost(mobile.body),
                );
                drawn.equipment = crowd::worn(&mobile.equipment, &self.resources.tiledata).into();
                (who, drawn)
            })
            .collect();
        // Whoever the view no longer holds walked out of range, and their clock
        // goes with them. Our own body is kept by its serial like anyone else's;
        // the placeholder's `None` is gone the moment a shard names us, which is
        // right — it was never a mobile.
        self.world.presentation.crowd.retain(|who| {
            who.is_some_and(|serial| {
                serial == view.player.serial
                    || view.mobiles.contains_key(&serial)
                    || view
                        .items
                        .get(&serial)
                        .is_some_and(|item| item.graphic == CORPSE_GRAPHIC)
            })
        });
        self.world.connection = format!("in world as 0x{:08X}", view.player.serial.raw());
        // The newest line in the journal, heard once and hung over its
        // speaker's head for a while — compared against the old view, still
        // in `self.world.authoritative.view` at this point, so a redraw that changed nothing else
        // does not restart the hold on the same sentence. A system line
        // (`serial: None`) has no mobile to hang over and is left for the
        // HUD's world window instead, which is not built yet.
        if let Some(latest) = view.journal.back() {
            let already_heard = previous_latest.as_ref() == Some(latest);
            if !already_heard {
                if let Some(serial) = latest.serial {
                    self.world.presentation.crowd.hear(
                        Some(serial),
                        latest.text.clone(),
                        latest.font,
                        latest.hue,
                    );
                }
            }
        }
        // Whole, for the HUD's world window: the three projections above are
        // what the renderer wants, and none of them keeps a serial.
        self.world.authoritative.view = Some(Box::new(view));
        // The offline placeholder exists so a map-only window has a body to
        // inspect. A connected client must never reveal it while login packets
        // are still in flight: this snapshot is the first world picture the
        // shard has actually authorised us to draw.
        self.world.render_ready = true;
        // The camera follows the body, which is what `0x20` is for — unless it
        // has been unlocked, in which case the eye is the mouse's and the body
        // is free to walk off the screen. `Home` puts it back. After the view is
        // stored, because that is what says who we are, and the glide is keyed
        // by it.
        //
        // Zero, for the reason `App::walk_offline` says: a packet is not a
        // frame. Game motion was brought up to date before this fold, so there
        // is no elapsed time left to hand a rig anyway.
        self.follow_player(std::time::Duration::ZERO);
        self.assert_motion_projection();
    }

    /// Point the eye at our own body, wherever the game-motion core has it.
    ///
    /// Called every frame and not only when a step arrives: the glide moves the
    /// body a few pixels per frame, and an eye that moved a tile at a time would
    /// jerk the whole world under it. The sprite and camera read the same
    /// `GameMotion` pose, so they cannot disagree by a frame.
    ///
    /// `elapsed` is the same span the game-motion clock was just advanced by,
    /// deliberately the same value: a rig that filters is integrating over it,
    /// and a camera integrating a different amount of time than the body moved
    /// through lags by whatever the difference was — which varies frame to
    /// frame, and varying lag is what an eye reads as a stutter.
    pub(crate) fn follow_player(&mut self, elapsed: std::time::Duration) {
        self.world.presentation.player.drawn = self.world.drawn_player();
        let gaze = mobiles::gaze(&self.world.presentation.player);
        self.control.follow_body(gaze, elapsed);
        // What the eye was asked for, what the screen was given, and what the
        // filter had before the quantiser — the three the bench records, from
        // the one place the camera is advanced.
        //
        // Only while the eye is the body's: unlocked, the camera is wherever a
        // hand left it and a lag against a body it is not following is not a
        // number about the rig.
        if let Some(state) = self.control.eye_exact() {
            if self.control.follow() == Follow::Body {
                self.scope
                    .record(elapsed, gaze, self.control.camera().eye(), state);
            }
        }
    }

    /// Verify the actual sprite projection against the continuous game core.
    /// Crowd still owns animation groups for every mobile, but it is no longer
    /// an authority for the player position between tiles.
    fn assert_motion_projection(&self) {
        self.world.motion.debug_assert_valid();
        debug_assert_eq!(self.world.presentation.player.drawn, self.world.motion.drawn());
    }
}

/// Fill the numbered slots used by `Cliloc.enu` without making the art-file
/// reader depend on the packet format that supplies their values.
///
/// # The slot name is not part of the key
///
/// A slot is `~<index>_<NAME>~` and the name is documentation: `1050045` — the
/// cliloc every mobile's tooltip is — reads `~1_PREFIX~~2_NAME~~3_SUFFIX~`,
/// while a skill message reads `~1_val~`. This used to substitute only the two
/// spellings of `val`, which was enough for the journal lines that were its only
/// caller and silently wrong for everything else: a tooltip resolved that way
/// draws the literal text `~1_PREFIX~`. The index alone decides which argument
/// fills a slot, so the name is read past rather than matched.
///
/// A slot whose index names no argument is left verbatim. That is deliberate:
/// blanking it would turn "the shard sent fewer arguments than this string
/// wants" into a sentence with a hole in it that reads as authored.
pub(crate) fn resolve_cliloc_arguments(template: &str, arguments: &str) -> String {
    let arguments: Vec<&str> = arguments.split('\t').collect();
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('~') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('~') else {
            // An unpaired `~`. The rest is text, not a slot.
            out.push_str(&rest[open..]);
            return out;
        };
        let slot = &after[..close];
        let index = slot.split('_').next().unwrap_or_default().parse::<usize>();
        match index.ok().and_then(|index| arguments.get(index.wrapping_sub(1))) {
            Some(argument) => out.push_str(argument),
            None => {
                out.push('~');
                out.push_str(slot);
                out.push('~');
            }
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

/// Resolve a server-selected cliloc, retaining a readable built-in line for
/// gameplay messages whose number is absent from the installed client files.
fn resolve_localized_message(cliloc: Option<&Cliloc>, message: &LocalizedMessage) -> String {
    let template = cliloc
        .and_then(|cliloc| cliloc.get(ClilocNumber::new(message.cliloc.0)))
        .or_else(|| localized::fallback(message.cliloc));
    let Some(template) = template else {
        return format!("Localized message #{}", message.cliloc.0);
    };
    resolve_cliloc_arguments(template, &message.arguments)
}

impl App {
    /// Fold a `0xD8` into the client's own store of designed houses.
    ///
    /// **The box has to come from somewhere, and this is the somewhere.** A
    /// `0xD8` carries no width or height — the grid stride *is* the height — so
    /// a receiver has to know the house's box before a byte of the planes means
    /// anything. A designed house is drawn as `0x4000 | foundation`, and a
    /// foundation is a real multi in every client's own files, so the box is
    /// the foundation's. That is also the answer for the classic client, which
    /// does exactly this.
    ///
    /// Silent on every refusal. A design for a house this client has not been
    /// shown, or whose foundation its files lack, is one it cannot place — and
    /// there is nothing to tell the player that an empty tile does not already
    /// say.
    pub(crate) fn fold_design(&mut self, bytes: &[u8]) {
        use openshard_protocol::design::{DesignBounds, DesignDetail};

        // The header first, for the serial — which is what says whose box to
        // look up. Read with a placeholder box, because the header is before
        // every plane and needs none.
        let peek = DesignBounds {
            x_min: 0,
            y_min: 0,
            width: 1,
            height: 1,
        };
        let Ok(header) = DesignDetail::decode(bytes, peek) else {
            return;
        };
        let Some(serial) = header.serial.validate() else {
            return;
        };
        let Some(bounds) = self.house_bounds(serial) else {
            return;
        };
        // And again, properly. The first read's tiles are discarded rather than
        // trusted: with the wrong stride every grid plane lands on the wrong
        // tile, which is worse than not drawing at all because it looks
        // deliberate.
        let Ok(design) = DesignDetail::decode(bytes, bounds) else {
            return;
        };
        self.world.authoritative.designs.insert(
            serial,
            crate::world::HouseShape {
                revision: design.revision.0,
                components: design
                    .tiles
                    .into_iter()
                    .map(|tile| openshard_uofiles::multi::Component {
                        graphic: tile.graphic.0,
                        dx: i16::from(tile.dx),
                        dy: i16::from(tile.dy),
                        dz: i16::from(tile.dz),
                        // Every tile on the wire is one the client draws: the
                        // undrawn ones never went into the packet.
                        flags: 1,
                    })
                    .collect(),
            },
        );
    }

    /// Ask the shard for every designed house whose shape this client does not
    /// hold at the revision the shard last named.
    ///
    /// The other half of the two-packet bargain, and it is the half that makes
    /// the first one worth sending: without this the revision would be a fact
    /// nobody acted on, and with it a client that already has the picture asks
    /// for nothing at all.
    fn ask_for_stale_designs(&self, view: &WorldView) {
        let Some(link) = self.world.shard.link() else {
            return;
        };
        for (&house, &revision) in &view.designs {
            if self
                .world
                .authoritative
                .designs
                .get(&house)
                .is_some_and(|held| held.revision == revision)
            {
                continue;
            }
            link.query_design(house);
        }
    }

    /// The box a designed house's planes were laid out on — its foundation's own
    /// multi, out of this client's files.
    fn house_bounds(
        &self,
        house: openshard_protocol::serial::Serial,
    ) -> Option<openshard_protocol::design::DesignBounds> {
        use openshard_protocol::wire::MultiId;

        let view = self.world.authoritative.view.as_ref()?;
        let item = view.items.get(&house)?;
        if item.graphic.0 & MultiId::FLAG == 0 {
            return None;
        }
        let multis = self.resources.multis.as_deref()?;
        let multi = multis.get(MultiId::from_graphic(item.graphic).0)?;
        let box_ = openshard_uofiles::multi::bounds(&multi.components)?;
        Some(openshard_protocol::design::DesignBounds {
            x_min: i8::try_from(box_.min_x).ok()?,
            y_min: i8::try_from(box_.min_y).ok()?,
            width: usize::from(box_.max_x.abs_diff(box_.min_x)) + 1,
            height: usize::from(box_.max_y.abs_diff(box_.min_y)) + 1,
        })
    }
}

/// What to draw for one world item, once a multi has been considered.
///
/// # Three answers, and `Option` could only carry two
///
/// This used to be an `Option<Vec<GroundItem>>`, and the doc under it claimed
/// that answering `None` was what stopped a house drawing "as whatever static
/// happened to sit there". It did not, because `None` meant **three** different
/// things and the caller could only act on one of them: not a multi at all, no
/// multi table, and a multi id this client's table does not hold. The first
/// falls through to the ordinary item path correctly; the other two fell through
/// too, and drew `0x4000 | id` out of the static art — the exact failure the
/// comment said had been fixed.
///
/// It is live today for any multi the client's table lacks: an install whose
/// `multi.mul` is older than the shard's, and **every foundation id**, which is
/// what made a designed house the case that found it. See
/// `docs/customisation.md`'s C8.
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum MultiDraw {
    /// Not a multi. The caller draws the item the ordinary way.
    NotAMulti,
    /// A multi this client can draw, expanded into its pieces.
    Pieces(Vec<GroundItem>),
    /// A multi this client **cannot** draw — no table, or an id the table does
    /// not hold. Nothing is drawn and nothing is picked, which is the honest
    /// answer: a house whose shape this client does not have is a house it
    /// cannot show, and showing one unrelated static in its place is worse than
    /// showing nothing, because nothing is visibly nothing.
    Unknown,
}

/// A house, as the pieces a renderer draws.
///
/// A multi is one item on the wire and a hundred statics on screen. This is the
/// seam where that expansion happens, so the renderer never learns what a multi
/// is: it is handed more items and nothing else changes.
///
/// Two things it gets right and a caller writing it inline would not:
///
/// - **The flag test comes first.** Almost every item is not a house, and the
///   table is only consulted for the graphics that could be one.
/// - **Only the drawn components.** A multi's list carries tiles the client never
///   draws, and the flag saying so reads backwards from its name — see
///   [`openshard_uofiles::multi`].
pub(crate) fn multi_pieces(
    multis: Option<&openshard_uofiles::multi::Multis>,
    design: Option<&[openshard_uofiles::multi::Component]>,
    graphic: Graphic,
    at: Point,
    hue: Hue,
) -> MultiDraw {
    use openshard_protocol::wire::MultiId;

    if graphic.0 & MultiId::FLAG == 0 {
        return MultiDraw::NotAMulti;
    }
    // The house's own shape first, when the shard has sent one. A designed house
    // *is* its design — the foundation multi under it is a bare platform, and
    // drawing that instead is the same wrong-picture failure as drawing an
    // unrelated static, one step less obvious.
    if let Some(design) = design {
        return MultiDraw::Pieces(laid_out(design, at, hue));
    }
    let Some(multi) = multis.and_then(|multis| multis.get(MultiId::from_graphic(graphic).0)) else {
        return MultiDraw::Unknown;
    };
    MultiDraw::Pieces(laid_out(multi.components.as_slice(), at, hue))
}

/// A component list, placed at an origin. The one piece of arithmetic a multi
/// and a design share, written once so the two cannot come to disagree about
/// what an offset means.
fn laid_out(components: &[openshard_uofiles::multi::Component], at: Point, hue: Hue) -> Vec<GroundItem> {
    components
        .iter()
        .filter(|component| component.drawn())
        .map(|component| GroundItem {
            at: Point::new(
                at.x.wrapping_add_signed(component.dx),
                at.y.wrapping_add_signed(component.dy),
                at.z.saturating_add(component.dz as i8),
            ),
            graphic: Graphic(component.graphic),
            hue,
            // A house's wall is a wall, not a pile of walls: a multi's parts
            // are pictures laid out from a component list and have no stack of
            // their own to count. See `GroundItem::amount`.
            amount: openshard_protocol::items::ItemAmount::ONE,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use openshard_client_render::follow::Gaze;
    use openshard_client_render::mobiles::EquipmentLayer;
    use openshard_movement::WALK_HOLD;
    use openshard_protocol::direction::{Direction, Facing};
    use openshard_protocol::speech::{Font, TalkMode};
    use openshard_protocol::wire::{Graphic, Hue, Layer};
    use openshard_protocol::world::Point;
    use openshard_uofiles::tiledata::AnimId;

    use super::*;

    #[test]
    fn cliloc_arguments_fill_their_numbered_slots() {
        assert_eq!(
            resolve_cliloc_arguments("You apply ~1_val~ to ~2_VAL~.", "a bandage\tBob"),
            "You apply a bandage to Bob."
        );
    }

    /// The cliloc every mobile's tooltip is. Its slots are not called `val`, and
    /// substituting only that spelling drew the placeholder text at the player.
    #[test]
    fn a_slot_is_matched_by_its_number_and_not_by_its_name() {
        assert_eq!(
            resolve_cliloc_arguments("~1_PREFIX~~2_NAME~~3_SUFFIX~", " \tLord British\t [OSS]"),
            " Lord British [OSS]"
        );
    }

    #[test]
    fn a_slot_with_no_argument_is_left_as_it_was_authored() {
        // Better a visible placeholder than a sentence with a hole in it that
        // reads as if the shard meant to say nothing there.
        assert_eq!(
            resolve_cliloc_arguments("~1_NAME~ has ~2_AMOUNT~ left", "Bob"),
            "Bob has ~2_AMOUNT~ left"
        );
    }

    #[test]
    fn a_stray_tilde_is_text() {
        assert_eq!(
            resolve_cliloc_arguments("about ~50% done", "x"),
            "about ~50% done"
        );
    }

    #[test]
    fn a_missing_begging_cliloc_uses_the_shared_fallback() {
        let message = LocalizedMessage {
            serial: None,
            graphic: None,
            mode: TalkMode::Regular,
            hue: Hue::NONE,
            font: Font::DEFAULT,
            cliloc: localized::begging::UNWILLING,
            name: "System".to_owned(),
            arguments: String::new(),
        };

        assert_eq!(
            resolve_localized_message(None, &message),
            "They seem unwilling to give you any money."
        );
    }

    #[test]
    fn a_prediction_starts_the_players_glide_before_an_ack_arrives() {
        let start = Point::new(100, 100, 0);
        let next = Point::new(101, 100, 0);
        let facing = Facing::walking(Direction::East);
        let mut crowd = crowd::Crowd::default();
        crowd.commanding(None);
        let mut player = crowd.see(None, start, Graphic(400), facing, Hue::NONE, false);
        player.equipment = vec![EquipmentLayer {
            graphic: AnimId(7005),
            hue: Hue::NONE,
            layer: Layer::TUNIC,
        }]
        .into();
        let equipment = player.equipment.clone();
        let standing = player.group;

        project_motion(
            &mut crowd,
            None,
            &mut player,
            MotionRenderState {
                rendered: openshard_client_net::walk::Predicted {
                    position: next,
                    facing,
                },
                predicted: openshard_client_net::walk::Predicted {
                    position: next,
                    facing,
                },
                transition: Some((start, next)),
                corrected: false,
            },
            false,
        );

        // A vendor/container packet may rebuild the presentation while this
        // glide is active. Replaying the same motion projection must not start
        // a second transition or change its logical endpoints.
        project_motion(
            &mut crowd,
            None,
            &mut player,
            MotionRenderState {
                rendered: openshard_client_net::walk::Predicted {
                    position: next,
                    facing,
                },
                predicted: openshard_client_net::walk::Predicted {
                    position: next,
                    facing,
                },
                transition: Some((start, next)),
                corrected: false,
            },
            false,
        );

        assert_eq!(player.at, next, "the prediction is its destination tile");
        assert_ne!(player.group, standing, "the prediction started a walk");
        assert!(crowd.anyone_gliding(), "the display-rate wake is armed now");
        assert!(
            Rc::ptr_eq(&player.equipment, &equipment),
            "prediction does not replace authoritative equipment"
        );

        crowd.advance(WALK_HOLD / 2);
        assert_ne!(
            crowd.drawn_for(None),
            Some(Gaze::on(start)),
            "the body moves before the server's acknowledgement"
        );
    }

    /// A house is one item on the wire and many statics on screen — and the one
    /// case that was silently wrong before: without the expansion, `0x4064` is
    /// looked up in the *static* art, where it is a valid id for something that
    /// is not a house at all.
    #[test]
    fn a_multi_becomes_the_statics_it_draws_as() {
        use openshard_uofiles::multi::{Component, Multi, Multis};

        let cottage = Multi::new(
            0x64,
            vec![
                // The signature tile every multi starts with, undrawn.
                Component {
                    graphic: 1,
                    dx: 0,
                    dy: 0,
                    dz: 0,
                    flags: 0,
                },
                Component {
                    graphic: 0x0006,
                    dx: -1,
                    dy: 2,
                    dz: 0,
                    flags: 1,
                },
                Component {
                    graphic: 0x0007,
                    dx: 1,
                    dy: 0,
                    dz: 20,
                    flags: 1,
                },
            ],
        );
        let multis = Multis::of([cottage]);

        let pieces = multi_pieces(
            Some(&multis),
            None,
            Graphic(0x4064),
            Point::new(100, 200, 5),
            Hue(0x1234),
        );
        let MultiDraw::Pieces(pieces) = pieces else {
            panic!("a multi the table holds did not expand");
        };
        assert_eq!(pieces.len(), 2, "the undrawn signature tile was drawn");
        assert_eq!(pieces[0].at, Point::new(99, 202, 5), "the offset is signed");
        assert_eq!(pieces[0].graphic, Graphic(0x0006));
        assert_eq!(
            pieces[0].hue,
            Hue(0x1234),
            "a dyed house dyes every wall, which is the one hue there is"
        );
        assert_eq!(pieces[1].at, Point::new(101, 200, 25), "z is an offset too");
    }

    /// An ordinary item is not a house, and the table is not even consulted.
    #[test]
    fn an_item_below_the_multi_flag_is_not_a_multi() {
        assert_eq!(
            multi_pieces(None, None, Graphic(0x0EED), Point::new(1, 1, 0), Hue(0)),
            MultiDraw::NotAMulti,
            "a pile of gold went looking for a house"
        );
    }

    /// **A multi this client cannot draw is not the same answer as an item.**
    ///
    /// This test used to assert `is_none()` for both, and passed — while the
    /// caller fell through on `None` and drew the house's own graphic out of the
    /// static art, which is the bug its own name says it prevents. It tested the
    /// return value rather than the behaviour, and the two had come apart.
    #[test]
    fn a_client_with_no_multi_table_draws_no_house_rather_than_the_wrong_sprite() {
        assert_eq!(
            multi_pieces(None, None, Graphic(0x4064), Point::new(1, 1, 0), Hue(0)),
            MultiDraw::Unknown,
            "with no table this must still be recognised as a multi"
        );
    }

    /// A multi id the table does not hold is `Unknown`, not an item.
    ///
    /// The case a designed house made unavoidable: `FOUNDATION_IDS` runs
    /// `0x13EC..0x1D00` and a shipped `multi.mul` holds 326 entries, so nearly
    /// every foundation is an id this client has never heard of. It is also the
    /// case for any install whose files are older than the shard's, which has
    /// been true since houses existed and drew a static nobody chose.
    #[test]
    fn a_multi_id_the_table_does_not_hold_draws_nothing() {
        use openshard_uofiles::multi::{Component, Multi, Multis};

        let known = Multi::new(
            0x64,
            vec![Component {
                graphic: 0x0006,
                dx: 0,
                dy: 0,
                dz: 0,
                flags: 1,
            }],
        );
        let multis = Multis::of([known]);

        assert_eq!(
            multi_pieces(Some(&multis), None, Graphic(0x53EC), Point::new(1, 1, 0), Hue(0)),
            MultiDraw::Unknown,
            "a foundation this install has no shape for fell through to the static art"
        );
    }

    /// **A designed house draws as its design, not as its foundation.**
    ///
    /// The foundation under a customised house is a bare platform, so falling
    /// back to it is the same wrong-picture failure as drawing an unrelated
    /// static — one step less obvious, because a platform at least looks like a
    /// building.
    #[test]
    fn a_design_wins_over_the_multi_table() {
        use openshard_uofiles::multi::{Component, Multi, Multis};

        let foundation = Multi::new(
            0x64,
            vec![Component {
                graphic: 0x0006,
                dx: 0,
                dy: 0,
                dz: 0,
                flags: 1,
            }],
        );
        let multis = Multis::of([foundation]);
        let design = [Component {
            graphic: 0x1234,
            dx: 2,
            dy: 3,
            dz: 4,
            flags: 1,
        }];

        let MultiDraw::Pieces(pieces) = multi_pieces(
            Some(&multis),
            Some(&design),
            Graphic(0x4064),
            Point::new(10, 10, 0),
            Hue(0),
        ) else {
            panic!("a designed house drew nothing");
        };
        assert_eq!(pieces.len(), 1);
        assert_eq!(
            pieces[0].graphic,
            Graphic(0x1234),
            "the foundation was drawn instead"
        );
        assert_eq!(pieces[0].at, Point::new(12, 13, 4), "the design's own offsets");
    }

    /// And a design is drawn even when this client's files have no such multi at
    /// all — which is the case that matters, because a foundation id is exactly
    /// what an install is most likely to be missing.
    #[test]
    fn a_design_draws_without_the_multi_table() {
        use openshard_uofiles::multi::Component;

        let design = [Component {
            graphic: 0x1234,
            dx: 0,
            dy: 0,
            dz: 0,
            flags: 1,
        }];
        let MultiDraw::Pieces(pieces) =
            multi_pieces(None, Some(&design), Graphic(0x53EC), Point::new(1, 1, 0), Hue(0))
        else {
            panic!("a design this client holds was not drawn");
        };
        assert_eq!(pieces.len(), 1);
    }
}
