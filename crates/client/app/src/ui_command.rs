//! State mutation driven by the player's own input: movement and targeting.
//!
//! [`App::walk`] and [`App::walk_toward_cursor`] are the keyboard's and the
//! mouse's own shares of taking a step; [`App::use_under_cursor`] and
//! [`App::attack_under_cursor`] are the single- and double-click, aimed by
//! the same pick the highlight was drawn from. [`ask_between`] and
//! [`heading_between`] are the bearing arithmetic both movement methods
//! share, pulled free of `&self` so they can be checked against a drawn
//! picture rather than a running window — see their own docs.
//!
//! Deliberately apart from `own_windows.rs`, which is also player input: a
//! gump press and a walk order are different subsystems that happen to both
//! start at the mouse, and folding them into one file would make "what does
//! a click do" answerable only by reading the whole thing.

use std::time::Instant;

use openshard_client_render::camera::{self, Camera};
use openshard_client_render::{doors, items, mobiles};
use openshard_movement::{Heading, Lean};
use openshard_protocol::direction::{Direction, Facing};
use openshard_protocol::target::{TargetKind, TargetResponse};
use openshard_protocol::wire::Layer;
use openshard_protocol::world::Point;
use winit::window::CursorIcon;

use crate::app::App;
use crate::net_command::project_motion;
use crate::world::{advance_presentation_to, cluttered, cluttered_with_doors_open, terrain};
use crate::{DEAD_ZONE, TURN_ZONE, steer};

impl App {
    /// Ask for this character's paperdoll without relying on a world pick.
    ///
    /// A closed doll must always have a way back: the body can be covered by a
    /// roof, another mobile or an open shop, none of which should make the
    /// paperdoll request unreachable.
    pub(crate) fn open_own_paperdoll(&self) {
        let Some(serial) = self
            .world
            .authoritative
            .view
            .as_ref()
            .map(|view| view.player.serial)
        else {
            return;
        };
        if let Some(link) = self.world.shard.link() {
            link.paperdoll(serial);
        }
    }

    /// Open this character's backpack without relying on the paperdoll being
    /// visible or open.
    pub(crate) fn open_own_inventory(&self) {
        let Some(backpack) = self.world.authoritative.view.as_ref().and_then(|view| {
            view.player
                .equipment
                .iter()
                .find(|item| item.layer == Layer::BACKPACK)
                .map(|item| item.serial)
        }) else {
            return;
        };
        if let Some(link) = self.world.shard.link() {
            link.use_object(backpack);
        }
    }

    /// Hold Tab to enter war mode and release it to return to peace mode.
    ///
    /// This is ClassicUO's default Tab behaviour. The remembered key state
    /// makes operating-system repeat events harmless and lets focus/UI loss
    /// release a stance whose physical key-up would otherwise never arrive.
    pub(crate) fn set_war_mode_held(&mut self, held: bool) {
        if self.input.war_mode_held == held {
            return;
        }
        self.input.war_mode_held = held;
        if let Some(link) = self.world.shard.link() {
            link.war_mode(held);
        }
    }

    /// The server's `0x6C` is a modal target operation. A crosshair is the
    /// platform cursor closest to Ultima's targeting reticle and, unlike a
    /// decorative in-world sprite, remains visible over windows as well.
    pub(crate) fn sync_target_cursor(&self) {
        let targeting = self
            .world
            .authoritative
            .view
            .as_ref()
            .is_some_and(|view| view.target.is_some());
        if let Some(window) = self.window.as_ref() {
            window.window.set_cursor(if targeting {
                CursorIcon::Crosshair
            } else {
                CursorIcon::Default
            });
        }
    }

    /// Answer the target cursor currently raised by the shard, if the click is
    /// a legal target for its kind. This runs before ordinary selection, combat
    /// and double-click use: a tool's second click belongs to that tool.
    pub(crate) fn target_under_cursor(&mut self, camera: Camera) -> bool {
        let Some(cursor) = self
            .world
            .authoritative
            .view
            .as_ref()
            .and_then(|view| view.target)
        else {
            return false;
        };
        if !self.world_owns_pointer() {
            return false;
        }
        let object = match (self.picking.hover.mobile, self.picking.hover.item) {
            (Some(Some(mobile)), _) => Some(mobile),
            (_, Some(item)) => Some(item),
            _ => None,
        };
        if cursor.cursor.kind == TargetKind::Object && object.is_none() {
            return false;
        }
        // A location target still has to name a static when the cursor is on
        // one.  Sending graphic zero means "bare land" on the 0x6C wire; doing
        // that for a tree made the shard resolve the grass underneath it and
        // reject an otherwise valid lumberjacking swing.  The static pick also
        // carries its placed z, which is the exact value the shard verifies
        // against the map.
        let (location, graphic) = if let Some(picked) = self.picking.hover.static_ {
            (picked.at, Some(picked.graphic))
        } else {
            let Some(tile) = self.pick_tile(camera) else {
                return false;
            };
            (Point::new(tile.at.x, tile.at.y, tile.stand_z.0), None)
        };
        if let Some(link) = self.world.shard.link() {
            link.target(TargetResponse {
                cursor_id: cursor.cursor.cursor_id,
                object,
                location,
                graphic,
                cancelled: false,
            });
        }
        // There is no server packet saying a successful target was consumed.
        // This is client-side UI state, just like a gump reply disappearing.
        if let Some(view) = self.world.authoritative.view.as_mut() {
            view.target = None;
        }
        self.sync_target_cursor();
        true
    }

    /// Cancel the modal target cursor currently owned by the shard.
    ///
    /// A target is not a movement gesture while it is open: the classic
    /// right-click means "never mind", even if the pointer happens to be over
    /// a window.  Sending the cancellation matters as much as clearing the
    /// local crosshair; otherwise the shard keeps the pending purpose and the
    /// next answer can be applied to a tool the player thought they had left.
    pub(crate) fn cancel_target_cursor(&mut self) -> bool {
        let Some(cursor) = self
            .world
            .authoritative
            .view
            .as_ref()
            .and_then(|view| view.target)
        else {
            return false;
        };
        if let Some(link) = self.world.shard.link() {
            link.target(TargetResponse {
                cursor_id: cursor.cursor.cursor_id,
                object: None,
                location: Point::new(0, 0, 0),
                graphic: None,
                cancelled: true,
            });
        }
        if let Some(view) = self.world.authoritative.view.as_mut() {
            view.target = None;
        }
        self.sync_target_cursor();
        true
    }

    /// Take a step, answering whether anything on screen changed.
    ///
    /// Movement is clamped to the map rather than wrapped: walking off the north
    /// edge in UO is impossible, and a camera that wrapped would draw a seam
    /// between two sides of the world.
    /// Ask the shard for one step, and draw it before it answers.
    ///
    /// The whole of the online walk, and it is here rather than on the shard
    /// thread because of the height: a `0x02` names the tile a step is asking
    /// for, the server lands the body wherever it actually stands and says
    /// nothing about it (`0x22` carries no position), so this end has to
    /// predict — and predicting needs the terrain, which comes out of the
    /// process's one `MapSnapshot`. See [`crate::link::connect`], which is
    /// handed no map at all.
    ///
    /// `MapTerrain::predict_step` is the shard's own step rule run on this end:
    /// it weighs the land's *average* (the same number the shard's own
    /// `ground_z` computes — on a slope the raw corner differs by most of the
    /// tile's relief, and a body predicted at the corner is drawn sunk into the
    /// hill and sorted behind it) against every platform static on the tile, a
    /// pier's or a bridge's deck among them, reaching from the top of the
    /// surface underfoot and standing on the highest surface within a step.
    /// That last part is what climbs a staircase, and it is why this is not
    /// `predict_z`: the nearest-height guess stays on the floor a stair tile
    /// also carries, and the body walks *through* the stairs while the shard
    /// has it half way up. Never a refusal — see `predict_step`'s own doc — so
    /// it cannot desync from a server that disagrees; it can only draw the
    /// wrong deck for one step, corrected by the next `0x20`.
    fn step_online(&mut self, facing: Facing) {
        // The presentation clocks first, before the step is folded in. This
        // used to happen because a prediction reached the app as an `Update`
        // and `App::on_update` advanced the clock for it; the prediction is
        // made here now, and this is called from `about_to_wait` — where
        // `Crowd`'s own `now` is as old as the last frame. A step recorded up
        // to a frame in the past starts its crossing there, and `crowd::
        // crossing` then measures the time it has left from the same stale
        // instant. The offline arm below says the same thing, and it is the
        // same defect.
        advance_presentation_to(
            &mut self.world.presentation,
            &mut self.world.motion,
            &mut self.last_advance,
            Instant::now(),
        );
        self.project_player_motion();
        let Some(walk) = self.world.authoritative.walk.as_mut() else {
            // A link with no world entered yet: the shard has not said where
            // the body is, so there is nothing to step from.
            return;
        };
        let terrain = openshard_movement::MapTerrain::new(self.resources.map.map(), &self.resources.tiledata);
        let stepped = walk.step(facing, |from, tile| {
            i8::try_from(terrain.predict_step(from, tile.x, tile.y)).ok()
        });
        let bytes = match stepped {
            Ok(bytes) => bytes,
            // A step this end refused on its own: the edge of the map, which
            // the server would refuse too, or a shard that has stopped
            // answering and is five steps behind already. Neither is worth a
            // round trip, and the body simply stays where it is.
            Err(refusal) => {
                tracing::debug!(%refusal, "not stepping");
                return;
            }
        };
        let body = crate::link::Body {
            predicted: walk.predicted(),
            corrected: false,
        };
        let sequence = walk
            .newest_pending_sequence()
            .expect("an accepted step is pending");
        self.world
            .shard
            .link()
            .expect("the link was there a moment ago")
            .step(bytes);
        // The body moves *now*, on this end's own prediction, rather than a
        // round trip later when the `0x22` says it may. That is the whole of
        // the lag compensation: the ack changes nothing on screen, and only a
        // refusal does.
        self.apply_prediction(body, sequence);
        if let Some(trace) = self.movement_trace.as_mut() {
            trace.record_detail(
                "command_step",
                &format!("facing={facing:?} goal={:?}", self.steer.goal()),
                &self.world,
                self.control.camera(),
            );
        }
    }

    pub(crate) fn walk(&mut self, facing: Facing) -> bool {
        // A hand on the body outranks a scenario, the same way a hand on the
        // camera outranks the lock: the two would otherwise both write the
        // player's position and the picture would be neither.
        self.replay = None;

        // Connected, the keyboard moves nothing: it asks. The body goes where
        // the `0x22` says it went, which is the whole point of the walk
        // handshake — a client that stepped locally and corrected later would
        // be predicting, and the prediction lives in `Walk` where it can be
        // rolled back.
        if self.world.shard.link().is_some() {
            self.open_door_ahead(facing);
        }
        if self.world.shard.link().is_some() {
            self.step_online(facing);
            return false;
        }
        // And a body whose shard is *gone* stands where the last packet left
        // it. Only the viewer walks itself: below is the map viewer's own
        // movement, and running it after a disconnect would put the character
        // somewhere nobody ever said it was — which is what made a dropped
        // connection read as a working game. See `world::Shard`.
        if !self.world.shard.is_viewer() {
            return false;
        }

        // Turning costs no ground here either, now decided by the same rule
        // the online handshake and the server share
        // (`openshard_movement::intend`) rather than the simplification this
        // used to be — every call moving the body, turn or not, because there
        // was no server round trip to tell the two apart. That was rarely
        // visible when a fresh direction changed once in a while; it stopped
        // being rare once `Steering::detour` started sending several
        // direction changes a hold's worth apart in real cadence, but one
        // right after another within a single event-loop wake — and moving
        // the body on every one of them was a real body covering twice the
        // ground its pace implied.
        let motion = self.world.motion.planning_state();
        let turn = matches!(
            openshard_movement::intend(motion.position, motion.facing, facing),
            openshard_movement::Intent::Turned { .. }
        );
        let (x, y) = match turn {
            true => (motion.position.x, motion.position.y),
            false => {
                let (dx, dy) = facing.direction.step();
                let x =
                    (i32::from(motion.position.x) + dx).clamp(0, self.resources.map.map().width() as i32 - 1);
                let y = (i32::from(motion.position.y) + dy)
                    .clamp(0, self.resources.map.map().height() as i32 - 1);
                (x as u16, y as u16)
            }
        };
        // On the surface there — the ground's average, or the highest platform
        // static's deck a step reaches — not at some height of the camera's,
        // and not the land alone: a mobile below the terrain is correctly
        // hidden by it, which is what the depth buffer is for and what looks
        // exactly like a mobile that failed to draw, and the same held for a
        // pier or a bridge before their deck was weighed. `predict_step` rather
        // than `predict_z` because reaching from the surface underfoot is what
        // climbs a staircase; the nearest-height guess walks through it. See
        // `link.rs`'s online `Command::Step`, which wants the identical answer
        // once a server is involved.
        let terrain = terrain(&self.resources);
        let ground = i8::try_from(terrain.predict_step(motion.position, x, y)).unwrap_or(motion.position.z);
        // The presentation clocks first, before the step is folded in, and for
        // the same reason `App::user_event` does it for a step off the wire: a
        // step is timestamped with `Crowd`'s own `now`, and this is called from
        // `about_to_wait` — where that clock is as old as the last frame. A step
        // recorded up to a frame in the past starts its crossing there, and
        // `crowd::crossing` then measures the time it has left from the same
        // stale instant. This is the offline half of the walk and it had the
        // defect the online half was already fixed for.
        let now = Instant::now();
        advance_presentation_to(
            &mut self.world.presentation,
            &mut self.world.motion,
            &mut self.last_advance,
            now,
        );
        let landed = Point::new(x, y, ground);
        // Offline movement is authoritative immediately, but it still changes
        // the one movement core before its renderer projection is updated.
        self.world.motion.accept_trusted_step(landed, facing);
        // The offline viewer is authoritative immediately, but it still uses
        // the same motion-to-Crowd adapter as a predicted online step.  This
        // keeps its glide source in PlayerMotion rather than asking Crowd to
        // rediscover it from two tiles.
        project_motion(
            &mut self.world.presentation.crowd,
            None,
            &mut self.world.presentation.player,
            self.world.motion.render_state(),
            false,
        );
        // Offline there is no shard to refuse a step, so nothing here is
        // speculative the way an online prediction is — trusted outright,
        // same as a correction is.
        self.world.presentation.cutaway_at = self.world.motion.planning_state().position;
        // Offline the body is what the camera is locked to, exactly as the
        // server's is when there is a server. Unlocked, walking still walks and
        // the body may leave the screen — walking and looking are different
        // questions, and `Home` is the answer to the second.
        //
        // No time has passed: this is an input, not a frame. A rig that filters
        // integrates over the span it is given, and time passes in `App::draw`.
        self.follow_player(std::time::Duration::ZERO);
        true
    }

    /// Use the closed door in the next cell before asking the shard to step
    /// into it. The shard remains authoritative: it may reject a locked door,
    /// and only its following item update makes this client regard the way as
    /// open.
    fn open_door_ahead(&mut self, facing: Facing) {
        if !self.auto_open_doors {
            self.auto_opened_door = None;
            return;
        }
        let motion = self.world.motion.planning_state();
        let openshard_movement::Intent::Stepped { target, .. } =
            openshard_movement::intend(motion.position, motion.facing, facing)
        else {
            return;
        };
        let door = self
            .world
            .presentation
            .items
            .iter()
            .zip(&self.world.presentation.item_serials)
            .find(|(item, _)| {
                item.at.x == target.x
                    && item.at.y == target.y
                    && doors::is_door(item.displayed())
                    && !doors::is_open(item.displayed())
            })
            .map(|(_, serial)| *serial);
        if door == self.auto_opened_door {
            return;
        }
        self.auto_opened_door = door;
        if let (Some(serial), Some(link)) = (door, self.world.shard.link()) {
            link.use_object(serial);
        }
    }

    /// Send the body to whatever tile the cursor is over, answering whether
    /// anything on screen changed.
    ///
    /// The mouse's whole share of walking: a click names a destination and a
    /// drag restates it, and `steer.rs` is what turns either into one step every
    /// step's length. A cursor that is off the map or outside the world's
    /// viewport names no tile and is left alone rather than treated as the
    /// nearest one — a move order nobody gave is worse than one that did
    /// nothing.
    /// The mouse's whole share of walking, one call for both of its idioms:
    /// `self.input.ctrl_held` says which. Without Ctrl this is a heading — no map
    /// touched, no route planned, the same "run toward the cursor" a strategy
    /// game's held mouse button means. With it, a move order: a route planned
    /// with `find_path` to the exact tile. See `steer.rs`'s module docs for why
    /// they are not the same thing wearing one name.
    pub(crate) fn walk_toward_cursor(&mut self) -> bool {
        // As above: between frames, what is on screen is what the last frame drew.
        let Some(tile) = self.pick_tile(*self.control.camera()) else {
            return false;
        };
        let guide = terrain(&self.resources);
        let opened = cluttered_with_doors_open(&self.world, &self.resources);
        let cluttered = cluttered(&self.world, &self.resources);
        let ground = steer::Ground {
            real: if self.auto_open_doors { &opened } else { &cluttered },
            through_doors: &opened,
            guide: &guide,
            coarse: self.resources.coarse.as_ref(),
        };
        let motion = self.world.motion.planning_state();
        let facing = if self.input.ctrl_held {
            self.steer.go_to(
                tile.at,
                motion.position,
                Instant::now(),
                motion.facing.direction,
                ground,
            )
        } else {
            self.steer.steer(
                self.ask_to_cursor(*self.control.camera()),
                motion.position,
                Instant::now(),
                motion.facing.direction,
                ground,
            )
        };
        match facing {
            Some(facing) => {
                // The marker under the destination has moved even when the step
                // itself changes nothing on screen, so the redraw is not the
                // step's to decide.
                self.walk(facing);
                true
            }
            None => true,
        }
    }

    /// Which way the cursor is asking the body to walk — measured **on the
    /// screen**, from where the body is drawn, not in the world's grid.
    ///
    /// The two are not the same question, and the screen one is the only one
    /// the player is actually asking. A player pushes the mouse away from the
    /// character in the direction they want it to go; what "that direction"
    /// means is a bearing on a flat picture. The grid is where the answer has
    /// to land — one of eight tile steps — but it is not where the ask lives,
    /// and measuring in the grid quietly swaps the isometric projection for
    /// nothing. That the two happen to agree for the projection drawn today
    /// (`camera::project` is a rotation and a uniform scale, and rounding to a
    /// sector survives that) is a coincidence of the numbers in it, not a
    /// property of the idea — change the tile to a 2:1 diamond, which is what
    /// most isometric art is, and the grid answer starts naming a direction
    /// the cursor is nowhere near.
    ///
    /// The origin is the body's own projected pixel and not the middle of the
    /// viewport, which is what makes this survive a camera that is not locked
    /// to the body: with a free eye the character is off-centre, sometimes far
    /// off-centre, and "away from the middle of the screen" would be a
    /// different direction from "away from the character". Both are defensible
    /// idioms and a shard may one day want the other; this is the one that
    /// keeps meaning what it means while the eye wanders.
    ///
    /// The sector is picked by the largest dot product against the eight
    /// directions' *projected* steps — normalised, since a diagonal projects to
    /// a longer screen vector than a cardinal and the unnormalised comparison
    /// would hand it sectors it has not earned. Those steps come from
    /// `camera::project` itself rather than from constants copied out of it, so
    /// there is one projection in this client and this reads it.
    ///
    /// How far it is asking from is the other half, and it decides *what* is
    /// asked for rather than only which way: a cursor held close in turns the
    /// body and walks it nowhere. [`ask_between`] is the rings; [`TURN_ZONE`]
    /// is the one that matters here.
    ///
    /// `None` when the cursor is on the body: no bearing exists, and picking
    /// one would be inventing an ask.
    pub(crate) fn ask_to_cursor(&self, camera: Camera) -> Option<steer::Ask> {
        let cursor = self.control.cursor();
        // The body's *drawn* pixel, height and all: what a player aims relative
        // to is the sprite they can see, not the tile beneath it.
        ask_between(self.world.drawn_player().eye().pixel(), camera.pick(cursor))
    }

    /// Double-click whatever the cursor is over: ask the shard to use it.
    ///
    /// **Picked against the picture, not against the tile.** A door's leaf is
    /// drawn two tiles up the screen from the tile it stands on, so the tile
    /// under the cursor is the one *behind* it — the answer
    /// [`App::pick_tile`] gives, which is right for the Tile panel and wrong for
    /// this. [`items::pick`] hits the sprite's own opaque texels instead, which
    /// is what the player thinks they clicked on.
    ///
    /// Entities only: the map's statics are not entities and have no serial to
    /// name. What this covers is doors, containers, everything else the shard
    /// has put on the ground — and now mobiles, whose double-click the shard
    /// answers with a `0x88` and which this client can finally draw (see
    /// [`paperdoll`] and `WindowSubject::Paperdoll`).
    ///
    /// Nothing is done locally on the way out. The door swings when the `0x1A`
    /// that redraws it arrives; a client that also opened it itself would show
    /// a door the shard may have refused (a lock, or reach) standing open.
    pub(crate) fn use_under_cursor(&self, camera: Camera) {
        // The same question the highlight is drawn from, so the two cannot
        // disagree about whether the world owns the mouse: a click that arrives
        // while a panel holds the pointer is the panel's.
        if !self.world_owns_pointer() {
            return;
        }
        // The atlas is the frame's, and it is where the art the click is tested
        // against lives — offline, or before the first frame, there is nothing
        // drawn to have clicked on.
        let Some(window) = self.window.as_ref() else {
            return;
        };
        // The same cutaway the frame was drawn with, computed the same way: a
        // barrel hidden under a roof this client is not drawing is not something
        // the player can have pointed at.
        let cutaway = self.cutaway();
        // A creature under the cursor takes the click, and no item is used: it
        // is what the highlight is telling the player they are pointing at, and
        // using the barrel *behind* the shopkeeper is the one answer that is
        // certainly wrong. What a mobile's double-click asks for is the
        // paperdoll — the same `0x06` an item gets, answered differently by the
        // shard (`DoubleClick::interpret`). Ctrl turns that same gesture into
        // the protocol's explicit paperdoll request; this is how a player can
        // inspect a vendor without replacing its normal double-click-to-trade
        // behaviour.
        let drawn = self.drawn_now(&window.atlases.mobiles);
        let on_mobile = mobiles::pick_iter(
            drawn.iter().map(|(_, mobile)| mobile),
            &camera,
            &window.atlases.mobiles,
            &cutaway,
            &self.resources.equip_conv,
            self.control.cursor(),
        );
        if let Some(index) = on_mobile {
            // A body with no serial is one this client is drawing without the
            // shard having named it — the offline viewer's placeholder — and
            // there is nothing to ask about.
            if let (Some(serial), Some(link)) = (drawn[index.position()].0, self.world.shard.link()) {
                let own = self
                    .world
                    .authoritative
                    .view
                    .as_ref()
                    .is_some_and(|view| view.player.serial == serial);
                if own || self.input.ctrl_held {
                    link.paperdoll(serial);
                } else {
                    link.use_object(serial);
                }
            }
            return;
        }
        let Some(index) = items::pick(
            &self.world.presentation.items,
            &camera,
            &self.resources.tiledata,
            &self.world.presentation.tile_animations,
            &window.atlases.statics,
            &cutaway,
            self.control.cursor(),
        ) else {
            return;
        };
        let serial = self.world.presentation.item_serials[index.position()];
        match self.world.shard.link() {
            Some(link) => link.use_object(serial),
            None => tracing::info!(serial = serial.raw(), "nothing used: no shard is connected"),
        }
    }

    /// Aim at whatever body the cursor is on, if this character is at war.
    ///
    /// The single click's half of a fight, beside [`App::use_under_cursor`]'s
    /// double click. Three gates, and each is a different kind of "no":
    ///
    /// - **At peace, nothing is sent.** Not a refusal — a click at peace is a
    ///   selection, which the caller has already made.
    /// - **A ghost sends nothing.** `Player::dead` is `0x2C`'s own answer
    ///   (`docs/combat.md`, D9/P4) — a ghost that somehow still carries the war
    ///   flag still sends no attack, the same `!InWarMode || IsDead` the
    ///   reference gates a swing by.
    /// - **A body with no serial is the offline viewer's placeholder** and
    ///   there is nothing to name.
    ///
    /// Reads [`Picking::hover`](crate::picking::Picking::hover) — what the
    /// *last frame* found under the cursor,
    /// the same answer the highlight is drawn from — rather than picking again
    /// here. That is `use_under_cursor`'s own rule turned into the one it should
    /// always have been: what the player clicked is what they were shown they
    /// were pointing at.
    pub(crate) fn attack_under_cursor(&mut self) {
        let Some(view) = self.world.authoritative.view.as_ref() else {
            return;
        };
        if !view.player.war || view.player.dead {
            return;
        }
        // `on_mobile` is a `Who` — `None` inside the `Some` is a body no shard
        // has named, which the offline viewer draws and nothing can be asked
        // about.
        let Some(Some(mobile)) = self.picking.hover.mobile else {
            return;
        };
        // A corpse is drawn through the mobile renderer so its body is visible,
        // but its serial still names an item. It can be opened, never attacked.
        if !view.mobiles.contains_key(&mobile) {
            return;
        }
        // Attacking yourself is a packet the shard refuses (`combat::attack`
        // checks it) and a click a player never means. Stopped here so the
        // refusal is not a round trip.
        if mobile == view.player.serial {
            return;
        }
        match self.world.shard.link() {
            // The same war-mode click that chose this target gives it up. The
            // server remains the authority and confirms the cleared target with
            // `0xAA`, so the marker cannot get ahead of the actual combat state.
            Some(link) if view.player.attacking == Some(mobile) => link.stop_attacking(),
            Some(link) => link.attack(mobile),
            None => tracing::info!(serial = mobile.raw(), "nothing attacked: no shard is connected"),
        }
    }
}

/// The heading from one point on the screen to another, as one of the eight
/// ways a body can walk plus which side of that way it actually points.
///
/// Split out of [`App::heading_to_cursor`] because it is the whole of the
/// arithmetic and none of the state — a thing that can be checked against a
/// drawn picture rather than against a running window.
///
/// The sector is the largest dot product against the eight directions'
/// *projected* steps, normalised: a diagonal projects to a longer screen vector
/// than a cardinal (44 pixels against 31), and comparing unnormalised would
/// hand the diagonals sectors they have not earned. Those steps come from
/// [`camera::project`] rather than from constants copied out of it, so there is
/// one projection in this client and this reads it.
///
/// Three rings, and the distance is what picks one.
///
/// `None` inside [`DEAD_ZONE`] of the body: a cursor that close is not naming a
/// direction, and answering one anyway is what makes a body with the button
/// held and the mouse sitting still walk at random — the vector is a couple of
/// pixels long, so which of the eight sectors it lands in is decided by the
/// hand's own jitter, and every twitch of the mouse re-rolls it.
///
/// [`steer::Ask::Turn`] out to [`TURN_ZONE`]: the bearing is real by then, and
/// what is not real is the *step* — from that close it lands past the cursor
/// that asked for it. So the body faces the way it was pointed and stays where
/// it is, which is also the only way a mouse can ask a character to turn.
///
/// [`steer::Ask::Walk`] beyond it.
pub(crate) fn ask_between(body: camera::WorldPixel, cursor: camera::WorldPixel) -> Option<steer::Ask> {
    let (dx, dy) = (cursor.x - body.x, cursor.y - body.y);
    let reach = f64::from(dx * dx + dy * dy);
    if reach <= DEAD_ZONE * DEAD_ZONE {
        return None;
    }
    let heading = heading_between(dx, dy)?;
    Some(match reach < TURN_ZONE * TURN_ZONE {
        true => steer::Ask::Turn(heading),
        false => steer::Ask::Walk(heading),
    })
}

/// Which of the eight ways the offset `(dx, dy)` points, and which side of it —
/// the whole of the arithmetic and none of the zones, so that
/// [`ask_between`]'s rings and this can be argued with one at a time.
pub(crate) fn heading_between(dx: i32, dy: i32) -> Option<Heading> {
    let direction = Direction::ALL.into_iter().max_by(|a, b| {
        let cosine = |direction| {
            let (sx, sy) = on_screen(direction);
            let dot = f64::from(dx) * f64::from(sx) + f64::from(dy) * f64::from(sy);
            dot / f64::from(sx * sx + sy * sy).sqrt()
        };
        cosine(*a).total_cmp(&cosine(*b))
    })?;
    let (sx, sy) = on_screen(direction);
    Some(Heading {
        direction,
        // A cross product needs no normalising, so the lean stays exact: a
        // cursor squarely on a direction's screen bearing leans neither way and
        // says so without a tolerance. The projection turns the plane without
        // flipping it, so "clockwise" means on the screen what it means on the
        // grid — see `Lean::of`.
        lean: Lean::of(sx, sy, dx, dy),
    })
}

/// One step's worth of the projection, taken from the projection.
///
/// The origin tile is arbitrary and cancels in the subtraction; it is away from
/// the map's edges only so that neither end of it has to clamp.
pub(crate) fn on_screen(direction: Direction) -> (i32, i32) {
    let origin = Point::new(1000, 1000, 0);
    let (sx, sy) = direction.step();
    let stepped = Point::new(
        (i32::from(origin.x) + sx) as u16,
        (i32::from(origin.y) + sy) as u16,
        0,
    );
    let (a, b) = (camera::project(origin), camera::project(stepped));
    (b.x - a.x, b.y - a.y)
}
