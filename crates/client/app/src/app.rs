//! [`App`]: the composition root. Every subsystem file has an `impl App`
//! block of its own — window/GPU setup in `window.rs`, packet-driven state
//! in `net_command.rs`, input-driven state in `ui_command.rs` and
//! `own_windows.rs`, read-only queries in `picking_query.rs`, the drawing
//! pipeline in `presentation.rs`, winit glue in `event_loop.rs` — and this
//! file is deliberately the thinnest of them: the struct's fields, and the
//! handful of accessors small enough that giving them a subsystem of their
//! own would be a file for one function.
//!
//! **Not a deeper decomposition.** `App` stays one struct with ~20 fields
//! rather than a struct-per-subsystem, because nearly every method above
//! reaches across more than one of those fields — `advance_replay` alone
//! touches the world, the crowd and the scope — and splitting the storage
//! along the same lines as the files would just move the borrow conflicts
//! the free functions in `presentation.rs` already dodge by staying free.
//! The split here is *where the code that touches a field lives*, not *which
//! struct the field is on*.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use openshard_client_render::animation::FRAME_DELAY;
use openshard_client_render::bench::{Scope, Script};
use openshard_client_render::camera::{Camera, TileBounds};
use openshard_client_render::composite::CompositeWorkQueue;
use openshard_client_render::control::Control;
use openshard_client_render::cutaway::Cutaway;
use openshard_client_render::lod::BlockLodSelector;
use openshard_client_render::mobiles;
use openshard_client_render::radar::{RadarCache, RadarLodSelector, RadarWorkQueue};
use openshard_protocol::direction::Facing;
use openshard_protocol::serial::Serial;
use openshard_protocol::world::Point;

use crate::chat::Chat;
use crate::diagnostics::{OccluderSurface, Route, TerrainOverlay};
use crate::net_command::project_motion;
use crate::window::Screen;
use crate::{
    GLIDE_INTERVAL, Scenario, desk, frames, graphics, input, picking, replay, resources, shell, steer,
    tooltips, windows, world,
};

/// State for the injected LOD field scenario.  The input stays in the client
/// rather than being synthesized by the desktop, so each frame follows the
/// same `Control` path as a real pan and can be reproduced in CI or locally.
pub(crate) struct LodSweep {
    pub(crate) elapsed: Duration,
    reported_lod: bool,
    dumped: bool,
    pub(crate) atlas_soak: bool,
    pub(crate) stationary_soak: bool,
    /// Unlike the deterministic offline sweeps, this keeps applying the
    /// shard's real world, movement and animation traffic and periodically
    /// compares the submitted frame with a direct LOD0 rendering.
    pub(crate) live_oracle: bool,
    /// A diagnostic-only A/B switch. It still observes all post-zoom server
    /// traffic, but leaves the rendered world at the last accepted snapshot.
    pub(crate) freeze_server: bool,
    /// The diagnostic begins only after the window has stopped changing the
    /// camera to fit its actual viewport. This makes "default zoom" a real
    /// rendered state rather than the window's startup transient.
    stationary_started: bool,
    stationary_warmup_camera: Option<Camera>,
    stationary_warmup_elapsed: Duration,
    /// `ZoomSoak` must first render at the opening zoom. Otherwise it skips
    /// the resident-atlas state that a user creates before zooming out.
    pub(crate) stationary_zoomed: bool,
    /// Exact camera value at the previous post-zoom frame. `ZoomSoak` must
    /// not silently turn into a follow/pan test after its one injected zoom.
    pub(crate) stationary_camera: Option<Camera>,
    pub(crate) next_atlas_audit: Duration,
    pub(crate) next_live_oracle: Duration,
    pub(crate) live_oracle_samples: u64,
    pub(crate) next_server_report: Duration,
    pub(crate) server_updates: ServerUpdateAudit,
}

/// Post-zoom server input observed by a stationary LOD run. This is deliberately
/// separate from animation and camera clocks: it answers whether an apparently
/// idle scene was actually rebuilt from newly arrived authoritative state.
#[derive(Debug, Default)]
pub(crate) struct ServerUpdateAudit {
    worlds: u64,
    mutations: BTreeMap<&'static str, u64>,
    movements: BTreeMap<&'static str, u64>,
    animations: u64,
    new_animations: u64,
    dropped: u64,
}

pub(crate) struct App {
    /// The optional output mixer. It hears packet feedback but never owns game
    /// state, which stays in `world`.
    pub(crate) audio: crate::audio::Audio,
    /// The client's own asset files, read once and held for the run — see
    /// [`resources::Resources`].
    pub(crate) resources: resources::Resources,
    /// The debug-view and lighting switches a person has set on this run —
    /// see [`graphics::GraphicsSettings`].
    pub(crate) graphics: graphics::GraphicsSettings,
    /// What the shard, or its absence, has said the world looks like — see
    /// [`world::WorldState`].
    pub(crate) world: world::WorldState,
    /// The shard thread's staged delivery into this event-loop-owned model.
    /// Every packet and numbered movement event is drained in wire/app order.
    pub(crate) updates: crate::link::Updates,
    /// One opt-in pause after entering the world, used only by a diagnostic
    /// harness to make mailbox backpressure observable.
    pub(crate) stall_on_update: Option<Duration>,
    /// The camera, who is allowed to move it, and what a drag has not yet spent.
    ///
    /// All of it arithmetic, and all of it in `client/render` where it can be
    /// reached by a test: this crate owns a window, a GPU and a `WorldMap`, and none
    /// of the three has anything to say about a wheel notch.
    pub(crate) control: Control,
    /// Whether the device's refusal to hold a zoom's image has been said out
    /// loud. A silently truncated target draws a smaller world into a larger
    /// rect, which looks exactly like a bug in the projection — so it is
    /// reported, and once.
    pub(crate) zoom_limit_reported: bool,
    /// The dev HUD, once there is a window to put it on.
    pub(crate) shell: Option<shell::Shell>,
    /// What the HUD looked like when the client last closed: which tab, where
    /// the dev window and the operating system's window sat, and at what scale.
    ///
    /// Read once at startup and handed to the [`shell::Shell`] when there is a
    /// window; written back in [`App::exiting`]. Held here rather than in the
    /// shell because half of it — the frame — is the *platform's* window, which
    /// the HUD does not own and cannot ask about.
    pub(crate) desk: desk::Desk,
    /// Where the player is asking to walk — the arrows, and the tile the mouse
    /// last sent the body to.
    ///
    /// A step is not sent from the input event: the operating system's
    /// auto-repeat is not a walking speed, a shard refuses a flood of steps as a
    /// speedhack, and a mouse held over the ground reports a move a pixel. One
    /// clock paces all of them. See `steer.rs`.
    pub(crate) steer: steer::Steering,
    /// Persisted movement preferences, applied at the point a step is sent.
    pub(crate) auto_open_doors: bool,
    /// The shut leaf already asked to open. A server update clears this when it
    /// swings, while keeping a locked door from receiving a use packet each
    /// walking beat.
    pub(crate) auto_opened_door: Option<Serial>,
    /// The last route assembled for the development HUD.
    ///
    /// A path search is considerably more expensive than drawing its line,
    /// especially when zooming out.  The cache is keyed by the two inputs that
    /// change as the body walks, and is cleared whenever a fresh world view
    /// changes the terrain it was planned over.
    pub(crate) route_cache: Option<RouteCache>,
    /// The terrain wash for an unchanged world and camera.
    pub(crate) terrain_cache: Option<TerrainCache>,
    /// The HUD's separate occlusion grid for an unchanged world/camera view.
    pub(crate) occluder_cache: Option<OccluderCache>,
    /// Ready minimap terrain lives with world content, never with a minimap
    /// window. Closing that window must not discard its CPU products.
    pub(crate) radar_cache: RadarCache,
    /// Bounded, coalescing requests for the minimap's terrain chunks — the
    /// radar's counterpart to [`Self::composite_work`]. Kept beside
    /// [`Self::radar_cache`] and not on [`Screen`] for the same reason: it
    /// survives a closed minimap window, and production only ever removes a
    /// key once [`RadarCache::publish`] has a complete chunk for it.
    pub(crate) radar_queue: RadarWorkQueue,
    /// Independent hysteresis state: two open windows may sit on opposite
    /// sides of an LOD boundary without changing one another's selection.
    pub(crate) minimap_radar_lod: RadarLodSelector,
    pub(crate) world_map_radar_lod: RadarLodSelector,
    /// What the last frame's radar demand and production came to, for the
    /// development HUD. Carried across the frame boundary rather than read
    /// live because the HUD is assembled before any of it happens — see
    /// [`crate::diagnostics::RadarFrame`].
    pub(crate) radar_frame: crate::diagnostics::RadarFrame,
    /// Bounded requests for immutable map-block composites.  It is updated
    /// from the camera snapshot; a future idle producer takes jobs from it,
    /// never from the camera frame itself.
    pub(crate) composite_work: CompositeWorkQueue,
    /// The persistent hysteresis state for the map-block representation.
    pub(crate) composite_lod: BlockLodSelector,
    /// What the window system and the mouse have last said — see
    /// [`input::Input`].
    pub(crate) input: input::Input,
    /// When the clock next advances a frame.
    pub(crate) next_tick: Instant,
    /// When it last did.
    ///
    /// Presentation clocks are moved by *measured* time and not by the interval
    /// that was waited for: `WaitUntil` is a floor and the compositor overshoots
    /// it, so a clock fed the nominal step would run slow by however much it did
    /// — which a stepping animation hides and a glide does not.
    pub(crate) last_advance: Instant,
    /// When the last frame was *drawn*, for the frame panel's interval.
    ///
    /// Not [`App::last_advance`], which is the clock the world is advanced on
    /// and is moved by an arriving packet as well as by a frame. Measured
    /// against that, a frame that followed a packet by a millisecond would be
    /// reported as a thousand a second, and the one number the panel exists to
    /// show — the gap between two pictures — would be the one it does not.
    pub(crate) last_frame: Instant,
    pub(crate) window: Option<Screen>,
    /// What the last frame's HUD asked for, waiting to be applied at the top of
    /// the next one.
    ///
    /// **The shell's output is the next frame's input, and that is the rule the
    /// frame's ordering rests on.** A request is laid out from a snapshot and
    /// therefore only exists after that snapshot has been taken; applying it
    /// straight away — which is what this used to do — mutates the world and the
    /// camera *between* the readers of one frame, so the overlay egui had already
    /// laid out was drawn against a camera the world pass no longer had. Held for
    /// a frame instead, every writer runs before the snapshot and there is
    /// nothing left in a frame that can move underneath it.
    ///
    /// The delay is a frame on a button press, which is the same latency every
    /// keyboard and mouse event here already has: they arrive between frames and
    /// land on the next one.
    pub(crate) pending: shell::Request,
    /// What is under the cursor, and what the last click named — see
    /// [`picking::Picking`].
    pub(crate) picking: picking::Picking,
    /// The player's own windows, and what the mouse is doing to them — see
    /// [`windows::Windows`].
    pub(crate) windows: windows::Windows,
    /// Which tooltips have already been asked for — see [`tooltips::Tooltips`].
    ///
    /// The lists themselves are not here: they arrive in packets and live in the
    /// [`WorldView`](openshard_client_net::view::WorldView) with everything else
    /// the shard said. This is only the outstanding questions, which is the one
    /// part of the exchange the wire has no packet for.
    pub(crate) tooltips: tooltips::Tooltips,
    /// The speech line — see [`Chat`].
    pub(crate) chat: Chat,
    /// The last few seconds of the eye, for the scope in the HUD.
    ///
    /// Recorded every frame the camera is advanced, from the same three values
    /// the offline bench records, so the panel's numbers and the table's are one
    /// arithmetic. See [`Scope`].
    pub(crate) scope: Scope,
    /// The last few seconds of the event loop, for the frame panel.
    ///
    /// Recorded every frame that is actually drawn, locked or not: this is a
    /// number about the loop and not about the camera. See [`frames::Frames`]
    /// for why it is not the scope.
    pub(crate) frames: frames::Frames,
    /// How many full atlas repacks this session has paid for — the eviction
    /// `AtlasError::Full` triggers, named in `docs/camera.md`: "costly and
    /// rare" was a claim nothing counted, and each one's cost otherwise reads
    /// as an ordinary heavy frame. See [`Frame::repacked`](frames::Frame) for
    /// which frame paid it.
    pub(crate) repacks: u64,
    /// The flamegraph socket, held open for as long as the client runs.
    ///
    /// Never read after it is built — dropping it is what closes the port, so
    /// holding it *is* the subscription. `None` unless `OPENSHARD_PUFFIN` asked
    /// for one; see [`profile::serve`], and [`profile`]'s docs for why the
    /// flamegraph is a separate viewer rather than a tab in this window.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) _puffin: Option<puffin_http::Server>,
    /// The bench's scenarios, built once.
    ///
    /// Held rather than rebuilt per frame because the HUD lists their names, and
    /// a scenario is a `Vec` of knots: building nine of them to print nine
    /// strings would be a small allocation storm on every frame that draws.
    pub(crate) scripts: Vec<Script>,
    /// The one being walked in the window, while it is.
    pub(crate) replay: Option<replay::Replay>,
    /// A requested presentation diagnostic, armed once the real GPU has told
    /// the control how large a world texture it can allocate.
    pub(crate) scenario: Option<Scenario>,
    /// The active state of [`Scenario::LodSweep`], if that diagnostic was
    /// requested at startup.
    pub(crate) lod_sweep: Option<LodSweep>,
    /// Diagnostic comparison of network, prediction, crowd and render positions.
    pub(crate) movement_trace: Option<crate::movement_trace::MovementTrace>,
}

/// A route snapshot and the world positions that make it valid.
pub(crate) struct RouteCache {
    pub(crate) from: Point,
    /// The place the route was planned to, height and all: two floors of one
    /// column are two destinations, and a cache keyed by the tile would hand a
    /// route to the street back for an order to the storey over it.
    pub(crate) goal: Point,
    pub(crate) route: Option<Arc<Route>>,
}

/// A terrain wash is independent of time; rebuilding it while the camera is
/// still only repeats per-tile walkability queries.
pub(crate) struct TerrainCache {
    pub(crate) bounds: TileBounds,
    pub(crate) from: Point,
    pub(crate) overlay: Arc<TerrainOverlay>,
}

/// The wireframe grid is only a different rendering of the same static
/// occlusion data while its bounds, cutaway and atlas geometry stay unchanged.
pub(crate) struct OccluderCache {
    pub(crate) bounds: TileBounds,
    pub(crate) cutaway: Cutaway,
    pub(crate) atlas_revision: Option<u64>,
    pub(crate) surfaces: Arc<[OccluderSurface]>,
}

impl App {
    /// Which reading of the shut doors this client's own steps are decided by.
    ///
    /// The two facts the rule is made of, fetched from where each of them
    /// lives: whether the shard has said this body is dead, and what the player
    /// asked of the door key. The rule itself is
    /// [`crate::world::walking_doors`], which is where it is explained and
    /// where it is tested.
    pub(crate) fn walking_doors(&self) -> openshard_map::overlay::Doors {
        crate::world::walking_doors(self.world.dead(), self.auto_open_doors)
    }

    /// The sole cutaway policy for this client's current frame.
    ///
    /// The frame and an immediate world click must ask the same question: a
    /// thing hidden by architecture in the picture cannot honestly be under
    /// the cursor. Keep this existing settings switch and map rule together;
    /// the future building renderer has its own policy and frame value.
    /// Whether there is a world under this client at all.
    ///
    /// [`Resources::grounded`](crate::resources::Resources::grounded) is where
    /// the invariant is written down; this is the name the two doors call it by.
    pub(crate) fn grounded(&self) -> bool {
        self.resources.grounded()
    }

    pub(crate) fn cutaway(&self) -> Cutaway {
        // Open with no ground, and it is not a special case: a cutaway is the
        // architecture standing between the eye and the body, and a facet
        // nobody has handed over has none. This is the one reader of the map
        // that a *packet* can reach — `App::apply_view` advances the cutaway —
        // so it answers for itself rather than being gated by a caller.
        if self.graphics.cutaway_disabled || !self.grounded() {
            Cutaway::OPEN
        } else {
            Cutaway::at(
                self.resources.map(),
                &self.resources.tiledata,
                self.world.presentation.cutaway_at,
                true,
            )
        }
    }

    /// Resolve the separate building picture once for this frame.
    ///
    /// The facet artifact selects every block of the player's *whole* building,
    /// never only the camera rectangle. The mutable cache then bakes those map
    /// blocks once and retains their stitched rooms/floors; this call performs
    /// only the small live walk through the doors as they stand now.
    pub(crate) fn interior_frame(&self) -> Option<openshard_client_render::interiors::InteriorFrame> {
        let player = self.world.presentation.cutaway_at;
        let z_slice = self.graphics.z_slice.then_some(self.graphics.z_slice_view);
        if !self.graphics.buildings {
            return z_slice
                .map(|view| openshard_client_render::interiors::InteriorFrame::z_slice(player, view));
        }
        let interiors = self.resources.interiors.as_ref()?;
        let Some(label) = interiors.building_at(player.x, player.y) else {
            let frame = openshard_client_render::interiors::InteriorFrame::outside(interiors.clone());
            return Some(z_slice.map_or(frame.clone(), |view| frame.with_z_slice(player, view)));
        };
        let surfaces = &self.resources.surfaces;
        let shape_of = |graphic| {
            surfaces
                .as_ref()
                .map_or(openshard_client_render::occlusion::Shape::UNREAD, |table| {
                    table.shape(graphic)
                })
        };
        let mut cache = self.world.presentation.interior_cache.borrow_mut();
        if !cache.buildings.contains_key(&label) {
            let blocks = self.resources.interiors.as_ref()?.blocks_for(label);
            // One terrain for both bakes, and it is the ground's own: the map,
            // the tile table and the span index over the pair travel together.
            let terrain = self.resources.terrain();
            let rooms = cache.index.stitched_with_shapes(&terrain, blocks, &shape_of)?;
            let buildings = openshard_client_render::interiors::Buildings::bake(&terrain, &rooms);
            cache
                .buildings
                .insert(label, world::InteriorBuilding { rooms, buildings });
        }
        let building = cache.buildings.get(&label)?;
        let player_cell = building.rooms.cell_at(player);
        let items = &self.world.presentation.items;
        let frame = openshard_client_render::interiors::InteriorFrame::at(
            &building.buildings,
            &building.rooms,
            player_cell,
            self.graphics.floor_view,
            |door| {
                items
                    .iter()
                    .find(|item| {
                        item.at == door.at && openshard_client_render::doors::is_door(item.displayed())
                    })
                    .map_or_else(
                        || openshard_client_render::doors::is_open(door.graphic),
                        |item| openshard_client_render::doors::is_open(item.displayed()),
                    )
            },
        )?;
        let frame = frame.with_other_buildings_hidden(interiors.clone(), label);
        Some(z_slice.map_or(frame.clone(), |view| frame.with_z_slice(player, view)))
    }

    /// Arm one ordinary rendered frame for a GPU dump. Both the visible HUD
    /// button and F12 call this so neither path can drift in naming or capture
    /// timing.
    pub(crate) fn request_frame_dump(&mut self) {
        if self.graphics.frame_dump.is_some() {
            return;
        }
        let into =
            crate::presentation::frame_dump_root().join(format!("frame-{}", self.graphics.frame_dumps));
        self.graphics.frame_dump = Some(into.clone());
        self.graphics.frame_dumps += 1;
        tracing::info!(into = %into.display(), "armed GPU frame dump");
    }

    /// Begin an injected presentation scenario after the window and GPU exist.
    pub(crate) fn begin_opening_scenario(&mut self) {
        match self.scenario.take() {
            Some(
                scenario @ (Scenario::LodSweep
                | Scenario::AtlasSoak
                | Scenario::ZoomSoak
                | Scenario::ZoomSoakFreezeServer
                | Scenario::LiveOracle),
            ) => {
                let atlas_soak = scenario == Scenario::AtlasSoak;
                let stationary_soak = matches!(
                    scenario,
                    Scenario::ZoomSoak | Scenario::ZoomSoakFreezeServer | Scenario::LiveOracle
                );
                let freeze_server = scenario == Scenario::ZoomSoakFreezeServer;
                let live_oracle = scenario == Scenario::LiveOracle;
                let mut zoom_steps = 0;
                // Both paths reach the widest zoom-out rung, 1/2×. The LOD
                // path crosses it quickly; the atlas path then pans slowly for
                // a long time, which is where a cyclic upload would eventually
                // overwrite a still-visible region.
                if !stationary_soak {
                    while zoom_steps < 3 && self.zoom(false) {
                        zoom_steps += 1;
                    }
                }
                tracing::info!(
                    zoom = %self.control.camera().zoom(),
                    zoom_steps,
                    atlas_soak,
                    stationary_soak,
                    live_oracle,
                    freeze_server,
                    "starting injected LOD/atlas sweep"
                );
                self.lod_sweep = Some(LodSweep {
                    elapsed: Duration::ZERO,
                    reported_lod: false,
                    dumped: false,
                    atlas_soak,
                    stationary_soak,
                    live_oracle,
                    freeze_server,
                    stationary_started: false,
                    stationary_warmup_camera: None,
                    stationary_warmup_elapsed: Duration::ZERO,
                    stationary_zoomed: !stationary_soak,
                    stationary_camera: None,
                    next_atlas_audit: Duration::ZERO,
                    next_live_oracle: Duration::ZERO,
                    live_oracle_samples: 0,
                    next_server_report: Duration::ZERO,
                    server_updates: ServerUpdateAudit::default(),
                });
            }
            None => {}
        }
    }

    /// Advance the LOD diagnostic through the same camera control that real
    /// input uses. At half scale an 8×8 map block enters LOD1; the two-second
    /// sweep crosses thousands of viewport pixels and repeatedly renews block
    /// ownership at its edges before it takes its diagnostic dump.
    pub(crate) fn advance_lod_sweep(&mut self, elapsed: Duration) {
        let warmup_is_settled = {
            let Some(sweep) = self.lod_sweep.as_mut() else {
                return;
            };
            if !sweep.stationary_soak || sweep.stationary_zoomed {
                true
            } else if !sweep.stationary_started {
                // A diagnostic that promises not to move must not inherit the
                // opening rig's glide towards its first camera target.
                self.control.unlock();
                sweep.stationary_started = true;
                sweep.stationary_warmup_camera = None;
                sweep.stationary_warmup_elapsed = Duration::ZERO;
                false
            } else {
                let camera = *self.control.camera();
                if sweep.stationary_warmup_camera == Some(camera) {
                    sweep.stationary_warmup_elapsed += elapsed;
                } else {
                    sweep.stationary_warmup_camera = Some(camera);
                    sweep.stationary_warmup_elapsed = Duration::ZERO;
                }
                sweep.stationary_warmup_elapsed >= Duration::from_secs(1)
            }
        };
        if !warmup_is_settled {
            return;
        }
        let delayed_zoom_due = {
            let Some(sweep) = self.lod_sweep.as_mut() else {
                return;
            };
            sweep.elapsed += elapsed;
            sweep.stationary_soak && !sweep.stationary_zoomed && sweep.elapsed >= Duration::from_secs(3)
        };
        if delayed_zoom_due {
            let mut zoom_steps = 0;
            while zoom_steps < 3 && self.zoom(false) {
                zoom_steps += 1;
            }
            let sweep = self
                .lod_sweep
                .as_mut()
                .expect("the active diagnostic owns its delayed zoom state");
            sweep.stationary_zoomed = true;
            tracing::info!(
                zoom = %self.control.camera().zoom(),
                zoom_steps,
                "injected stationary LOD soak completed default-to-max zoom"
            );
        }
        let Some(sweep) = self.lod_sweep.as_mut() else {
            return;
        };
        let speed = if sweep.atlas_soak { 120.0 } else { 2_400.0 };
        let pixels = (elapsed.as_secs_f64() * speed).round().max(1.0) as i32;
        if sweep.stationary_soak {
            // Intentionally no pan. This isolates late queue/atlas/cache
            // mutation after the default-to-max-zoom transition.
        } else if sweep.atlas_soak {
            // The reported residual pattern appears when travelling sideways:
            // that crosses the block lattice's diagonal screen projections
            // rather than only moving along one of them.
            self.control.pan(pixels, 0);
        } else {
            self.control.pan(0, -pixels);
        }
        let settle_after = if sweep.atlas_soak || sweep.stationary_soak {
            Duration::from_secs(30)
        } else {
            Duration::from_secs(2)
        };
        if !sweep.reported_lod && sweep.elapsed >= settle_after {
            sweep.reported_lod = true;
            tracing::info!(selected = ?self.composite_lod.current(), "injected LOD sweep reached steady state");
        }
        if !sweep.atlas_soak && !sweep.stationary_soak && !sweep.dumped && sweep.elapsed >= settle_after {
            sweep.dumped = true;
            self.graphics.frame_dump = Some(crate::presentation::frame_dump_root().join("lod-sweep"));
            tracing::info!("injected LOD sweep requested frame dump");
        }
    }

    /// Count post-zoom authoritative traffic before the event loop folds it
    /// into the render-facing world. The freeze variant returns true for server
    /// state updates, turning the same scenario into a controlled A/B test.
    pub(crate) fn observe_stationary_soak_update(&mut self, update: &crate::link::Update) -> bool {
        let Some(sweep) = self
            .lod_sweep
            .as_mut()
            .filter(|sweep| sweep.stationary_soak && sweep.stationary_zoomed)
        else {
            return false;
        };
        let is_server_update = match update {
            crate::link::Update::World { .. } => {
                sweep.server_updates.worlds += 1;
                true
            }
            // Split by packet *kind*, because the freeze decision is made here
            // and the fold that would answer "did it actually move" happens
            // after it — a frozen packet is never folded. The four kinds below
            // are exactly the ones `Walk::on_packet` can answer, so this counts
            // packets that *could* move the player rather than ones that did:
            // a swallowed ack or a reject a rollback already voided lands in
            // `movements` here and moves nothing.
            crate::link::Update::Mutation { packet } => {
                let counter = match crate::link::touches_the_walk(packet) {
                    true => &mut sweep.server_updates.movements,
                    false => &mut sweep.server_updates.mutations,
                };
                *counter
                    .entry(crate::movement_trace::packet_kind(packet))
                    .or_default() += 1;
                true
            }
            crate::link::Update::Animation(_) => {
                sweep.server_updates.animations += 1;
                true
            }
            crate::link::Update::NewAnimation(_) => {
                sweep.server_updates.new_animations += 1;
                true
            }
            // And not the ground: it arrives once, before the world does and so
            // long before the zoom this soak is counting traffic after. Counting
            // it as a server update would also let `freeze_server` swallow the
            // one update the client cannot draw without.
            crate::link::Update::Design(_)
            | crate::link::Update::Ground { .. }
            | crate::link::Update::Lost(_) => false,
        };
        let freeze = sweep.freeze_server && is_server_update;
        if freeze {
            sweep.server_updates.dropped += 1;
        }
        freeze
    }

    /// Log server traffic independently of the field oracle. It runs from the
    /// frame clock, so an absence of a user event is visible as an empty report.
    pub(crate) fn report_stationary_soak_server_updates(&mut self) {
        let Some(sweep) = self.lod_sweep.as_mut().filter(|sweep| {
            sweep.stationary_soak && sweep.stationary_zoomed && sweep.elapsed >= sweep.next_server_report
        }) else {
            return;
        };
        sweep.next_server_report = sweep.elapsed + Duration::from_secs(2);
        tracing::info!(
            frozen = sweep.freeze_server,
            updates = ?sweep.server_updates,
            "stationary LOD soak post-zoom server-update audit"
        );
    }

    /// Real pixels per gump pixel, which is egui's own scale.
    ///
    /// Not the window's scale factor: the interface's art is placed at
    /// coordinates egui laid out in points, so any other number here slides a
    /// window's pictures off whatever egui drew beside them — and the cursor,
    /// which arrives from `winit` in real pixels, has to come back the same way
    /// or a click lands where the picture is not.
    pub(crate) fn gump_scale(&self) -> f32 {
        self.shell
            .as_ref()
            .map(|shell| shell.pixels_per_point())
            .unwrap_or(1.0)
    }

    /// How big this client's own windows are drawn — see
    /// [`desk::WindowScale`](crate::desk::WindowScale).
    ///
    /// **From the shell while there is one, and from `self.desk` only when
    /// there is not.** `App::desk` is the file as it was *loaded*, and it is
    /// not written to again until the client is closing (`event_loop.rs` reads
    /// `Shell::desk` back at exit); the copy the dev window's slider moves is
    /// the shell's. Reading the app's here is therefore a knob that appears to
    /// do nothing and takes effect on the next launch — which is exactly what
    /// it did on the frame this was first written, and what `Shell::tuning`'s
    /// doc had already said about the lighting. A run with no shell at all —
    /// the offline map viewer — has no slider to have moved, so the loaded
    /// value is the live one there.
    pub(crate) fn window_scale(&self) -> crate::desk::WindowScale {
        self.shell
            .as_ref()
            .map_or(self.desk.window_scale, |shell| shell.window_scale())
    }

    /// Put the eye back on the body and lock it there.
    ///
    /// Where the body is *drawn* this frame, not the tile it is nominally on:
    /// a relock mid-step would otherwise land up to half a tile from the sprite
    /// and be corrected on the frame after.
    pub(crate) fn relock(&mut self) {
        self.world.presentation.player.drawn = self.world.drawn_player();
        self.control
            .relock(mobiles::gaze(&self.world.presentation.player));
    }

    /// Whether there is anybody to show a frame to: the window has the keyboard
    /// and is not covered.
    ///
    /// What the loop's pacing hangs on, and the whole of what this client does
    /// about power. A window in the background still ages its animations — the
    /// other mobile animations age on their animation clock rather than at the
    /// display's rate.
    pub(crate) fn watched(&self) -> bool {
        self.input.focused && !self.input.occluded
    }

    /// What is deciding when the next frame is drawn.
    ///
    /// Watched, it is the display and nothing else: [`App::draw`] asks for the
    /// next frame the moment it has queued one, and `PresentMode::Fifo` blocks
    /// the frame after that until the display has taken it. That is the loop
    /// every other real-time client runs, and it is what makes a still screen
    /// cost the same sixty frames a second as a moving one — which is the point,
    /// because "the frame rate drops when I stand still" was true here and read
    /// as a stall no matter how correct the reason was.
    ///
    /// Unwatched, there is nobody to show a frame to, and the timer below is
    /// what the loop falls back to. Two rates there, because there are two
    /// reasons for a frame and they are an order of magnitude apart: a body's
    /// animation steps once every [`FRAME_DELAY`] and nothing between two of
    /// those changes a pixel, while a *glide* moves a body a couple of pixels at
    /// a time and drawn on the animation clock would arrive in five visible
    /// jumps — the teleport it exists to remove, in instalments. Three reasons
    /// for the fast one and not one, because they are three independent things
    /// that move a pixel: a body mid-step, an eye still converging on one that
    /// has stopped, and a scenario waiting to deliver its next knot.
    ///
    /// The eye is the one that was missing. A rig that filters is still settling
    /// on frames where nothing else moved, and a loop that only woke for gliding
    /// bodies delivered the tail of every ease 80ms late and whole — the stutter
    /// the filter exists to remove, arriving just after it.
    pub(crate) fn pacing(&self) -> frames::Pacing {
        if self.watched() {
            return frames::Pacing::Display;
        }
        frames::Pacing::Timer(self.redraw_interval())
    }

    /// The fallback timer's interval. See [`App::pacing`] for when it is the one
    /// that decides.
    pub(crate) fn redraw_interval(&self) -> std::time::Duration {
        let moving = self.world.anyone_gliding() || self.control.settling() || self.replay.is_some();
        if moving { GLIDE_INTERVAL } else { FRAME_DELAY }
    }

    /// Start walking one of the bench's scenarios in the window.
    ///
    /// Offline only: with a shard connected the body goes where the `0x22` says
    /// it went, and a second writer would be two clients fighting over one
    /// character. The panel does not offer the buttons in that state and this
    /// refuses anyway, because a guard that only lives in a widget is a guard
    /// until somebody adds a keybinding.
    pub(crate) fn start_replay(&mut self, name: &str) {
        // The viewer, and not merely "nobody to send to": a scenario is a
        // second writer of the body's position, and a client that *lost* its
        // shard has no more right to move the character than a connected one —
        // its last position is still the shard's fact. Same question the panel
        // asks (`shell.rs`), so the widget and the guard cannot disagree.
        if !self.world.shard.is_viewer() {
            return;
        }
        let Some(script) = self.scripts.iter().find(|script| script.name == name).cloned() else {
            return;
        };
        // The height the script's own `z = 0` means here. Read once, from the
        // tile it starts on — see `Replay`'s docs on why not per tile.
        let ground = script
            .knots()
            .first()
            .map_or(self.world.motion.planning_state().position.z, |knot| {
                Self::in_bounds(
                    i32::from(knot.from.x),
                    i32::from(knot.from.y),
                    self.resources.map(),
                )
                .and_then(|tile| self.resources.map().land(tile.x, tile.y))
                .map_or(self.world.motion.planning_state().position.z, |cell| cell.z)
            });
        let replay = replay::Replay::new(script, ground);
        if let Some(start) = replay.start() {
            // Put down rather than walked, and the camera cut to it: a body
            // that strolled to the start of a scenario would be measured on the
            // way there, and an eye that eased across a facet is a second
            // motion on top of the one being looked at.
            let (body, hue) = (
                self.world.presentation.player.body,
                self.world.presentation.player.hue,
            );
            let equipment = std::mem::take(&mut self.world.presentation.player.equipment);
            let war = self
                .world
                .authoritative
                .view
                .as_ref()
                .is_some_and(|view| view.player.war);
            let facing = Facing::walking(self.world.motion.planning_state().facing.direction);
            self.world.motion.set_local(start, facing);
            let motion = self.world.motion.planning_state();
            self.world.presentation.player = self.world.presentation.crowd.snap(
                self.world.me(),
                motion.position,
                body,
                motion.facing,
                hue,
                war,
            );
            self.world.presentation.player.equipment = equipment;
            self.world.presentation.cutaway_at = motion.position;
            self.control
                .relock(mobiles::gaze(&self.world.presentation.player));
        }
        // The frames either side of a start are two different runs, and a metric
        // over both is a number about nothing.
        self.scope.clear();
        self.replay = Some(replay);
    }

    /// One frame of whatever scenario is being walked.
    ///
    /// Every knot the span covered, in order, each handed to the crowd as the
    /// packet it stands for: a crossing is glided and a jump is put down.
    pub(crate) fn advance_replay(&mut self, elapsed: std::time::Duration) {
        let Some(replay) = self.replay.as_mut() else {
            return;
        };
        let moves = replay.advance(elapsed);
        let finished = replay.finished();
        for step in moves {
            // The stance the session is actually in: a replay walks this body
            // through a recorded route, and what it is wearing or holding is
            // not part of the recording — so a scenario replayed while at war
            // is drawn at war, exactly as the same walk would be live.
            let war = self
                .world
                .authoritative
                .view
                .as_ref()
                .is_some_and(|view| view.player.war);
            if step.glided {
                self.world.motion.accept_trusted_step(step.to, step.facing);
                let me = self.world.me();
                project_motion(
                    &mut self.world.presentation.crowd,
                    me,
                    &mut self.world.presentation.player,
                    self.world.motion.render_state(),
                    war,
                );
            } else {
                self.world.motion.set_local(step.to, step.facing);
                let motion = self.world.motion.planning_state();
                let equipment = std::mem::take(&mut self.world.presentation.player.equipment);
                self.world.presentation.player = self.world.presentation.crowd.snap(
                    self.world.me(),
                    motion.position,
                    self.world.presentation.player.body,
                    motion.facing,
                    self.world.presentation.player.hue,
                    war,
                );
                self.world.presentation.player.equipment = equipment;
            }
            self.world.presentation.cutaway_at = self.world.motion.planning_state().position;
            if let Some(trace) = self.movement_trace.as_mut() {
                trace.record("replay_step", &self.world, self.control.camera());
            }
        }
        if finished {
            self.replay = None;
        }
    }

    /// A viewport that grew may have taken the world texture past what the
    /// device allows, which no zoom step asked for.
    pub(crate) fn fit_zoom_to_device(&mut self) {
        if let Some(refusal) = self.control.fit_to_device() {
            self.report_limit(format_args!(
                "a {}x{} world texture at {} is more than this GPU's {}: zooming in to {}",
                refusal.width, refusal.height, refusal.wanted, refusal.max, refusal.settled,
            ));
        }
    }

    /// One notch of the wheel, answering whether anything changed.
    ///
    /// At either end of the ladder nothing does, and zooming out can be refused
    /// by the device — which is said out loud rather than truncated.
    pub(crate) fn zoom(&mut self, inwards: bool) -> bool {
        match self.control.zoom(inwards) {
            Ok(changed) => changed,
            Err(refusal) => {
                self.report_limit(format_args!(
                    "{} would want a {}x{} world texture and this GPU allows {}: staying at {}",
                    refusal.wanted, refusal.width, refusal.height, refusal.max, refusal.settled,
                ));
                false
            }
        }
    }

    /// Say what the device refused, once.
    ///
    /// Once, because the wheel is held down and a line per notch is a wall of
    /// the same sentence — and because the second one tells nobody anything the
    /// first did not.
    pub(crate) fn report_limit(&mut self, message: std::fmt::Arguments<'_>) {
        if !self.zoom_limit_reported {
            self.zoom_limit_reported = true;
            eprintln!("{message}");
        }
    }
}
