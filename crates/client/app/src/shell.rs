//! The dev HUD: egui panels over the world.
//!
//! # What this is, and what it is deliberately not
//!
//! It is a *dev* HUD. Whether this client's real interface is egui or the
//! `0xB0` gump layer is M4's decision, and building a journal or a paperdoll
//! here would take that decision by accident — so what is here is what tells you
//! whether the client is working: the connection, the camera, and the contents
//! of the [`WorldView`](openshard_client_net::view::WorldView), which are
//! decoded and otherwise invisible.
//!
//! Nothing here reaches into `client/net` or the view beyond reading them. What
//! the panels display arrives as a [`Hud`], built by the caller, and what they
//! ask for goes back as a [`Request`] — so the panels cannot move a camera or
//! send a packet, only say that somebody pressed something.
//!
//! # Four things that are silent when wrong
//!
//! Each of these is a mistake nobody reports as a bug, so each is written down
//! where it is made:
//!
//! 1. **Colour.** The surface is deliberately non-sRGB and egui's shader assumes
//!    an sRGB target unless it is told otherwise. It reads the format and picks
//!    the gamma entry point itself, which is why the format handed to
//!    [`egui_wgpu::Renderer::new`] has to be the surface's own — the usual
//!    symptom of getting it wrong is a UI that is merely *slightly* too bright.
//! 2. **Depth.** The UI pass takes no depth attachment. The world's depth buffer
//!    ordered the world; the UI is drawn over the result of that.
//! 3. **Input.** A consumed event must reach neither the camera nor the walk
//!    keys, or a drag inside a panel pans the world underneath it.
//!    [`Shell::on_window_event`] answers that question and its caller obeys.
//!    A *consumed* event is not the whole of it, though: an event the UI took is
//!    an event the world never hears, so the world's idea of where the cursor is
//!    stops updating and keeps whatever it last saw — which is why the cursor
//!    going over a panel used to freeze the tile highlight at the panel's edge
//!    rather than put it out. [`Shell::holds_pointer`] is the positive question,
//!    asked once and answered for the whole frame, and the world reads *that*
//!    rather than inferring it from an absence of events.
//! 4. **Points against pixels.** egui lays out in logical points and the world
//!    is drawn in physical pixels, so the rect egui leaves free is multiplied by
//!    `pixels_per_point` before it becomes the camera's viewport. Getting this
//!    wrong is invisible at scale factor 1 and wrong on every HiDPI screen.

use std::collections::{
    BTreeMap,
    BTreeSet,
};
use std::time::Duration;

use openshard_client_net::action::GumpReply;
use openshard_client_render::bench::Reading;
use openshard_client_render::blit::ViewportRect;
use openshard_client_render::camera::Camera;
use openshard_client_render::facing::{
    Face,
    Prism,
};
use openshard_client_render::follow::Rig;
use openshard_client_render::light;
use openshard_client_render::solid::Cut;
use openshard_movement::sight::{
    EYE,
    Stop,
};
use openshard_protocol::feedback::{
    ActionStage,
    CombatActionKind,
    CombatActionOutcome,
    InterruptReason,
};
use openshard_protocol::gump::{
    RawButtonId,
    RawGumpId,
    RawGumpKey,
};
use openshard_protocol::localized;
use openshard_protocol::mobile::Notoriety;
use openshard_protocol::wire::{
    ClilocId,
    Graphic,
    Hue,
};
use openshard_protocol::world::RangedRange;
use openshard_uofiles::cliloc::{
    Cliloc,
    ClilocNumber,
};
use winit::window::Window;

use crate::crowd::ActionFill;
use crate::desk::{
    Desk,
    Tab,
};
use crate::diagnostics::{
    ActionBar,
    HealthBar,
    Height,
    Hud,
    Navigation,
    PickedTile,
    PriorityZ,
    Route,
    Selection,
    SightLine,
    TerrainOverlay,
};
use crate::graphics::{
    HighlightStyle,
    HighlightTarget,
};
use crate::world::{
    Shard,
    WorldState,
};

/// What the panels asked for this frame.
///
/// No longer `Copy`: one of these carries what the player typed. A request is
/// built fresh each frame and spent by the caller, so cloning it is not a thing
/// that happens on any path.
#[derive(Clone, Default, Debug)]
pub struct Request {
    /// Reply selected in the client-owned craft window. It preserves the
    /// server's existing button encoding and validation path on every page.
    pub craft_reply: Option<GumpReply>,
    /// One bounded house inventory page or result-open request.
    pub house_inventory: Option<openshard_protocol::house_inventory::HouseInventoryRequest>,
    /// Save the next fully rendered world frame and its GPU planes. This is an
    /// edge-triggered diagnostic action, not a persistent display setting.
    pub frame_dump: bool,
    /// Put the eye back on the body and lock it there.
    pub relock: bool,
    /// Let go of the body.
    pub unlock: bool,
    /// Follow with these numbers from now on.
    ///
    /// Sent on the frame a slider moved or a preset was clicked, and not every
    /// frame: the eye is not moved by a rig arriving, but a scope that cleared
    /// its trace on every frame would never have one to draw.
    pub rig: Option<Rig>,
    /// A new body ease, if the slider moved.
    pub ease: Option<crate::crowd::Ease>,
    /// Switch the terrain overlay on or off, on the frame the box was ticked.
    ///
    /// Sent on the change and not every frame, like the rig: the overlay costs a
    /// walkability lookup per visible tile and a fresh plan per frame, and that
    /// is a bill the client should only pay while somebody is looking at it.
    pub show_terrain: Option<bool>,
    /// Start or stop writing the route journal, on the frame the box was
    /// ticked.
    ///
    /// Stopping keeps the file: the lines already written are the report, and
    /// discarding them because somebody unticked a box would throw away the
    /// thing they are about to attach. See
    /// `docs/world/reference/path_journal.md`.
    pub path_journal: Option<bool>,
    /// Switch the sight overlay on or off, on the frame the box was ticked.
    ///
    /// Sent on the change like the others, though this one is the cheapest of
    /// them: one Bresenham walk of the line a shot would fly along. See
    /// `docs/combat/design_sight.md`.
    pub show_sight: Option<bool>,
    /// Name the reach the sight overlay draws its limit at, on the frame the
    /// number was changed. See
    /// [`GraphicsSettings::sight_reach`](crate::graphics::GraphicsSettings::sight_reach)
    /// for why a person names it and the shard does not.
    pub sight_reach: Option<RangedRange>,
    /// Switch the R1 interior-index overlay on or off. It is deliberately a
    /// diagnostic request: no normal world geometry consults this setting.
    pub show_interiors: Option<bool>,
    /// Enable the R2 building picture policy.
    pub buildings: Option<bool>,
    /// Replace the building policy with the diagnostic height-only band.
    pub z_slice: Option<bool>,
    /// Bounds for the diagnostic height-only band.
    pub z_slice_view: Option<openshard_client_render::interiors::ZSliceView>,
    /// The non-persistent structural floor selection.
    pub floor_view: Option<openshard_client_render::interiors::FloorView>,
    /// Which of the world's producers to draw from now on, on the frame a box
    /// was ticked — see [`openshard_client_render::frame::Draw`].
    ///
    /// The whole set and not one field, because the boxes are read against each
    /// other: "walls only" is three of them off, and a request that carried one
    /// change at a time would put a frame between the ticks with a picture nobody
    /// asked for.
    pub draw: Option<openshard_client_render::frame::Draw>,
    /// Switch the architectural cutaway on or off.
    pub cutaway_disabled: Option<bool>,
    /// Switch body-overlap transparency on or off.
    pub body_overlap_transparency_disabled: Option<bool>,
    /// Whether ambient light follows the shard's time-of-day updates.
    pub time_of_day: Option<bool>,
    /// Whether the local night-lighting comparison is enabled.
    pub night: Option<bool>,
    /// Switch the occluder wireframe on or off, on the frame the box was ticked.
    ///
    /// Sent on the change and not every frame, like the terrain overlay, and for
    /// the same reason: while it is on the client walks the map a second time
    /// each frame to rebuild the grid the lighting builds for itself.
    pub show_occluders: Option<bool>,
    /// Switch the solids view on or off, likewise. It reads the same grid, so
    /// either box being ticked is what pays for building it.
    pub show_solids: Option<bool>,
    /// Switch the world image off underneath the solids, on the frame the box
    /// was ticked — see [`Hud::solids_only`].
    pub solids_only: Option<bool>,
    /// Switch the solids view's fill between translucent and opaque, on the
    /// frame the box was ticked — see [`Hud::solids_opaque`].
    pub solids_opaque: Option<bool>,
    /// Which of [`Cut`]'s two answers either view should draw from now on, on
    /// the frame the person picked it. Both views, because they are read against
    /// each other and two grids cut differently cannot be compared.
    pub solid_cut: Option<Cut>,
    /// Start or stop a scripted walk.
    pub script: Option<ScriptRequest>,
    /// What the cursor may light from now on, on the frame the picker moved.
    pub highlight: Option<HighlightTarget>,
    /// And how an item says it, likewise.
    pub highlight_style: Option<HighlightStyle>,
    /// How long a window the scope should keep from now on.
    ///
    /// Four seconds holds a reversal and is wrong for both ends of the range a
    /// scenario can be: a `teleport` is over in one, and a `back_and_forth`
    /// worth reading is longer than the window that shows it.
    pub scope_span: Option<Duration>,
    /// A hand edit to a graphic's prism, on the frame a slider in the
    /// selected-tile panel moved.
    ///
    /// Sandbox only — this authors the running client's own in-memory table
    /// and repacks so the shape redraws immediately, and nothing here writes
    /// to disk. See `App::apply`.
    pub authored_prism: Option<(Graphic, Prism)>,
    /// New effect and music gains from the Audio tab.
    pub audio: Option<crate::desk::Audio>,
    /// Run without holding shift.
    pub always_run: Option<bool>,
    /// Use a shut door when a movement step reaches it.
    pub auto_open_doors: Option<bool>,
    /// Build the coarse navigation graph again, on the frame the button was
    /// pressed.
    ///
    /// Edge-triggered like [`Self::frame_dump`], and for the same reason: it
    /// starts work rather than setting a state, so a `bool` that stayed true
    /// would start it again on every frame after.
    pub rebake_navigation: bool,
    /// Enter or leave map-editor mode under the authority held when this
    /// request is applied.
    pub editor_mode: Option<bool>,
    /// Publish the current editor draft after the event-loop owner rechecks
    /// authority, facet and revision.
    pub commit_map_edit: bool,
    /// Create this item in the staff member's backpack.
    pub create_item: Option<AdminItemRequest>,
    /// Raise a server-side target cursor for this animal catalogue entry.
    pub place_creature: Option<u16>,
    /// Stamp a mark into the combat recorder, saying this.
    ///
    /// Edge-triggered like [`frame_dump`](Self::frame_dump): a mark is a thing
    /// that *happens* at an instant, and a `String` that stayed set would stamp
    /// one on every frame after.
    pub mark_combat: Option<String>,
    /// Write the combat recorder out to a file beside the client.
    pub save_combat_log: bool,
    /// Throw away what the combat recorder has kept.
    pub clear_combat_log: bool,
    /// Type this staff command for the player, prefix and all (`.dummy`).
    ///
    /// A button, and the same sentence a person could have typed: the shard has
    /// exactly one implementation of every command and exactly one authority
    /// gate on the way in, so a panel that reached past speech to a private
    /// entry point would be a second place for the two to disagree. What the
    /// button buys is not a shortcut past the shard, it is not having to
    /// remember the word.
    pub staff_command: Option<String>,
}

/// What the script picker asked for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ScriptRequest {
    /// Walk this scenario from its start.
    Run(&'static str),
    /// Stop wherever it got to.
    Stop,
}

/// One validated submission from the F1 administrator item panel.
///
/// The ordinary catalogue names durable gameplay identity. Raw client art is
/// retained only in the explicitly labelled legacy/debug form.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AdminItemRequest {
    Kind {
        kind:      openshard_protocol::item_kind::ItemKindId,
        material:  Option<openshard_protocol::item_kind::MaterialId>,
        amount:    u16,
        stackable: bool,
    },
    LegacyArt {
        graphic:   u16,
        hue:       u16,
        amount:    u16,
        stackable: bool,
    },
}

/// The two egui compositions recorded into one GPU command buffer.
///
/// They have independent contexts and renderers.  A renderer owns mutable
/// vertex and index streams, so sharing it would let the later HUD upload
/// replace the already-encoded world-overlay mesh before the GPU executes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EguiLayer {
    WorldOverlay,
    Hud,
}

impl EguiLayer {
    const fn pass_label(self) -> &'static str {
        match self {
            Self::WorldOverlay => "egui: world overlay",
            Self::Hud => "egui: HUD",
        }
    }
}

/// egui, and the two crates that put it on a window and on a GPU.
pub struct Shell {
    context: egui::Context,
    /// A deliberately separate egui frame for things anchored in the world.
    ///
    /// It is recorded after the world has been drawn and submitted before the
    /// gump layer.  Keeping it out of the HUD frame makes the composition
    /// order a property of the renderer, not an accident of egui painter
    /// creation order.
    world_overlay_context: egui::Context,
    world_overlay_output: Option<egui::FullOutput>,
    state: egui_winit::State,
    /// The HUD's stream; intentionally distinct from `world_overlay_renderer`.
    hud_renderer: egui_wgpu::Renderer,
    /// The world-overlay pass and the HUD are encoded into one command buffer.
    /// They therefore cannot share egui's mutable vertex/index streams: the
    /// HUD upload would otherwise replace the health-bar mesh before the GPU
    /// executes the earlier world-overlay render pass.
    world_overlay_renderer: egui_wgpu::Renderer,
    /// Textures may be destroyed only after the command buffer that last used
    /// them has been submitted. `egui_wgpu::Renderer::free_texture` destroys
    /// the underlying wgpu texture rather than merely dropping a handle.
    hud_textures_to_free: Vec<egui::TextureId>,
    world_overlay_textures_to_free: Vec<egui::TextureId>,
    /// Where the world may be drawn: what [`egui::CentralPanel`] left free,
    /// converted to physical pixels. Held between frames because the camera is
    /// resized from it before the next frame's UI has run.
    viewport: ViewportRect,
    /// What the last [`Shell::run`] asked to be woken after.
    repaint_after: std::time::Duration,
    /// What the HUD remembers between runs: the tab in front, where the dev
    /// window sits, whether it is open, and the scale.
    ///
    /// Lives here because it is what the UI is holding between frames — and is read back out by [`Shell::desk`] when
    /// the app has a file to write. The `window` field is the one part of it this
    /// never touches: the operating system's window is the app's, not the HUD's.
    desk: Desk,
    /// Decoded static-art thumbnails for the small page currently visible in
    /// the F1 administrator browser. Keeping this in the shell lets egui own
    /// the GPU textures while the source art stays in [`Resources`].
    item_catalogue: ItemArtCatalogue,
    /// One client-owned craft window. Catalogue, workbench and recipe details
    /// are pages of this same state rather than independent floating windows.
    crafting: CraftWindowPanel,
    /// Client-owned search/filter/page state for permissioned house storage.
    house_inventory: HouseInventoryPanel,
}

impl Shell {
    /// Build the HUD for a window and a surface format.
    ///
    /// `format` must be the surface's own: egui picks its fragment entry point
    /// from whether that format is sRGB, and a guess here is the "slightly too
    /// bright" failure in the module docs.
    ///
    /// `desk` is what the last run left behind — see [`crate::desk`]. The scale
    /// is applied here rather than in the first frame's layout because
    /// `zoom_factor` is what egui lays *everything* out against, and a frame at
    /// the wrong one is a frame the window's saved rect is placed against the
    /// wrong coordinate system.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, window: &Window, desk: Desk) -> Self {
        let context = egui::Context::default();
        let world_overlay_context = egui::Context::default();
        let world_overlay_renderer = egui_wgpu::Renderer::new(
            device,
            format,
            egui_wgpu::RendererOptions {
                depth_stencil_format: None,
                ..Default::default()
            },
        );
        // On top of the monitor's own density, which `egui_winit::State` is given
        // below and which nothing here saves.
        context.set_zoom_factor(desk.zoom.hud_scale_factor());
        let state = egui_winit::State::new(
            context.clone(),
            egui::ViewportId::ROOT,
            window,
            Some(window.scale_factor() as f32),
            None,
            Some(device.limits().max_texture_dimension_2d as usize),
        );
        let hud_renderer = egui_wgpu::Renderer::new(
            device,
            format,
            egui_wgpu::RendererOptions {
                // No depth attachment: see the module docs.
                depth_stencil_format: None,
                ..Default::default()
            },
        );
        let size = window.inner_size();
        Self {
            context,
            world_overlay_context,
            world_overlay_output: None,
            state,
            hud_renderer,
            world_overlay_renderer,
            hud_textures_to_free: Vec::new(),
            world_overlay_textures_to_free: Vec::new(),
            viewport: ViewportRect {
                x:      0,
                y:      0,
                width:  size.width.max(1),
                height: size.height.max(1),
            },
            // Until the first frame has run there is nothing to wait for; the
            // animation clock is what wakes the loop.
            repaint_after: std::time::Duration::MAX,
            desk,
            item_catalogue: ItemArtCatalogue::new(),
            crafting: CraftWindowPanel::default(),
            house_inventory: HouseInventoryPanel::new(),
        }
    }

    /// What the HUD would have remembered if the client stopped now.
    ///
    /// The `window` field is [`None`] here whatever it was loaded as: only the
    /// caller knows where the operating system's window ended up, and this is a
    /// snapshot of the HUD's half of the file rather than of the file.
    pub fn desk(&self) -> Desk {
        Desk {
            window: None,
            ..self.desk.clone()
        }
    }

    /// What the lighting is turned to right now — the Light tab's own numbers,
    /// as the renderer wants them.
    ///
    /// Read once a frame by the thing that collects the lighting, and read from
    /// *this* `Desk` and not the app's for the same reason [`Shell::toggle_dev`]
    /// writes to this one: the app's copy is what was loaded at startup and what
    /// will be saved at exit, and the sliders a person is dragging are here.
    pub fn tuning(&self) -> light::Tuning {
        self.desk.light.tuning()
    }

    /// What the HUD chat box is turned to right now — the Chat tab's own
    /// numbers, [`Shell::tuning`]'s own reason for being read from here rather
    /// than the app's copy.
    pub fn chat(&self) -> crate::desk::Chat {
        self.desk.chat
    }

    /// The text sizes the Chat tab is currently showing.
    ///
    /// This is deliberately read from the shell's [`Desk`], not the app's
    /// startup copy: a drag changes this value immediately, while the latter
    /// is only refreshed when the client exits and writes `client_ui.ron`.
    pub fn fonts(&self) -> crate::desk::FontSizes {
        self.desk.fonts
    }

    /// The face F1 has chosen for this run. Availability is the app's fact —
    /// this is only the remembered preference.
    pub fn font_face(&self) -> crate::desk::FontFace {
        self.desk.font_face
    }

    /// How big the client's own windows are drawn right now — the Windows
    /// tab's own number, and [`Shell::tuning`]'s reason again.
    ///
    /// Read once a frame by the draw pass and once per input by the pointer;
    /// both go through `App::window_scale`, which is the one place this or the
    /// app's own copy is chosen between.
    pub fn window_scale(&self) -> crate::desk::WindowScale {
        self.desk.window_scale
    }

    /// Which of the two status frames the player has chosen — the live copy,
    /// for [`Self::window_scale`]'s reason.
    pub fn status_frame(&self) -> crate::desk::StatusFrame {
        self.desk.status_frame
    }

    /// Show or hide the dev window — the strip's `dev` toggle, reached from a key.
    ///
    /// It has to come through here, and not through the app's own [`Desk`]: the
    /// one the panels are laid out against is *this* one. The app's copy is what
    /// was loaded at startup and what will be written at exit, and between those
    /// two moments nothing draws from it, so a key that flipped it would change a
    /// value nobody reads and take effect on the next launch.
    pub fn toggle_dev(&mut self, authority: openshard_protocol::access::AccessLevel) {
        if !self.desk.open && authority.allows(openshard_commands::StaffCommand::AUTHORITY) {
            // The staff tab is first in the bar as well, but selecting it here
            // makes F1 a direct way into its one frequent testing workflow.
            self.desk.tab = Tab::Admin;
        }
        self.desk.open = !self.desk.open;
    }

    /// Show or hide the current-house inventory search (Ctrl+I).
    pub fn toggle_house_inventory(&mut self) {
        self.house_inventory.open = !self.house_inventory.open;
    }

    /// Record an event for egui and answer whether a *visible, active* egui
    /// control owns it.
    ///
    /// `State::on_window_event` reports whether its internal input collector
    /// accepted an event.  That is deliberately broader than ownership: a
    /// stale or off-screen egui widget can accept a key while the player is
    /// looking at the world.  Letting that implementation detail stop the
    /// game's window manager was how an invisible shop could make chat and
    /// paperdolls appear dead.  Egui still receives every event so its state
    /// stays coherent; it may block the game only when it explicitly says it
    /// owns the corresponding input channel.
    ///
    /// A `true` here means the camera and the walk keys must not see the event.
    pub fn on_window_event(&mut self, window: &Window, event: &winit::event::WindowEvent) -> bool {
        // One key egui is not merely refused but never told about — see
        // [`crate::keyboard::egui_may_see`] for the trap that costs, and note
        // that returning early here means `state` never records it either, which
        // is the point: focus navigation cannot run on an event it never got.
        if let winit::event::WindowEvent::KeyboardInput { event, .. } = event {
            if let winit::keyboard::PhysicalKey::Code(code) = event.physical_key {
                if !crate::keyboard::egui_may_see(code) {
                    return false;
                }
            }
        }
        let consumed = self.state.on_window_event(window, event).consumed;
        if !consumed {
            return false;
        }
        match event {
            winit::event::WindowEvent::KeyboardInput { .. } | winit::event::WindowEvent::Ime(_) => {
                self.holds_keyboard()
            }
            winit::event::WindowEvent::CursorMoved { .. }
            | winit::event::WindowEvent::MouseInput { .. }
            | winit::event::WindowEvent::MouseWheel { .. }
            | winit::event::WindowEvent::Touch { .. } => self.holds_pointer(),
            // Lifecycle events do not belong to either interaction layer.
            _ => false,
        }
    }

    /// Whether the keyboard belongs to the UI rather than to the game.
    ///
    /// **A text field**, and not egui's own `egui_wants_keyboard_input`, which
    /// is literally "some widget has the focus" — a focused button or slider
    /// answers `true` to that, and `Tab` is what hands out the focus. See
    /// [`crate::keyboard`]'s module docs for the whole of that defect.
    ///
    /// The staff item creator in F1 uses `egui::TextEdit`; its focused field is
    /// therefore the one case that correctly holds the keyboard here. The
    /// question remains live rather than a panel-specific flag, so future text
    /// fields get the same ownership rule automatically.
    pub fn holds_keyboard(&self) -> bool {
        self.context.text_edit_focused()
    }

    /// Whether the pointer belongs to the UI rather than to the world.
    ///
    /// Over a panel or a window, or holding a widget that has been dragged out
    /// from under itself — in either case the world must not read the cursor: no
    /// tile is picked, nothing is highlighted, and a click is the UI's. The
    /// answer is egui's own from the frame just laid out, which is the same
    /// answer `on_window_event` consumes pointer events by, so the two can never
    /// disagree about who owns the mouse.
    pub fn holds_pointer(&self) -> bool {
        self.context.is_pointer_over_egui() || self.context.egui_is_using_pointer()
    }

    /// How long the UI is content to wait before it wants drawing again.
    ///
    /// An animating widget asks for the next frame soon and a still one asks
    /// for eternity, so the event loop's deadline is the earlier of this and the
    /// animation clock's — two terms, because they are two independent reasons
    /// for a frame.
    pub fn repaint_after(&self) -> std::time::Duration {
        self.repaint_after
    }

    /// The rectangle of the surface the world may be drawn into.
    ///
    /// Physical pixels, and not the window: a docked panel shrinks it, which is
    /// the same path a resize already takes.
    pub fn viewport(&self) -> ViewportRect {
        self.viewport
    }

    /// Lay the panels out, and hand back what they asked for.
    ///
    /// Splitting this from [`Shell::paint`] is what lets the camera be resized
    /// from the viewport this leaves *before* the world is drawn into it: a
    /// frame that laid out its UI after drawing the world would size the world
    /// from the previous frame's panels.
    pub fn run(&mut self, window: &Window, frame: ShellFrame<'_>) -> (Request, egui::FullOutput) {
        let ShellFrame {
            hud,
            camera,
            world,
            art,
            tiledata,
            hue_ramp,
            cliloc,
            skill_names,
            map_editor,
            authority,
        } = frame;
        let input = self.state.take_egui_input(window);
        let mut request = Request::default();
        // What the panels leave behind, taken from the root `Ui` *after* they
        // have claimed their edges. That rectangle is the world's viewport, so
        // a docked panel shrinks the world and a floating window sits over it.
        let mut free = egui::Rect::from_min_size(egui::Pos2::ZERO, self.context.content_rect().size());
        let desk = &mut self.desk;
        let output = self.context.run_ui(input, |ui| {
            request = layout(
                ui,
                LayoutFrame {
                    shell: ShellFrame {
                        hud,
                        camera,
                        world,
                        art,
                        tiledata,
                        hue_ramp,
                        cliloc,
                        skill_names,
                        map_editor: &mut *map_editor,
                        authority,
                    },
                    item_catalogue: &mut self.item_catalogue,
                    crafting: &mut self.crafting,
                    house_inventory: &mut self.house_inventory,
                    desk,
                },
            );
            // **No party invitation here any more.** It was an `egui::Window`
            // with a Join and a Decline on it, drawn over the gump layer with
            // its own font and its own frame — and, because egui is painted on
            // top and claims a click before any of this client's own windows are
            // offered it, with its own idea of what "the window under the
            // pointer" means. It is `panes::confirm` now: the reference client's
            // own `0x0816` plate, opened and taken away by
            // `reconcile_own_windows` off the same `party.invited_by` this used
            // to read, and clicked by the one walk every other window is clicked
            // by. See `crates/client/render/src/confirm.rs`.
            //
            // **And no party roster either.** It listed the members and carried
            // an Add and a Leave, drawn from `!members.is_empty()`; it is
            // `panes::party` now — the reference's own `0x0A28` manifest, opened
            // and taken away by `reconcile_own_windows` off the same roster.
            // See `crates/client/render/src/party.rs`.
            // **No amount picker here any more.** It was an `egui::Window` with
            // a `DragValue` on it, anchored to the middle of the screen while
            // the pointer was wherever the drag had got to — and, for the
            // invitation's reason, answered through a window system that is not
            // the one every other window of this client is clicked by. It is
            // `panes::split` now: the reference client's own `SplitMenuGump`,
            // put up under the pointer by `App::open_split_prompt` and routed
            // back by the same `Windows::prompt` record as before. See
            // `crates/client/render/src/split.rs`.
            free = ui.available_rect_before_wrap();
        });
        // The scale lives in egui — Ctrl+`+` is egui's own shortcut and writes it
        // there — so it is read back rather than tracked, every frame, where it
        // cannot drift from what the UI was actually laid out at.
        self.desk.zoom = crate::desk::Zoom::new(self.context.zoom_factor());
        self.state
            .handle_platform_output(window, output.platform_output.clone());
        self.repaint_after = output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map_or(std::time::Duration::MAX, |viewport| viewport.repaint_delay);

        // Points to physical pixels: the one conversion that is invisible at
        // scale factor 1 and wrong on every HiDPI screen.
        let scale = self.context.pixels_per_point();
        let size = window.inner_size();
        let clamp = |value: f32, limit: u32| (value.max(0.0) as u32).min(limit);
        let x = clamp(free.min.x * scale, size.width);
        let y = clamp(free.min.y * scale, size.height);
        self.viewport = ViewportRect {
            x,
            y,
            width: clamp(free.width() * scale, size.width - x),
            height: clamp(free.height() * scale, size.height - y),
        };
        // World-attached UI is a separate composition layer. It has no input
        // and no widgets, but gets the same point-to-pixel transform as the
        // HUD so anchors line up on HiDPI displays and at every HUD zoom.
        self.world_overlay_context
            .set_pixels_per_point(self.context.pixels_per_point());
        let world_overlay = self.world_overlay_context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    self.context.content_rect().size(),
                )),
                ..Default::default()
            },
            |ui| draw_world_overlays(ui.ctx(), hud, camera, free, map_editor),
        );
        debug_assert_eq!(
            world_overlay.pixels_per_point,
            self.world_overlay_context.pixels_per_point(),
            "the world-overlay output must be tessellated at its own context's scale"
        );
        self.world_overlay_output = Some(world_overlay);
        (request, output)
    }

    /// Draw what [`Shell::run`] produced, over whatever is already on the
    /// surface.
    #[allow(clippy::too_many_arguments)]
    /// Real pixels per gump pixel: egui's own scale, which the interface's art
    /// is drawn at too.
    ///
    /// Read back off the context rather than tracked, so it cannot drift from
    /// what the panels were actually laid out at — and it is the same number the
    /// gump pass is handed, because the cursor is measured in one space and both
    /// halves of the interface are drawn in it. See `App::gump_scale`.
    pub fn pixels_per_point(&self) -> f32 {
        self.context.pixels_per_point()
    }

    pub fn paint(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        output: egui::FullOutput,
        size_in_pixels: [u32; 2],
    ) {
        let textures_to_free = Self::paint_output(
            &mut self.hud_renderer,
            &self.context,
            EguiLayer::Hud,
            device,
            queue,
            encoder,
            target,
            output,
            size_in_pixels,
        );
        self.hud_textures_to_free.extend(textures_to_free);
    }

    /// Finish an egui frame when the surface supplied no texture to paint.
    ///
    /// Texture commands are state changes, not part of the discarded picture:
    /// egui will not repeat a font-atlas upload merely because the swapchain was
    /// lost or occluded. Apply them to both renderers even though this frame's
    /// shapes have nowhere to go. Consuming the commands also fulfils
    /// `TexturesDelta`'s contract that no delta may be dropped unhandled.
    pub fn finish_without_painting(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        hud_output: egui::FullOutput,
    ) {
        let world_overlay_output = self.world_overlay_output.take();
        debug_assert!(
            world_overlay_output.is_some(),
            "world overlays must be finished once after Shell::run"
        );
        if let Some(output) = world_overlay_output {
            Self::finish_output_without_painting(&mut self.world_overlay_renderer, device, queue, output);
        }
        Self::finish_output_without_painting(&mut self.hud_renderer, device, queue, hud_output);
    }

    /// Paint the world-overlay layer between the world and client windows.
    /// Calling this after the world pass and before gumps is the only legal
    /// placement for health bars, routes and diagnostic markers.
    pub fn paint_world_overlays(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        size_in_pixels: [u32; 2],
    ) {
        let output = self.world_overlay_output.take();
        debug_assert!(
            output.is_some(),
            "world overlays must be encoded once after Shell::run and before HUD painting"
        );
        if let Some(output) = output {
            let textures_to_free = Self::paint_output(
                &mut self.world_overlay_renderer,
                &self.world_overlay_context,
                EguiLayer::WorldOverlay,
                device,
                queue,
                encoder,
                target,
                output,
                size_in_pixels,
            );
            self.world_overlay_textures_to_free.extend(textures_to_free);
        }
    }

    /// Release textures retired by the egui frames in the command buffer the
    /// caller has just submitted.
    pub fn finish_submission(&mut self) {
        for id in self.world_overlay_textures_to_free.drain(..) {
            self.world_overlay_renderer.free_texture(&id);
        }
        for id in self.hud_textures_to_free.drain(..) {
            self.hud_renderer.free_texture(&id);
        }
    }

    // egui's own paint call, and these are egui's own nine arguments: the
    // renderer, the context, the device pair, the encoder and target, and the
    // output to draw. Nothing here is ours to group.
    #[allow(clippy::too_many_arguments)]
    fn paint_output(
        renderer: &mut egui_wgpu::Renderer,
        context: &egui::Context,
        layer: EguiLayer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        mut output: egui::FullOutput,
        size_in_pixels: [u32; 2],
    ) -> Vec<egui::TextureId> {
        let pixels_per_point = output.pixels_per_point;
        debug_assert_eq!(
            pixels_per_point,
            context.pixels_per_point(),
            "{} used an output from a different egui context",
            layer.pass_label(),
        );
        let jobs = context.tessellate(output.shapes, pixels_per_point);
        Self::upload_textures(renderer, device, queue, &mut output.textures_delta);
        let descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels,
            pixels_per_point,
        };
        renderer.update_buffers(device, queue, encoder, &jobs, &descriptor);

        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(layer.pass_label()),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view:           target,
                depth_slice:    None,
                resolve_target: None,
                ops:            wgpu::Operations {
                    // Over the world, not instead of it.
                    load:  wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            multiview_mask: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        renderer.render(&mut pass.forget_lifetime(), &jobs, &descriptor);

        let textures_to_free = output.textures_delta.free.drain().collect();
        textures_to_free
    }

    fn finish_output_without_painting(
        renderer: &mut egui_wgpu::Renderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mut output: egui::FullOutput,
    ) {
        Self::upload_textures(renderer, device, queue, &mut output.textures_delta);
        Self::free_textures(renderer, &mut output.textures_delta);
    }

    #[expect(clippy::iter_over_hash_type)]
    fn upload_textures(
        renderer: &mut egui_wgpu::Renderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        textures: &mut egui::TexturesDelta,
    ) {
        for (id, deltas) in textures.set.drain() {
            for delta in deltas {
                renderer.update_texture(device, queue, id, &delta);
            }
        }
    }

    #[expect(clippy::iter_over_hash_type)]
    fn free_textures(renderer: &mut egui_wgpu::Renderer, textures: &mut egui::TexturesDelta) {
        for id in textures.free.drain() {
            renderer.free_texture(&id);
        }
    }
}

/// The world-side inputs used to lay out one shell frame.
pub struct ShellFrame<'a> {
    /// Read-only facts assembled for the HUD.
    pub hud:         &'a Hud,
    /// The camera whose viewport the shell may resize.
    pub camera:      Camera,
    /// The client's current projection of the world.
    pub world:       &'a WorldState,
    /// Installed static art used by catalogue panels.
    pub art:         &'a openshard_uofiles::art::Art,
    /// Installed tile metadata used by catalogue panels.
    pub tiledata:    &'a openshard_tiles::TileData,
    /// The installed hue ramps, used by the staff dye palette and its preview.
    pub hue_ramp:    &'a openshard_client_render::hue::HueRamp,
    /// Localized recipe and skill labels supplied by the client's install.
    pub cliloc:      Option<&'a Cliloc>,
    /// Installed skill names used by the staff skill tester.
    pub skill_names: &'a openshard_uofiles::skills::Skills,
    /// Mutable map-editor session shown by staff panels.
    pub map_editor:  &'a mut crate::editor_mode::MapEditor,
    /// Authority granted by the connected shard.
    pub authority:   openshard_protocol::access::AccessLevel,
}

struct LayoutFrame<'a> {
    shell:           ShellFrame<'a>,
    item_catalogue:  &'a mut ItemArtCatalogue,
    crafting:        &'a mut CraftWindowPanel,
    house_inventory: &'a mut HouseInventoryPanel,
    desk:            &'a mut Desk,
}

/// The panels, and the server's own dialogs.
///
/// Deliberately absent: the paperdoll, containers, and the speech line and
/// journal, which are now `App::chat`'s and drawn through
/// `openshard_client_render::gump::GumpRenderer` — see `App::draw` — rather
/// than egui's. Building the paperdoll and containers here would decide M4
/// without arguing it; the speech line already had that argument, in the
/// commit that moved it off this file.
fn layout(root: &mut egui::Ui, frame: LayoutFrame<'_>) -> Request {
    let LayoutFrame {
        shell:
            ShellFrame {
                hud,
                camera,
                world,
                art,
                tiledata: _,
                hue_ramp,
                cliloc,
                skill_names,
                map_editor,
                authority,
            },
        item_catalogue,
        crafting,
        house_inventory,
        desk,
    } = frame;
    let mut request = Request::default();
    let staff = authority.allows(openshard_commands::StaffCommand::AUTHORITY);
    // egui 0.35 hands the frame a root `Ui`: panels are shown inside it and
    // what is left of it is the world's viewport, while windows float over the
    // context. The two are laid out here in that order for exactly that reason.
    let context = root.ctx().clone();

    egui::Panel::top("status").show(root, |ui| {
        ui.horizontal(|ui| {
            // The shard has the last word on this line. A connection that has
            // ended says why here rather than only in the terminal, and — the
            // part that mattered — it stops reading "in world" over a socket
            // that is closed.
            match &world.shard {
                Shard::Lost(reason) => ui.label(format!("disconnected: {reason}")),
                Shard::Viewer | Shard::Live(_) => ui.label(&world.connection),
            };
            ui.separator();
            match world.authoritative.view.as_ref().map(|view| view.player.serial) {
                Some(serial) => ui.label(format!("serial {serial}")),
                None => ui.label("no serial"),
            };
            ui.separator();
            let at = world.motion.hud_state().predicted.position;
            ui.label(format!("{}, {}, {}", at.x, at.y, at.z));
            ui.separator();
            ui.label(match hud.ping {
                Some(ping) => match hud.ping_app_delivery {
                    Some(delivery) => format!(
                        "step RTT {} ms + app {} ms",
                        ping.as_millis(),
                        delivery.as_millis()
                    ),
                    None => format!("step RTT {} ms", ping.as_millis()),
                },
                None => "step RTT —".to_owned(),
            })
            .on_hover_text(
                "The first value is client → shard → client-net. The second is how long the decoded acknowledgement waited for the window event loop.",
            );
            ui.separator();
            // What the frame cost to *build*, and not how long it took: paced by
            // the display, every frame takes a refresh interval whatever it was
            // doing, and the strip would read 16.7ms on an idle client for ever.
            // Milliseconds with one decimal, because a frame is a millisecond or
            // two here and an integer would read as zero.
            ui.label(format!(
                "{:.1} ms",
                hud.perf
                    .frames
                    .last()
                    .map_or(0.0, |frame| frame.build().as_secs_f64() * 1_000.0)
            ));
            ui.separator();
            // The coarse graph, which is the difference between a click that
            // routes out of a building and one that is refused — see
            // [`Navigation`]. On the always-there strip rather than in a tab
            // because the state a person needs to see is the *transient* one: a
            // graph that is still being built explains a refusal that will stop
            // happening on its own in a few seconds, and a tab nobody has open
            // cannot say so.
            match &hud.navigation {
                Navigation::Absent => {
                    ui.label(egui::RichText::new("nav: none").color(ui.visuals().warn_fg_color))
                }
                Navigation::Baking { since } => ui.label(
                    egui::RichText::new(format!("nav: building {:.0}s…", since.elapsed().as_secs_f64()))
                        .color(ui.visuals().warn_fg_color),
                ),
                Navigation::Ready { nodes, .. } => ui.label(format!("nav: {nodes} nodes")),
            }
            .on_hover_text(match &hud.navigation {
                Navigation::Absent => "No coarse graph: a route further than a few tiles is the \
                                       bounded search alone, so a click that has to leave a \
                                       building can be refused. The World tab bakes one."
                    .to_owned(),
                Navigation::Baking { .. } => "Building the coarse graph. Long routes are the \
                                              bounded search alone until it lands."
                    .to_owned(),
                Navigation::Ready {
                    regions,
                    nodes,
                    edges,
                    path,
                } => format!(
                    "{regions} regions, {nodes} nodes, {edges} edges\n{}",
                    path.display()
                ),
            });
            // And the standing order's own refusal, while there is one. Beside
            // the graph on purpose: "nav: none" and "too far to plot a route"
            // are two halves of one story more often than not.
            if let Some(refusal) = hud.refusal {
                ui.separator();
                ui.label(egui::RichText::new(refusal.text()).color(ui.visuals().warn_fg_color));
            }
            // The dev window's own switch, on the one strip that is always
            // there. A window with a close button and no way back is a window
            // you close once and then relaunch the client to get back — which is
            // exactly the state this whole file is here to stop being normal.
            ui.separator();
            if ui
                .selectable_label(desk.open, "dev")
                .on_hover_text("F1")
                .clicked()
            {
                desk.open = !desk.open;
            }
            // Hidden rather than disabled for ordinary players: editor mode is
            // staff vocabulary, just like staff commands in the speech-line
            // completer. The shard remains the authority for every eventual
            // commit; this gate only stops the client offering an unusable UI.
            if crate::editor_mode::MapEditor::available_to(authority)
                && ui.selectable_label(map_editor.active(), "map editor").clicked()
            {
                request.editor_mode = Some(!map_editor.active());
            }
            // This stays on the ordinary status strip rather than hidden in a
            // developer tab: an intermittent roof/LOD artifact must be saved
            // at the moment it is seen, not after navigating a panel. F12 is
            // the keyboard twin for the same action.
            if ui
                .button("capture GPU dump")
                .on_hover_text("Save this world frame's GPU planes and render inputs (F12)")
                .clicked()
            {
                request.frame_dump = true;
            }
            // The HUD's scale, shown because it is remembered: a client that
            // reopened at yesterday's zoom and does not say so reads as a client
            // that is rendering at the wrong size. Ctrl+`+` / Ctrl+`-` /
            // Ctrl+`0` are egui's own — see `Options::zoom_with_keyboard` — and
            // this is the readout, not the control.
            ui.label(format!("{}%", (ui.ctx().zoom_factor() * 100.0).round()));
        });
    });

    if map_editor.active() && crate::editor_mode::MapEditor::available_to(authority) && map_editor.panel(root)
    {
        request.commit_map_edit = true;
    }

    // One window, five tabs. Five floating windows is five things to place, and
    // — with nothing saving them — five things to place again on every launch;
    // what a dev HUD is for is reading, and arranging it was most of the cost of
    // using it. The panels themselves are untouched: each tab is the body one of
    // those windows had, so this is a change of furniture and not of content.
    let mut open = desk.open;
    let window = egui::Window::new("Dev").open(&mut open);
    // Where it was left, and a first run's defaults when it has never been
    // placed. `default_*` and not `current_*`: after the first frame egui's own
    // memory is what moves the window, and forcing the saved rect every frame
    // would make it undraggable.
    let window = match desk.panel {
        Some(panel) => {
            window
                .default_pos([panel.x, panel.y])
                .default_size([panel.width, panel.height])
        }
        None => window.default_pos([16.0, 48.0]).default_size([360.0, 420.0]),
    };
    let placed = window.show(&context, |ui| {
        ui.horizontal(|ui| {
            for tab in Tab::ALL {
                if tab == Tab::Admin && !staff {
                    continue;
                }
                if ui.selectable_label(desk.tab == tab, tab.title()).clicked() {
                    desk.tab = tab;
                }
            }
        });
        // A saved preference may name the staff tab after the shard has taken
        // authority away. Do not leave an ordinary player looking at controls
        // the current connection must not use.
        if desk.tab == Tab::Admin && !staff {
            desk.tab = Tab::Camera;
        }
        ui.separator();
        // Scrolled, because the tabs are of very different heights — the rig's
        // sliders and the tile's overlays do not fit what the camera's six rows
        // want the window to be, and a window sized to the tallest of them is a
        // window mostly full of nothing on the other four.
        // One scroll offset *per tab*, which is what `id_salt` buys: without it
        // all five share egui's one id, and scrolling the world list down leaves
        // the camera tab scrolled to somewhere it has no rows for.
        egui::ScrollArea::vertical()
            .id_salt(desk.tab.title())
            .show(ui, |ui| {
                match desk.tab {
                    Tab::Camera => camera_panel(ui, hud, camera, &mut desk.movement, &mut request),
                    Tab::Rig => rig_panel(ui, hud, world, &mut request),
                    Tab::Frames => frames_panel(ui, hud),
                    Tab::World => world_panel(ui, hud, world, &mut request),
                    Tab::Tile => tile_tab(ui, hud, world, &mut request),
                    Tab::Light => light_panel(ui, hud, &mut desk.light, &mut request),
                    Tab::Chat => {
                        chat_panel(
                            ui,
                            ChatPanel {
                                chat:               &mut desk.chat,
                                fonts:              &mut desk.fonts,
                                face:               &mut desk.font_face,
                                override_all_fonts: &mut desk.override_all_fonts,
                                bitmap_font:        &mut desk.bitmap_font,
                                ttf_active:         hud.ttf_active,
                                ttf_available:      hud.ttf_available,
                            },
                        )
                    }
                    Tab::Audio => audio_panel(ui, &mut desk.audio, &mut request),
                    Tab::Windows => windows_panel(ui, &mut desk.window_scale, &mut desk.status_frame),
                    Tab::Admin => {
                        admin_items_panel(
                            ui,
                            &mut desk.admin_item,
                            &mut desk.admin_skill,
                            &mut desk.admin_catalogue,
                            art,
                            hue_ramp,
                            skill_names,
                            world,
                            item_catalogue,
                            &mut request,
                        )
                    }
                    Tab::Combat => combat_recorder_panel(ui, &mut desk.combat_recorder, world, &mut request),
                }
            });
    });
    desk.open = open;
    // What egui made of it, read back after the frame it was laid out in: this
    // is the rect that goes in the file, and it is the one the window is
    // actually at rather than the one it was asked for.
    if let Some(placed) = placed {
        let rect = placed.response.rect;
        desk.panel = Some(crate::desk::Panel {
            x:      rect.min.x,
            y:      rect.min.y,
            width:  rect.width(),
            height: rect.height(),
        });
    }

    craft_window(CraftWindowFrame {
        context: &context,
        world,
        art,
        hue_ramp,
        cliloc,
        staff,
        panel: crafting,
        request: &mut request,
    });
    house_inventory_window(&context, world, house_inventory, &mut request);

    request
}

struct HouseInventoryPanel {
    open:             bool,
    query:            String,
    selected:         BTreeSet<openshard_protocol::house_inventory::HouseItemIdentity>,
    active_selectors: Vec<openshard_protocol::house_inventory::HouseItemIdentity>,
    epoch:            Option<u64>,
    rows:             Vec<openshard_protocol::house_inventory::HouseInventoryRow>,
    next:             Option<openshard_protocol::house_inventory::HouseInventoryCursor>,
    append_next:      bool,
    last_reply:       Option<openshard_protocol::house_inventory::HouseInventoryReply>,
    notice:           Option<String>,
}

impl HouseInventoryPanel {
    fn new() -> Self {
        Self {
            open:             false,
            query:            String::new(),
            selected:         BTreeSet::new(),
            active_selectors: Vec::new(),
            epoch:            None,
            rows:             Vec::new(),
            next:             None,
            append_next:      false,
            last_reply:       None,
            notice:           None,
        }
    }
}

fn house_inventory_window(
    context: &egui::Context,
    world: &WorldState,
    panel: &mut HouseInventoryPanel,
    request: &mut Request,
) {
    if !panel.open {
        return;
    }
    let Some(view) = world.authoritative.view.as_ref() else {
        panel.notice = Some("Enter the world before searching a house.".to_owned());
        return;
    };
    if view.house_inventory.as_ref() != panel.last_reply.as_ref() {
        if let Some(reply) = &view.house_inventory {
            match reply {
                openshard_protocol::house_inventory::HouseInventoryReply::Page { epoch, rows, next } => {
                    panel.epoch = Some(*epoch);
                    if panel.append_next {
                        panel.rows.extend(rows.iter().copied());
                    } else {
                        panel.rows = rows.clone();
                    }
                    panel.next = *next;
                    panel.notice = Some(format!("{} storage root result(s).", panel.rows.len()));
                    panel.append_next = false;
                }
                openshard_protocol::house_inventory::HouseInventoryReply::Resolved { root, .. } => {
                    panel.notice = Some(format!("Opened storage root {root}."));
                }
                openshard_protocol::house_inventory::HouseInventoryReply::Refused { reason, .. } => {
                    panel.notice = Some(house_inventory_refusal(*reason).to_owned());
                    panel.append_next = false;
                }
            }
            panel.last_reply = Some(reply.clone());
        }
    }

    let query = panel.query.trim().to_ascii_lowercase();
    let mut matches: Vec<_> = openshard_protocol::house_inventory::HOUSE_ITEM_CATALOGUE
        .iter()
        .filter(|entry| {
            query.is_empty()
                || query
                    .split_whitespace()
                    .all(|word| entry.name.contains(word) || entry.tags.iter().any(|tag| tag.contains(word)))
        })
        .collect();
    matches.sort_by_key(|entry| (entry.name, entry.identity));
    matches.dedup_by_key(|entry| entry.identity);

    let mut open = panel.open;
    egui::Window::new("House inventory · Ctrl+I")
        .id(egui::Id::new("house inventory"))
        .default_pos([48.0, 48.0])
        .default_size([720.0, 680.0])
        .open(&mut open)
        .show(context, |ui| {
            ui.label("Searches only locked-down storage you may access in the house where you stand.");
            ui.horizontal(|ui| {
                ui.strong("Name or category");
                ui.add(
                    egui::TextEdit::singleline(&mut panel.query)
                        .hint_text("e.g. valorite, armor, tool")
                        .desired_width(320.0),
                );
                if ui.button("Clear").clicked() {
                    panel.query.clear();
                    panel.selected.clear();
                }
            });

            if let Some(identity) = parse_legacy_house_identity(&query) {
                let mut selected = panel.selected.contains(&identity);
                if ui
                    .checkbox(&mut selected, format!("Exact legacy {query}"))
                    .changed()
                {
                    if selected {
                        if panel.selected.len()
                            < openshard_protocol::house_inventory::MAX_HOUSE_INVENTORY_SELECTORS
                        {
                            panel.selected.insert(identity);
                        }
                    } else {
                        panel.selected.remove(&identity);
                    }
                }
            }

            ui.label(format!(
                "{} catalogue match(es); {} of {} selectors chosen",
                matches.len(),
                panel.selected.len(),
                openshard_protocol::house_inventory::MAX_HOUSE_INVENTORY_SELECTORS
            ));
            egui::ScrollArea::vertical()
                .id_salt("house selector list")
                .max_height(220.0)
                .show(ui, |ui| {
                    for entry in matches.iter().take(300) {
                        let mut selected = panel.selected.contains(&entry.identity);
                        let enabled = selected
                            || panel.selected.len()
                                < openshard_protocol::house_inventory::MAX_HOUSE_INVENTORY_SELECTORS;
                        ui.add_enabled_ui(enabled, |ui| {
                            if ui
                                .checkbox(
                                    &mut selected,
                                    format!(
                                        "{} · art {:#06x}, hue {:#06x}",
                                        entry.name, entry.graphic.0, entry.hue.0
                                    ),
                                )
                                .changed()
                            {
                                if selected {
                                    panel.selected.insert(entry.identity);
                                } else {
                                    panel.selected.remove(&entry.identity);
                                }
                            }
                        });
                    }
                });

            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!panel.selected.is_empty(), egui::Button::new("Search selected"))
                    .clicked()
                {
                    panel.active_selectors = panel.selected.iter().copied().collect();
                    panel.rows.clear();
                    panel.next = None;
                    panel.epoch = None;
                    panel.append_next = false;
                    request.house_inventory = Some(
                        openshard_protocol::house_inventory::HouseInventoryRequest::Search {
                            expected_epoch: None,
                            selectors:      panel.active_selectors.clone(),
                            after:          None,
                            limit:          openshard_protocol::house_inventory::MAX_HOUSE_INVENTORY_PAGE
                                as u8,
                        },
                    );
                }
                if ui
                    .add_enabled(panel.next.is_some(), egui::Button::new("Next page"))
                    .clicked()
                {
                    panel.append_next = true;
                    request.house_inventory = Some(
                        openshard_protocol::house_inventory::HouseInventoryRequest::Search {
                            expected_epoch: panel.epoch,
                            selectors:      panel.active_selectors.clone(),
                            after:          panel.next,
                            limit:          openshard_protocol::house_inventory::MAX_HOUSE_INVENTORY_PAGE
                                as u8,
                        },
                    );
                }
                if let Some(notice) = &panel.notice {
                    ui.label(notice);
                }
            });

            ui.separator();
            egui::ScrollArea::vertical()
                .id_salt("house result list")
                .show(ui, |ui| {
                    for row in &panel.rows {
                        ui.horizontal(|ui| {
                            ui.label(format!(
                                "{} · {} in root · {} total · {} pile(s)",
                                house_identity_name(row.identity),
                                row.root_total,
                                row.aggregate_total,
                                row.pile_count
                            ));
                            if ui.button("Open root").clicked() {
                                if let Some(epoch) = panel.epoch {
                                    request.house_inventory = Some(
                                        openshard_protocol::house_inventory::HouseInventoryRequest::Resolve {
                                            epoch,
                                            identity: row.identity,
                                            root: row.root,
                                            item: row.first_pile,
                                        },
                                    );
                                }
                            }
                        });
                    }
                });
        });
    panel.open = open;
}

fn house_identity_name(identity: openshard_protocol::house_inventory::HouseItemIdentity) -> String {
    openshard_protocol::house_inventory::HOUSE_ITEM_CATALOGUE
        .iter()
        .find(|entry| entry.identity == identity)
        .map_or_else(
            || {
                match identity {
                    openshard_protocol::house_inventory::HouseItemIdentity::Semantic { kind, material } => {
                        format!(
                            "item kind {} material {:?}",
                            kind.0,
                            material.map(|material| material.0)
                        )
                    }
                    openshard_protocol::house_inventory::HouseItemIdentity::Legacy { graphic, hue } => {
                        format!("legacy {:#06x}:{:#06x}", graphic.0, hue.0)
                    }
                }
            },
            |entry| entry.name.to_owned(),
        )
}

fn parse_legacy_house_identity(
    query: &str,
) -> Option<openshard_protocol::house_inventory::HouseItemIdentity> {
    let (graphic, hue) = query.split_once(':')?;
    let graphic = u16::from_str_radix(graphic.trim_start_matches("0x"), 16).ok()?;
    let hue = u16::from_str_radix(hue.trim_start_matches("0x"), 16).ok()?;
    Some(openshard_protocol::house_inventory::HouseItemIdentity::Legacy {
        graphic: Graphic(graphic),
        hue:     Hue(hue),
    })
}

const fn house_inventory_refusal(
    reason: openshard_protocol::house_inventory::HouseInventoryRefusal,
) -> &'static str {
    match reason {
        openshard_protocol::house_inventory::HouseInventoryRefusal::NotInHouse => {
            "Stand inside a house to search its storage."
        }
        openshard_protocol::house_inventory::HouseInventoryRefusal::Banned => {
            "You may not search this house."
        }
        openshard_protocol::house_inventory::HouseInventoryRefusal::InvalidRequest => {
            "The search selection is invalid."
        }
        openshard_protocol::house_inventory::HouseInventoryRefusal::Unavailable => {
            "The house index is rebuilding; try again next tick."
        }
        openshard_protocol::house_inventory::HouseInventoryRefusal::Stale => {
            "The house changed; run the search again."
        }
        openshard_protocol::house_inventory::HouseInventoryRefusal::NotFound => {
            "That result moved or is no longer accessible."
        }
    }
}

/// Catalogue, tool workbench and recipe details are pages of one server gump.
/// Giving every page the same egui id makes replacement keep the window's
/// position and size instead of making details look like a second dialog.
fn crafting_window_id(gump_id: u32) -> egui::Id {
    egui::Id::new(("crafting", gump_id))
}

/// Client-owned state shared by every page of the craft window.
///
/// The shard owns recipes and the meaning of their buttons; this owns only
/// presentation state. Details and Back therefore switch the page inside one
/// state instead of closing one UI and constructing another.
#[derive(Default)]
struct CraftWindowPanel {
    gump_id:       Option<u32>,
    query:         String,
    availability:  CraftAvailability,
    skill:         Option<u32>,
    materials:     CraftMaterials,
    sort:          CraftSort,
    /// These areas may be absent while a detail page is visible. Their offsets
    /// live here so returning to either list resumes exactly where it was.
    table_scroll:  egui::Vec2,
    row_scroll:    f32,
    recipe_scroll: f32,
    /// A gump reply closes the authoritative shell locally before the shard
    /// sends its replacement page. Keep this state alive across that gap.
    awaiting_page: bool,
    /// Thumbnails are decoded only for rows inside the scroll viewport. The
    /// key includes hue: the same ingot graphic is several distinct metals.
    textures:      BTreeMap<(u16, u16), Option<egui::TextureHandle>>,
}

impl CraftWindowPanel {
    fn close(&mut self) {
        self.gump_id = None;
        self.awaiting_page = false;
        self.table_scroll = egui::Vec2::ZERO;
        self.row_scroll = 0.0;
        self.recipe_scroll = 0.0;
        self.textures.clear();
    }

    fn page_missing(&mut self) {
        if !self.awaiting_page {
            self.close();
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum CraftAvailability {
    #[default]
    All,
    Ready,
    Missing,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum CraftMaterials {
    #[default]
    Any,
    One,
    Several,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum CraftSort {
    #[default]
    Name,
    Skill,
    Materials,
    Availability,
}

/// Localized, searchable data for one recipe. Keeping this separate from the
/// wire row makes filtering and sorting linear in the catalogue rather than
/// repeatedly resolving localization strings from a sort comparator.
struct CraftCatalogueEntry<'a> {
    row:                 &'a openshard_protocol::craft::CraftCatalogueRow,
    name:                String,
    skill:               String,
    component_names:     Vec<String>,
    weapon_combat_skill: Option<String>,
    search:              String,
}

impl<'a> CraftCatalogueEntry<'a> {
    fn new(row: &'a openshard_protocol::craft::CraftCatalogueRow, cliloc: Option<&Cliloc>) -> Self {
        let name = craft_label(row.name, cliloc);
        let skill = craft_skill_label(row.skill, cliloc);
        let component_names: Vec<_> = row
            .components
            .iter()
            .map(|component| craft_label(component.name, cliloc))
            .collect();
        let weapon_combat_skill = row.weapon.map(|weapon| craft_label(weapon.combat_skill, cliloc));
        let graphics = std::iter::once(row.result)
            .chain(row.components.iter().map(|component| component.graphic))
            .map(|graphic| format!("{:#06x}", graphic.0))
            .collect::<Vec<_>>()
            .join(" ");
        let search = format!("{name} {skill} {} {graphics}", component_names.join(" ")).to_lowercase();
        Self {
            row,
            name,
            skill,
            component_names,
            weapon_combat_skill,
            search,
        }
    }

    fn matches(
        &self,
        query: &str,
        availability: CraftAvailability,
        skill: Option<u32>,
        materials: CraftMaterials,
    ) -> bool {
        let row = self.row;
        (availability != CraftAvailability::Ready || row.ready)
            && (availability != CraftAvailability::Missing || !row.ready)
            && skill.is_none_or(|skill| row.skill.0 == skill)
            && (materials != CraftMaterials::One || row.components.len() == 1)
            && (materials != CraftMaterials::Several || row.components.len() >= 2)
            && (query.is_empty() || self.search.contains(query))
    }

    fn compare(&self, other: &Self, sort: CraftSort) -> std::cmp::Ordering {
        let order = match sort {
            CraftSort::Name => self.name.cmp(&other.name),
            CraftSort::Skill => self.skill.cmp(&other.skill),
            CraftSort::Materials => self.row.components.len().cmp(&other.row.components.len()),
            CraftSort::Availability => other.row.ready.cmp(&self.row.ready),
        };
        order.then_with(|| self.name.cmp(&other.name))
    }
}

/// Fixed columns keep the header and virtualized rows in sync. The number of
/// component columns comes from the filtered rows, so short recipes leave
/// explicit empty cells and the final Status column never drifts.
#[derive(Clone, Copy)]
struct CraftTableLayout {
    component_columns: usize,
}

impl CraftTableLayout {
    const RESULT: f32 = 84.0;
    const RECIPE: f32 = 270.0;
    // Skill is a compact requirement badge; the full localized name belongs in
    // its tooltip rather than consuming a text column in every row.
    const SKILL: f32 = 76.0;
    const STATUS: f32 = 120.0;
    const ROW_HEIGHT: f32 = 70.0;
    const COMPONENT: f32 = 92.0;

    fn for_entries(entries: &[CraftCatalogueEntry<'_>]) -> Self {
        Self {
            // Every material has a real column. Empty cells in shorter recipes
            // keep Status at one x coordinate instead of attaching it to the
            // last material the particular row happened to have.
            component_columns: entries
                .iter()
                .map(|entry| entry.row.components.len().max(1))
                .fold(1, usize::max),
        }
    }

    fn width(self) -> f32 {
        Self::RESULT
            + Self::RECIPE
            + Self::SKILL
            + self.component_columns as f32 * Self::COMPONENT
            + Self::STATUS
    }
}

/// The client resources and mutable panel state used by one craft window.
struct CraftWindowFrame<'a> {
    context:  &'a egui::Context,
    world:    &'a WorldState,
    art:      &'a openshard_uofiles::art::Art,
    hue_ramp: &'a openshard_client_render::hue::HueRamp,
    cliloc:   Option<&'a Cliloc>,
    staff:    bool,
    panel:    &'a mut CraftWindowPanel,
    request:  &'a mut Request,
}

/// Render exactly one craft window. The two typed packets describe different
/// pages of the same server gump; they must never own separate egui state or
/// compete to write the frame's reply.
fn craft_window(frame: CraftWindowFrame<'_>) {
    let CraftWindowFrame {
        context,
        world,
        art,
        hue_ramp,
        cliloc,
        staff,
        panel,
        request,
    } = frame;
    let Some(view) = world.authoritative.view.as_ref() else {
        panel.close();
        return;
    };

    let workbench = view.craft_workbenches.values().find_map(|workbench| {
        view.gumps
            .iter()
            .find(|gump| gump.gump_id == workbench.gump_id)
            .map(|gump| (workbench, gump))
    });
    if let Some((workbench, gump)) = workbench {
        panel.awaiting_page = false;
        craft_workbench_window(context, workbench, gump, art, hue_ramp, cliloc, panel, request);
        return;
    }

    let catalogue = view.craft_catalogues.values().find_map(|catalogue| {
        view.gumps
            .iter()
            .find(|gump| gump.gump_id == catalogue.gump_id)
            .map(|gump| (catalogue, gump))
    });
    if let Some((catalogue, gump)) = catalogue {
        panel.awaiting_page = false;
        craft_catalogue_window(
            context, catalogue, gump, art, hue_ramp, cliloc, staff, panel, request,
        );
    } else {
        panel.page_missing();
    }
}

#[allow(clippy::too_many_arguments)]
fn craft_catalogue_window(
    context: &egui::Context,
    catalogue: &openshard_protocol::craft::CraftCatalogue,
    gump: &openshard_client_net::view::OpenGump,
    art: &openshard_uofiles::art::Art,
    hue_ramp: &openshard_client_render::hue::HueRamp,
    cliloc: Option<&Cliloc>,
    staff: bool,
    panel: &mut CraftWindowPanel,
    request: &mut Request,
) {
    if panel.gump_id != Some(catalogue.gump_id.0) {
        panel.gump_id = Some(catalogue.gump_id.0);
        // The screenshot runner can ask for a representative, narrow subset
        // without changing the player's normal opening state. It is deliberately
        // process-local and ignored unless that runner supplied the variable.
        panel.query = std::env::var("OPENSHARD_CRAFT_CATALOGUE_SHOWCASE_QUERY").unwrap_or_default();
        panel.availability = CraftAvailability::All;
        panel.skill = None;
        panel.materials = CraftMaterials::Any;
        panel.sort = CraftSort::Name;
        panel.table_scroll = egui::Vec2::ZERO;
        panel.row_scroll = 0.0;
        panel.recipe_scroll = 0.0;
        panel.textures.clear();
    }

    let mut open = true;
    let mut reply = None;
    // A wide catalogue must be wide on a wide monitor, but its scroll canvas
    // may never enlarge the floating window past the current viewport.  The
    // latter would hide the right-hand filters and status column rather than
    // making them reachable by the table's horizontal scrollbar.
    let viewport = context.content_rect().size();
    let max_size = egui::vec2((viewport.x - 32.0).max(900.0), (viewport.y - 32.0).max(500.0));
    egui::Window::new(format!("Crafting · {} recipes", catalogue.rows.len()))
        .id(crafting_window_id(catalogue.gump_id.0))
        .default_pos([24.0, 24.0])
        .default_size([1240.0, 760.0])
        .min_size([900.0, 500.0])
        .max_size(max_size)
        .open(&mut open)
        .show(context, |ui| {
            ui.horizontal(|ui| {
                ui.strong("Search");
                ui.add(
                    egui::TextEdit::singleline(&mut panel.query)
                        .hint_text("name, skill, result, or component")
                        .desired_width(360.0),
                );
                if ui.button("Clear").clicked() {
                    panel.query.clear();
                    panel.availability = CraftAvailability::All;
                    panel.skill = None;
                    panel.materials = CraftMaterials::Any;
                }
            });

            ui.horizontal_wrapped(|ui| {
                ui.strong("Availability");
                ui.selectable_value(&mut panel.availability, CraftAvailability::All, "All");
                ui.selectable_value(&mut panel.availability, CraftAvailability::Ready, "Available");
                ui.selectable_value(&mut panel.availability, CraftAvailability::Missing, "Unavailable");
                ui.separator();
                let mut skills: Vec<_> = catalogue
                    .rows
                    .iter()
                    .map(|row| row.skill)
                    .filter(|skill| skill.0 != 0)
                    .collect();
                skills.sort_by_key(|skill| skill.0);
                skills.dedup_by_key(|skill| skill.0);
                egui::ComboBox::from_id_salt(("craft skill", catalogue.gump_id.0))
                    .selected_text(
                        panel
                            .skill
                            .map(|skill| craft_label(ClilocId(skill), cliloc))
                            .unwrap_or_else(|| "All skills".to_owned()),
                    )
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut panel.skill, None, "All skills");
                        for skill in skills {
                            ui.selectable_value(&mut panel.skill, Some(skill.0), craft_label(skill, cliloc));
                        }
                    });
                ui.separator();
                ui.strong("Components");
                ui.selectable_value(&mut panel.materials, CraftMaterials::Any, "Any count");
                ui.selectable_value(&mut panel.materials, CraftMaterials::One, "One");
                ui.selectable_value(&mut panel.materials, CraftMaterials::Several, "Two or more");
                ui.separator();
                ui.label("Sort");
                egui::ComboBox::from_id_salt(("craft sort", catalogue.gump_id.0))
                    .selected_text(craft_sort_name(panel.sort))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut panel.sort, CraftSort::Name, "By name");
                        ui.selectable_value(&mut panel.sort, CraftSort::Skill, "By skill");
                        ui.selectable_value(&mut panel.sort, CraftSort::Materials, "By component count");
                        ui.selectable_value(&mut panel.sort, CraftSort::Availability, "Available first");
                    });
            });

            let query = panel.query.trim().to_lowercase();
            let mut matching: Vec<_> = catalogue
                .rows
                .iter()
                .map(|row| CraftCatalogueEntry::new(row, cliloc))
                .filter(|entry| entry.matches(&query, panel.availability, panel.skill, panel.materials))
                .collect();
            matching.sort_by(|left, right| left.compare(right, panel.sort));
            if panel.textures.len() > 512 {
                panel.textures.clear();
            }
            let layout = CraftTableLayout::for_entries(&matching);
            ui.horizontal(|ui| {
                ui.small(format!("Showing {} of {}", matching.len(), catalogue.rows.len()));
                ui.separator();
                ui.small("Scroll to browse · hover framed items for details");
            });
            ui.separator();

            let table_viewport_width = ui.available_width();
            let table_scroll = egui::ScrollArea::both()
                .id_salt(("craft catalogue rows", catalogue.gump_id.0))
                .auto_shrink([false, false])
                .max_width(table_viewport_width)
                .scroll_offset(panel.table_scroll);
            let table_output = table_scroll.show(ui, |ui| {
                // The header belongs to the same horizontal canvas as the
                // virtualized rows. Otherwise its widest column quietly
                // becomes a minimum width for the whole floating window,
                // pushing the filters and final columns off a small screen.
                ui.set_min_width(layout.width());
                craft_table_header(ui, layout);
                ui.separator();
                let row_scroll = egui::ScrollArea::vertical()
                    .id_salt(("craft catalogue virtual rows", catalogue.gump_id.0))
                    .auto_shrink([false, false])
                    .vertical_scroll_offset(panel.row_scroll);
                let row_output =
                    row_scroll.show_rows(ui, CraftTableLayout::ROW_HEIGHT, matching.len(), |ui, rows| {
                        for entry in &matching[rows] {
                            if let Some(button) =
                                craft_table_row(ui, entry, art, hue_ramp, staff, panel, layout)
                            {
                                panel.awaiting_page = true;
                                reply = Some(craft_reply(gump, button));
                            }
                        }
                    });
                panel.row_scroll = row_output.state.offset.y;
            });
            panel.table_scroll = table_output.state.offset;
        });

    if !open {
        panel.close();
        reply = Some(craft_reply(gump, 0));
    }
    request.craft_reply = reply;
}

/// Render the ordinary tool craft gump as an egui workbench.
///
/// The server still emits the old gump as a compatibility shell, but this view
/// is its owner for OpenShard clients. Every visible action sends the exact
/// reply number from the typed model, so this is presentation replacement, not
/// a second crafting protocol.
#[allow(clippy::too_many_arguments)]
fn craft_workbench_window(
    context: &egui::Context,
    workbench: &openshard_protocol::craft::CraftWorkbench,
    gump: &openshard_client_net::view::OpenGump,
    art: &openshard_uofiles::art::Art,
    hue_ramp: &openshard_client_render::hue::HueRamp,
    cliloc: Option<&Cliloc>,
    panel: &mut CraftWindowPanel,
    request: &mut Request,
) {
    if panel.gump_id != Some(workbench.gump_id.0) {
        panel.gump_id = Some(workbench.gump_id.0);
        panel.query.clear();
        panel.availability = CraftAvailability::All;
        panel.skill = None;
        panel.materials = CraftMaterials::Any;
        panel.sort = CraftSort::Name;
        panel.table_scroll = egui::Vec2::ZERO;
        panel.row_scroll = 0.0;
        panel.recipe_scroll = 0.0;
        panel.textures.clear();
    }

    let mut open = true;
    let mut reply = None;
    let title = craft_workbench_text(&workbench.title, cliloc);
    egui::Window::new(format!("Crafting · {title}"))
        .id(crafting_window_id(workbench.gump_id.0))
        .default_pos([48.0, 48.0])
        .default_size([1040.0, 700.0])
        .min_size([760.0, 500.0])
        .open(&mut open)
        .show(context, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.heading(&title);
                ui.separator();
                match workbench.tool_uses {
                    Some(uses) if workbench.tool_carried => {
                        ui.colored_label(
                            egui::Color32::from_rgb(95, 205, 120),
                            format!("Tool · {uses} uses"),
                        )
                    }
                    Some(uses) => {
                        ui.colored_label(
                            ui.visuals().warn_fg_color,
                            format!("Tool is not carried · {uses} uses"),
                        )
                    }
                    None => ui.colored_label(ui.visuals().warn_fg_color, "Tool unavailable"),
                };
                craft_facility_badges(ui, workbench.required_facilities, workbench.present_facilities);
                if ui.button("Refresh").clicked() {
                    panel.awaiting_page = true;
                    reply = Some(craft_reply(gump, workbench.refresh_button));
                }
            });
            if let Some(notice) = &workbench.notice {
                ui.colored_label(ui.visuals().warn_fg_color, craft_workbench_text(notice, cliloc));
            }
            ui.separator();
            ui.columns(2, |columns| {
                columns[0].set_min_width(210.0);
                columns[0].strong("Categories");
                egui::ScrollArea::vertical().show(&mut columns[0], |ui| {
                    for group in &workbench.groups {
                        let label = craft_workbench_text(&group.name, cliloc);
                        if ui.selectable_label(group.selected, label).clicked() {
                            panel.awaiting_page = true;
                            reply = Some(craft_reply(gump, group.button));
                        }
                    }
                });
                columns[1].vertical(|ui| {
                    match &workbench.page {
                        openshard_protocol::craft::CraftWorkbenchPage::Items { recipes } => {
                            ui.strong("Recipes");
                            let recipe_scroll = egui::ScrollArea::vertical()
                                .id_salt(("craft recipes", workbench.gump_id.0))
                                .vertical_scroll_offset(panel.recipe_scroll);
                            let output = recipe_scroll.show(ui, |ui| {
                                for recipe in recipes {
                                    craft_workbench_recipe_row(
                                        ui, recipe, art, hue_ramp, cliloc, panel, gump, &mut reply,
                                    );
                                    ui.separator();
                                }
                            });
                            panel.recipe_scroll = output.state.offset.y;
                        }
                        openshard_protocol::craft::CraftWorkbenchPage::Resources { materials } => {
                            ui.strong("Materials");
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                for material in materials {
                                    ui.horizontal(|ui| {
                                        craft_workbench_icon(
                                            ui,
                                            art,
                                            hue_ramp,
                                            panel,
                                            material.graphic,
                                            material.hue,
                                            34.0,
                                        );
                                        let text = format!(
                                            "{} · {} available",
                                            craft_workbench_text(&material.name, cliloc),
                                            material.carried
                                        );
                                        if ui.selectable_label(material.selected, text).clicked() {
                                            panel.awaiting_page = true;
                                            reply = Some(craft_reply(gump, material.button));
                                        }
                                    });
                                }
                            });
                        }
                        openshard_protocol::craft::CraftWorkbenchPage::Details {
                            recipe,
                            success_per_mille,
                            exceptional_per_mille,
                        } => {
                            ui.strong("Recipe details");
                            craft_workbench_detail(
                                ui,
                                recipe,
                                *success_per_mille,
                                *exceptional_per_mille,
                                art,
                                hue_ramp,
                                cliloc,
                                panel,
                                gump,
                                if workbench.tool_uses.is_some() {
                                    "Back to recipes"
                                } else {
                                    "Back to catalogue"
                                },
                                &mut reply,
                            );
                        }
                    }
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                if let Some(material) = &workbench.selected_material {
                    craft_workbench_icon(ui, art, hue_ramp, panel, material.graphic, material.hue, 28.0);
                    ui.label(format!(
                        "Material: {} · {} available",
                        craft_workbench_text(&material.name, cliloc),
                        material.carried
                    ));
                }
                if let Some(button) = workbench.materials_button {
                    if ui.button("Materials").clicked() {
                        panel.awaiting_page = true;
                        reply = Some(craft_reply(gump, button));
                    }
                }
                if ui.button("Cancel make").clicked() {
                    panel.awaiting_page = true;
                    reply = Some(craft_reply(gump, workbench.cancel_button));
                }
                if ui.button("Close").clicked() {
                    panel.close();
                    reply = Some(craft_reply(gump, 0));
                }
            });
        });
    if !open {
        panel.close();
        reply = Some(craft_reply(gump, 0));
    }
    request.craft_reply = reply;
}

#[allow(clippy::too_many_arguments)]
fn craft_workbench_recipe_row(
    ui: &mut egui::Ui,
    recipe: &openshard_protocol::craft::CraftWorkbenchRecipe,
    art: &openshard_uofiles::art::Art,
    hue_ramp: &openshard_client_render::hue::HueRamp,
    cliloc: Option<&Cliloc>,
    panel: &mut CraftWindowPanel,
    gump: &openshard_client_net::view::OpenGump,
    reply: &mut Option<GumpReply>,
) {
    let mut open_details = false;
    ui.horizontal(|ui| {
        open_details |= craft_workbench_icon(
            ui,
            art,
            hue_ramp,
            panel,
            recipe.result.graphic,
            recipe.result.hue,
            42.0,
        )
        .interact(egui::Sense::click())
        .on_hover_text("Open recipe details")
        .clicked();
        ui.vertical(|ui| {
            open_details |= ui
                .add(
                    egui::Label::new(
                        egui::RichText::new(craft_workbench_text(&recipe.result.name, cliloc)).strong(),
                    )
                    .sense(egui::Sense::click()),
                )
                .on_hover_text("Open recipe details")
                .clicked();
            ui.small(craft_workbench_requirements(recipe, cliloc));
        });
        if let Some(button) = recipe.make_button {
            if ui.button("Make").clicked() {
                panel.awaiting_page = true;
                *reply = Some(craft_reply(gump, button));
            }
        }
        if let Some(button) = recipe.admin_button {
            if ui.button("Create (admin)").clicked() {
                panel.awaiting_page = true;
                *reply = Some(craft_reply(gump, button));
            }
        }
        if let Some(button) = recipe.details_button {
            if ui.button("Details").clicked() {
                open_details = true;
            }
            if open_details {
                panel.awaiting_page = true;
                *reply = Some(craft_reply(gump, button));
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn craft_workbench_detail(
    ui: &mut egui::Ui,
    recipe: &openshard_protocol::craft::CraftWorkbenchRecipe,
    success_per_mille: u16,
    exceptional_per_mille: Option<u16>,
    art: &openshard_uofiles::art::Art,
    hue_ramp: &openshard_client_render::hue::HueRamp,
    cliloc: Option<&Cliloc>,
    panel: &mut CraftWindowPanel,
    gump: &openshard_client_net::view::OpenGump,
    back_label: &str,
    reply: &mut Option<GumpReply>,
) {
    ui.horizontal(|ui| {
        craft_workbench_icon(
            ui,
            art,
            hue_ramp,
            panel,
            recipe.result.graphic,
            recipe.result.hue,
            88.0,
        );
        ui.vertical(|ui| {
            ui.heading(craft_workbench_text(&recipe.result.name, cliloc));
            ui.label(format!(
                "Success chance: {:.1}%",
                f32::from(success_per_mille) / 10.0
            ));
            if let Some(exceptional) = exceptional_per_mille {
                ui.label(format!(
                    "Exceptional chance: {:.1}%",
                    f32::from(exceptional) / 10.0
                ));
            }
            for (skill, minimum) in &recipe.skills {
                ui.small(format!(
                    "{}: {:.1}%",
                    craft_workbench_text(skill, cliloc),
                    f32::from(*minimum) / 10.0
                ));
            }
        });
    });
    ui.separator();
    ui.strong("Materials");
    for component in &recipe.components {
        ui.horizontal(|ui| {
            craft_workbench_icon(ui, art, hue_ramp, panel, component.graphic, component.hue, 32.0);
            let carried = component
                .carried
                .map_or_else(String::new, |amount| format!(" · {amount} available"));
            ui.label(format!(
                "{} ×{}{}",
                craft_workbench_text(&component.name, cliloc),
                component.amount,
                carried
            ));
        });
    }
    if recipe.use_all_resources {
        ui.small("Uses all available resources.");
    }
    if recipe.markable {
        ui.small("May receive a maker's mark.");
    }
    ui.separator();
    if let Some(button) = recipe.make_button {
        if ui.button("Make now").clicked() {
            panel.awaiting_page = true;
            *reply = Some(craft_reply(gump, button));
        }
    }
    if let Some(button) = recipe.admin_button {
        if ui.button("Create immediately (admin)").clicked() {
            panel.awaiting_page = true;
            *reply = Some(craft_reply(gump, button));
        }
    }
    if ui.button(back_label).clicked() {
        panel.awaiting_page = true;
        *reply = Some(craft_reply(gump, 0));
    }
}

fn craft_workbench_requirements(
    recipe: &openshard_protocol::craft::CraftWorkbenchRecipe,
    cliloc: Option<&Cliloc>,
) -> String {
    let skills = recipe
        .skills
        .iter()
        .map(|(skill, minimum)| {
            format!(
                "{} {:.1}%",
                craft_workbench_text(skill, cliloc),
                f32::from(*minimum) / 10.0
            )
        })
        .collect::<Vec<_>>()
        .join(" · ");
    let materials = recipe
        .components
        .iter()
        .map(|component| {
            format!(
                "{} ×{}",
                craft_workbench_text(&component.name, cliloc),
                component.amount
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    match (skills.is_empty(), materials.is_empty()) {
        (true, true) => String::new(),
        (false, true) => skills,
        (true, false) => materials,
        (false, false) => format!("{skills} · {materials}"),
    }
}

fn craft_workbench_icon(
    ui: &mut egui::Ui,
    art: &openshard_uofiles::art::Art,
    hue_ramp: &openshard_client_render::hue::HueRamp,
    panel: &mut CraftWindowPanel,
    graphic: Graphic,
    hue: Hue,
    size: f32,
) -> egui::Response {
    egui::Frame::group(ui.style())
        .show(ui, |ui| {
            match craft_item_texture(ui.ctx(), art, hue_ramp, &mut panel.textures, graphic, hue) {
                Some(texture) => {
                    ui.add(egui::Image::from_texture(texture).fit_to_exact_size(egui::vec2(size, size)));
                }
                None => {
                    ui.add_sized([size, size], egui::Label::new("—"));
                }
            }
        })
        .response
}

fn craft_workbench_text(text: &openshard_protocol::craft::CraftText, cliloc: Option<&Cliloc>) -> String {
    match text {
        openshard_protocol::craft::CraftText::Cliloc(id) => craft_label(*id, cliloc),
        openshard_protocol::craft::CraftText::Literal(value) => craft_plain_text(value),
    }
}

fn craft_facility_badges(ui: &mut egui::Ui, required: u8, present: u8) {
    const NAMES: [&str; 6] = ["Forge", "Anvil", "Fire", "Oven", "Mill", "Water"];
    for (index, name) in NAMES.iter().enumerate() {
        let bit = 1 << index;
        if required & bit != 0 {
            let ready = present & bit != 0;
            ui.colored_label(
                if ready {
                    egui::Color32::from_rgb(95, 205, 120)
                } else {
                    ui.visuals().warn_fg_color
                },
                format!("{name}: {}", if ready { "ready" } else { "missing" }),
            );
        }
    }
}

#[cfg(test)]
fn craft_matches(
    row: &openshard_protocol::craft::CraftCatalogueRow,
    query: &str,
    availability: CraftAvailability,
    skill: Option<u32>,
    materials: CraftMaterials,
    cliloc: Option<&Cliloc>,
) -> bool {
    let entry = CraftCatalogueEntry::new(row, cliloc);
    let query = query.trim().to_lowercase();
    entry.matches(&query, availability, skill, materials)
}

fn craft_sort_name(sort: CraftSort) -> &'static str {
    match sort {
        CraftSort::Name => "By name",
        CraftSort::Skill => "By skill",
        CraftSort::Materials => "By component count",
        CraftSort::Availability => "Available first",
    }
}

fn craft_table_header(ui: &mut egui::Ui, layout: CraftTableLayout) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_sized(
            [CraftTableLayout::RESULT, 22.0],
            egui::Label::new(egui::RichText::new("Result").strong()),
        );
        let recipe = ui.add_sized(
            [CraftTableLayout::RECIPE, 22.0],
            egui::Label::new(egui::RichText::new("Recipe").strong()),
        );
        craft_table_column_separator(ui, &recipe);
        let skill = ui.add_sized(
            [CraftTableLayout::SKILL, 22.0],
            egui::Label::new(egui::RichText::new("Req.").strong()),
        );
        craft_table_column_separator(ui, &skill);
        for index in 0..layout.component_columns {
            let label = if layout.component_columns == 1 {
                "Component".to_owned()
            } else {
                format!("Component {}", index + 1)
            };
            let component = ui.add_sized(
                [CraftTableLayout::COMPONENT, 22.0],
                egui::Label::new(egui::RichText::new(label).strong()),
            );
            craft_table_column_separator(ui, &component);
        }
        let status = ui.add_sized(
            [CraftTableLayout::STATUS, 22.0],
            egui::Label::new(egui::RichText::new("Status").strong()),
        );
        craft_table_column_separator(ui, &status);
    });
    ui.separator();
}

fn craft_table_column_separator(ui: &egui::Ui, cell: &egui::Response) {
    let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
    ui.painter()
        .line_segment([cell.rect.left_top(), cell.rect.left_bottom()], stroke);
}

fn craft_table_row(
    ui: &mut egui::Ui,
    entry: &CraftCatalogueEntry<'_>,
    art: &openshard_uofiles::art::Art,
    hue_ramp: &openshard_client_render::hue::HueRamp,
    staff: bool,
    panel: &mut CraftWindowPanel,
    layout: CraftTableLayout,
) -> Option<u32> {
    let row = entry.row;
    let mut clicked = false;
    let mut admin_clicked = false;
    let response = ui.allocate_ui_with_layout(
        egui::vec2(layout.width(), CraftTableLayout::ROW_HEIGHT),
        egui::Layout::left_to_right(egui::Align::Center).with_main_justify(false),
        |ui| {
            let fill = if row.ready {
                ui.visuals().faint_bg_color
            } else {
                egui::Color32::from_rgba_unmultiplied(110, 55, 45, 42)
            };
            ui.painter().rect_filled(ui.max_rect(), 4.0, fill);
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.allocate_ui_with_layout(
                egui::vec2(CraftTableLayout::RESULT, CraftTableLayout::ROW_HEIGHT),
                egui::Layout::top_down(egui::Align::Center),
                |ui| {
                    clicked |= craft_icon(ui, art, hue_ramp, panel, row.result, row.result_hue, 48.0)
                        .interact(egui::Sense::click())
                        .on_hover_ui(|ui| craft_result_tooltip(ui, entry))
                        .clicked();
                },
            );
            let recipe_cell = ui.allocate_ui_with_layout(
                egui::vec2(CraftTableLayout::RECIPE, CraftTableLayout::ROW_HEIGHT),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    clicked |= ui
                        .add_sized(
                            [CraftTableLayout::RECIPE, 25.0],
                            egui::Label::new(egui::RichText::new(&entry.name).strong())
                                .truncate()
                                .sense(egui::Sense::click()),
                        )
                        .on_hover_text("Open recipe details")
                        .clicked();
                    if let Some(weapon) = row.weapon {
                        ui.horizontal_wrapped(|ui| craft_weapon_chips(ui, weapon));
                    }
                },
            );
            craft_table_column_separator(ui, &recipe_cell.response);
            let skill_cell = ui.allocate_ui_with_layout(
                egui::vec2(CraftTableLayout::SKILL, CraftTableLayout::ROW_HEIGHT),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    craft_skill_requirement(ui, entry);
                },
            );
            craft_table_column_separator(ui, &skill_cell.response);
            for index in 0..layout.component_columns {
                let component_cell = ui.allocate_ui_with_layout(
                    egui::vec2(CraftTableLayout::COMPONENT, CraftTableLayout::ROW_HEIGHT),
                    egui::Layout::top_down(egui::Align::Center),
                    |ui| {
                        if let Some((component, name)) =
                            row.components.get(index).zip(entry.component_names.get(index))
                        {
                            craft_icon(ui, art, hue_ramp, panel, component.graphic, component.hue, 36.0)
                                .on_hover_text(format!(
                                    "{name}\nRequired: ×{}\nGraphic: {:#06x}",
                                    component.amount, component.graphic.0
                                ));
                            ui.small(format!("×{}", component.amount));
                        }
                    },
                );
                craft_table_column_separator(ui, &component_cell.response);
            }
            let status_cell = ui.allocate_ui_with_layout(
                egui::vec2(CraftTableLayout::STATUS, CraftTableLayout::ROW_HEIGHT),
                egui::Layout::top_down(egui::Align::Center),
                |ui| {
                    let (status, color, fill) = if row.ready {
                        (
                            "Available",
                            egui::Color32::from_rgb(95, 205, 120),
                            egui::Color32::from_rgba_unmultiplied(52, 128, 70, 80),
                        )
                    } else {
                        (
                            "Unavailable",
                            ui.visuals().warn_fg_color,
                            egui::Color32::from_rgba_unmultiplied(160, 75, 60, 70),
                        )
                    };
                    ui.painter()
                        .rect_filled(ui.max_rect(), 0.0, fill.gamma_multiply(0.35));
                    ui.add_sized(
                        [108.0, if staff { 24.0 } else { 28.0 }],
                        egui::Button::new(egui::RichText::new(status).color(color).strong())
                            .fill(fill)
                            .sense(egui::Sense::hover()),
                    );
                    if staff
                        && ui
                            .add_sized([108.0, 24.0], egui::Button::new("Create (admin)"))
                            .clicked()
                    {
                        admin_clicked = true;
                    }
                },
            );
            craft_table_column_separator(ui, &status_cell.response);
        },
    );
    if admin_clicked {
        Some(row.admin_button)
    } else if clicked || response.response.interact(egui::Sense::click()).clicked() {
        Some(row.button)
    } else {
        None
    }
}

fn craft_icon(
    ui: &mut egui::Ui,
    art: &openshard_uofiles::art::Art,
    hue_ramp: &openshard_client_render::hue::HueRamp,
    panel: &mut CraftWindowPanel,
    graphic: Graphic,
    hue: Hue,
    size: f32,
) -> egui::Response {
    // A sprite's transparent bounds are not a useful target on their own.
    // The frame makes the cell legible and gives every result/material the
    // same clickable footprint, whatever its native art dimensions are.
    egui::Frame::group(ui.style())
        .show(ui, |ui| {
            match craft_item_texture(ui.ctx(), art, hue_ramp, &mut panel.textures, graphic, hue) {
                Some(texture) => {
                    ui.add_sized(
                        [size, size],
                        egui::Image::from_texture(texture)
                            .fit_to_exact_size(egui::vec2(size, size))
                            .sense(egui::Sense::hover()),
                    )
                }
                None => ui.add_sized([size, size], egui::Button::new("—")),
            }
        })
        .inner
}

/// The repeated table cell is intentionally symbolic: the precise localized
/// skill name is available on hover, while the number is the actionable fact.
fn craft_skill_requirement(ui: &mut egui::Ui, entry: &CraftCatalogueEntry<'_>) -> egui::Response {
    let response = ui.allocate_ui_with_layout(
        egui::vec2(CraftTableLayout::SKILL, 28.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            let (icon, painter) = ui.allocate_painter(egui::vec2(20.0, 20.0), egui::Sense::hover());
            let rect = icon.rect;
            let accent = egui::Color32::from_rgb(218, 166, 82);
            painter.rect_filled(rect, 4.0, egui::Color32::from_rgba_unmultiplied(122, 84, 35, 110));
            // A compact craft-tool mark: the horizontal anvil and diagonal
            // hammer read as a skill requirement without relying on a font
            // glyph that may be absent from a player's installation.
            painter.line_segment(
                [
                    egui::pos2(rect.left() + 4.0, rect.center().y + 4.0),
                    egui::pos2(rect.right() - 4.0, rect.center().y + 4.0),
                ],
                egui::Stroke::new(2.0, accent),
            );
            painter.line_segment(
                [
                    egui::pos2(rect.left() + 6.0, rect.center().y - 4.0),
                    egui::pos2(rect.right() - 6.0, rect.center().y + 2.0),
                ],
                egui::Stroke::new(2.0, accent),
            );
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(format!("{}%", tenths(entry.row.skill_min)))
                    .small()
                    .strong()
                    .color(accent),
            );
        },
    );
    response.response.on_hover_text(format!(
        "{}\nMinimum required: {}%",
        entry.skill,
        tenths(entry.row.skill_min)
    ))
}

fn craft_result_tooltip(ui: &mut egui::Ui, entry: &CraftCatalogueEntry<'_>) {
    ui.strong(&entry.name);
    ui.small(format!(
        "Crafting skill: {} (minimum {}%)",
        entry.skill,
        tenths(entry.row.skill_min)
    ));
    ui.small(format!("Result graphic: {:#06x}", entry.row.result.0));
    if let Some(weapon) = entry.row.weapon {
        ui.separator();
        ui.strong("Weapon properties");
        ui.label(format!(
            "Combat skill: {}",
            entry.weapon_combat_skill.as_deref().unwrap_or("—")
        ));
        ui.label(format!("Damage: {}–{}", weapon.damage_min, weapon.damage_max));
        ui.label(format!("Speed: {:.2} s", f32::from(weapon.speed_centis) / 100.0));
        ui.label(format!("Attack: {}", craft_weapon_kind_name(weapon.kind)));
        if let Some(range) = weapon.range {
            ui.label(format!("Range: {range} tiles"));
        }
    }
    ui.separator();
    ui.small("Click anywhere on this row to open recipe details.");
}

fn craft_weapon_chips(ui: &mut egui::Ui, weapon: openshard_protocol::craft::CraftWeaponProperties) {
    craft_property_chip(ui, format!("DMG {}–{}", weapon.damage_min, weapon.damage_max));
    craft_property_chip(ui, format!("SPD {:.2}s", f32::from(weapon.speed_centis) / 100.0));
    craft_property_chip(ui, craft_weapon_kind_name(weapon.kind).to_ascii_uppercase());
    if let Some(range) = weapon.range {
        craft_property_chip(ui, format!("RNG {range}"));
    }
}

fn craft_property_chip(ui: &mut egui::Ui, text: String) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .small()
                .color(egui::Color32::from_rgb(129, 196, 224)),
        )
        .sense(egui::Sense::hover()),
    );
    ui.add_space(6.0);
}

fn craft_weapon_kind_name(kind: openshard_protocol::craft::CraftWeaponKind) -> &'static str {
    use openshard_protocol::craft::CraftWeaponKind;
    match kind {
        CraftWeaponKind::Slashing => "Slashing",
        CraftWeaponKind::Piercing => "Piercing",
        CraftWeaponKind::Bashing => "Bashing",
        CraftWeaponKind::Axe => "Axe",
        CraftWeaponKind::Polearm => "Polearm",
        CraftWeaponKind::Staff => "Staff",
        CraftWeaponKind::Ranged => "Ranged",
    }
}

fn craft_item_texture<'a>(
    context: &egui::Context,
    art: &openshard_uofiles::art::Art,
    hue_ramp: &openshard_client_render::hue::HueRamp,
    textures: &'a mut BTreeMap<(u16, u16), Option<egui::TextureHandle>>,
    graphic: Graphic,
    hue: Hue,
) -> Option<&'a egui::TextureHandle> {
    let key = (graphic.0, hue.0);
    textures.entry(key).or_insert_with(|| {
        let image = art.static_art(graphic).ok().flatten()?;
        let size = [usize::from(image.width()), usize::from(image.height())];
        let pixels = image
            .pixels()
            .iter()
            .map(|pixel| preview_pixel(*pixel, hue_ramp, hue.0))
            .collect();
        Some(context.load_texture(
            format!("craft-item-art-{:04x}-{:04x}", graphic.0, hue.0),
            egui::ColorImage::new(size, pixels),
            egui::TextureOptions::NEAREST,
        ))
    });
    textures.get(&key).and_then(Option::as_ref)
}

fn craft_label(id: ClilocId, cliloc: Option<&Cliloc>) -> String {
    cliloc
        .and_then(|table| table.get(ClilocNumber::new(id.0)))
        .or_else(|| localized::fallback(id))
        .map(craft_plain_text)
        .unwrap_or_else(|| format!("#{:08x}", id.0))
}

/// Craft rows already carry amounts and layout separately, so their labels
/// need only the visible words from old gump-authored clilocs. Those strings
/// commonly contain HTML alignment tags and an amount slot which this typed
/// packet deliberately does not duplicate as an argument.
fn craft_plain_text(text: &str) -> String {
    let mut visible = String::with_capacity(text.len());
    let mut in_tag = false;
    let mut in_slot = false;
    for character in text.chars() {
        if in_tag {
            if character == '>' {
                in_tag = false;
            }
        } else if in_slot {
            if character == '~' {
                in_slot = false;
            }
        } else if character == '<' {
            in_tag = true;
        } else if character == '~' {
            in_slot = true;
        } else {
            visible.push(character);
        }
    }
    visible
        .replace("()", " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn craft_skill_label(id: ClilocId, cliloc: Option<&Cliloc>) -> String {
    if id.0 == 0 {
        "—".to_owned()
    } else {
        craft_label(id, cliloc)
    }
}

fn craft_reply(gump: &openshard_client_net::view::OpenGump, button: u32) -> GumpReply {
    GumpReply {
        key:          RawGumpKey(gump.key.0),
        gump_id:      RawGumpId(gump.gump_id.0),
        button:       RawButtonId(button),
        switches:     Vec::new(),
        text_entries: Vec::new(),
    }
}

/// **What this client was told about fighting, and when.**
///
/// The page behind *"there was a stall right here"*. Everything the shard says
/// about a fight crosses the wire as an edge — an action begins, a stage
/// changes, an outcome lands, a refusal starts or lifts — so a stall is the
/// *absence* of an edge, and an absence cannot be looked at directly. What can
/// be looked at is the **gap** between two edges, which is why that column is in
/// front of every line rather than left to be subtracted by eye.
///
/// The mark is the other half. A person notices a stall and then reaches for the
/// panel, by which time what was on screen is gone; a mark stamps the moment
/// *and* a snapshot of what was drawn over the body at it, which is the one
/// thing no later reading can recover.
fn combat_recorder_panel(
    ui: &mut egui::Ui,
    page: &mut crate::desk::CombatRecorder,
    world: &WorldState,
    request: &mut Request,
) {
    let me = world.me();
    ui.heading("Combat recorder");
    ui.label(
        "Every combat packet this client received, with the time between them. A stall is a gap: \
         the shard names every refusal it makes, so once a line has a reason, what is left to find \
         is how long nothing was said.",
    );
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label("Note");
        ui.add(
            egui::TextEdit::singleline(&mut page.note)
                .hint_text("what looked wrong")
                .desired_width(220.0),
        );
        if ui.button("Mark here").clicked() {
            request.mark_combat = Some(page.note.clone());
        }
    });
    ui.label(
        egui::RichText::new(
            "A mark records what was drawn over your body at that instant — the bar, the stage, \
             the last outcome and whatever is holding you up — because that is gone by the time \
             anybody reads the log.",
        )
        .small(),
    );
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.checkbox(&mut page.only_me, "Only my body");
        ui.add(
            egui::DragValue::new(&mut page.shown)
                .range(10..=400)
                .prefix("show ")
                .suffix(" lines"),
        );
        if ui.button("Save to file").clicked() {
            request.save_combat_log = true;
        }
        if ui.button("Clear").clicked() {
            request.clear_combat_log = true;
        }
    });
    ui.add_space(6.0);
    let log = &world.presentation.combat_log;
    let only = page.only_me.then_some(me).flatten();
    ui.label(format!(
        "{} entries kept; this client's clock is at {:.1}s.",
        log.len(),
        log.now().as_secs_f32()
    ));
    ui.separator();
    // Newest at the bottom, which is the direction a log is read in, and the gap
    // in front rather than behind: what a reader is scanning for is the row that
    // *followed* a long silence.
    let text = log.to_text(only);
    let lines: Vec<&str> = text.lines().collect();
    let tail = lines.len().saturating_sub(page.shown);
    egui::ScrollArea::vertical()
        .max_height(320.0)
        .stick_to_bottom(true)
        .id_salt("combat recorder log")
        .show(ui, |ui| {
            for line in &lines[tail..] {
                // Monospaced, because the gap column only reads as a column if
                // the digits line up.
                let mut text = egui::RichText::new(*line).monospace().small();
                if line.contains("MARK") {
                    text = text.color(ui.visuals().warn_fg_color);
                }
                ui.label(text);
            }
        });
}

/// The compact F1 workflow for making registered gameplay items, with an
/// explicitly labelled legacy-art escape hatch. The shard remains the
/// authority: this only avoids manually opening and filling a classic gump.
#[allow(clippy::too_many_arguments)]
fn admin_items_panel(
    ui: &mut egui::Ui,
    item: &mut crate::desk::AdminItem,
    skill: &mut crate::desk::AdminSkill,
    catalogue: &mut crate::desk::AdminCatalogue,
    art: &openshard_uofiles::art::Art,
    hue_ramp: &openshard_client_render::hue::HueRamp,
    skill_names: &openshard_uofiles::skills::Skills,
    world: &WorldState,
    item_browser: &mut ItemArtCatalogue,
    request: &mut Request,
) {
    ui.heading("Administrator catalogue");
    admin_skills_panel(ui, skill, skill_names, world, request);
    ui.separator();
    ui.heading("Gameplay items");
    ui.label(
        "Registered items, not arbitrary art. Weapons, armour, tools, books and containers are created with their gameplay behaviour.",
    );
    item_catalogue(ui, catalogue, art, hue_ramp, item_browser, request);
    ui.add_space(10.0);
    ui.heading("Animals");
    ui.label("Click an animal, then choose its place on the map.");
    catalogue_grid(ui, ANIMALS, |entry| request.place_creature = Some(entry.id));
    ui.add_space(10.0);
    ui.heading("Scarecrow");
    ui.label(
        "A target that does nothing back: it does not move, does not fight and does not flag you. \
         Put one down before chasing anything about combat — against a live creature the mob, its \
         brain and the sight line all move at once, and no two runs are the same run.",
    );
    ui.horizontal(|ui| {
        if ui.button("Stand one in front of me").clicked() {
            request.staff_command = Some(format!("{}dummy", openshard_commands::PREFIX));
        }
        if ui.button("Take the nearest away").clicked() {
            request.staff_command = Some(format!("{}dummy off", openshard_commands::PREFIX));
        }
    });
    ui.add_space(10.0);
    ui.collapsing("Legacy client art (debug only)", |ui| {
        ui.label(
            "This creates an unregistered legacy item when the art does not map to a gameplay definition. Use it to inspect client assets, not to make playable items.",
        );
        let selected_hue = admin_hue_panel(ui, item, art, hue_ramp, item_browser);
        ui.separator();
        ui.label("Graphic and amount accept decimal or 0x hexadecimal values.");
        egui::Grid::new("admin item fields")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("Graphic");
                ui.add(egui::TextEdit::singleline(&mut item.graphic).hint_text("0x0eed"));
                ui.end_row();
                ui.label("Amount");
                ui.add(egui::TextEdit::singleline(&mut item.amount).hint_text("1"));
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut item.stackable, "Stack identical items");
                ui.end_row();
            });

        let parsed = selected_hue
            .ok_or("Hue is required and must name a colour in this client's palette.")
            .and_then(|_| parse_admin_item(item));
        ui.add_space(8.0);
        match parsed {
            Ok(created) => {
                if ui.button("Create legacy art in backpack").clicked() {
                    request.create_item = Some(created);
                }
            }
            Err(problem) => {
                ui.add_enabled(false, egui::Button::new("Create legacy art in backpack"));
                ui.colored_label(ui.visuals().warn_fg_color, problem);
            }
        }
    });
    ui.add_space(8.0);
    ui.label(
        egui::RichText::new(
            "The server checks staff authority and backpack capacity before creating anything.",
        )
        .small()
        .weak(),
    );
}

/// The F1 front end for `.skill <name> <value>`.
///
/// The command remains speech rather than gaining a private protocol route:
/// its authority check, value cap and one-line skill update are therefore the
/// very same ones used by a staff member typing it into chat.
fn admin_skills_panel(
    ui: &mut egui::Ui,
    skill: &mut crate::desk::AdminSkill,
    names: &openshard_uofiles::skills::Skills,
    world: &WorldState,
    request: &mut Request,
) {
    ui.heading("Skill tester");
    ui.label("Set this character's skill, then immediately try its action in the world.");

    egui::Grid::new("admin skill fields")
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            ui.label("Skill");
            egui::ComboBox::from_id_salt("admin skill name")
                .selected_text(&skill.name)
                .show_ui(ui, |ui| {
                    for (_, known) in names.iter() {
                        ui.selectable_value(&mut skill.name, known.name.clone(), &known.name);
                    }
                });
            ui.end_row();

            ui.label("Value");
            ui.add(egui::TextEdit::singleline(&mut skill.value).hint_text("95 or 95.5"));
            ui.end_row();
        });

    let current = names
        .iter()
        .find(|(_, known)| known.name == skill.name)
        .and_then(|(id, _)| world.authoritative.view.as_ref()?.player.skills.get(&id.0));
    match current {
        Some(line) => {
            ui.label(format!(
                "Current: {}  |  trained: {}  |  cap: {}",
                tenths(line.value),
                tenths(line.base),
                tenths(line.cap)
            ))
        }
        None => ui.label("Current value has not arrived from the shard yet."),
    };

    match parse_admin_skill(skill) {
        Ok(command) => {
            if ui.button("Apply to my character").clicked() {
                request.staff_command = Some(command);
            }
        }
        Err(problem) => {
            ui.add_enabled(false, egui::Button::new("Apply to my character"));
            ui.colored_label(ui.visuals().warn_fg_color, problem);
        }
    }
    ui.label(
        egui::RichText::new("The shard confirms the applied value in the journal and updates this row.")
            .small()
            .weak(),
    );
}

fn tenths(value: u16) -> String {
    format!("{}.{}", value / 10, value % 10)
}

fn parse_admin_skill(skill: &crate::desk::AdminSkill) -> Result<String, &'static str> {
    let name: String = skill
        .name
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect();
    if name.is_empty() {
        return Err("Choose a skill.");
    }
    let value = skill.value.trim();
    let valid = value.split_once('.').map_or_else(
        || value.parse::<u16>().ok().and_then(|whole| whole.checked_mul(10)),
        |(whole, fraction)| {
            let whole = whole.parse::<u16>().ok()?;
            let tenth = fraction.parse::<u16>().ok().filter(|_| fraction.len() == 1)?;
            whole.checked_mul(10)?.checked_add(tenth)
        },
    );
    valid.ok_or("Enter a whole value or one decimal place, e.g. 95 or 95.5.")?;
    Ok(format!("{}skill {name} {value}", openshard_commands::PREFIX))
}

struct AnimalCatalogueEntry {
    icon: &'static str,
    name: &'static str,
    id:   u16,
}

trait CatalogueEntry {
    fn icon(&self) -> &'static str;
    fn name(&self) -> &'static str;
}

impl CatalogueEntry for AnimalCatalogueEntry {
    fn icon(&self) -> &'static str {
        self.icon
    }
    fn name(&self) -> &'static str {
        self.name
    }
}

const ANIMALS: &[AnimalCatalogueEntry] = &[
    AnimalCatalogueEntry {
        icon: "🐎",
        name: "Horse",
        id:   1,
    },
    AnimalCatalogueEntry {
        icon: "🐕",
        name: "Dog",
        id:   2,
    },
    AnimalCatalogueEntry {
        icon: "🐈",
        name: "Cat",
        id:   3,
    },
    AnimalCatalogueEntry {
        icon: "🐄",
        name: "Cow",
        id:   4,
    },
    AnimalCatalogueEntry {
        icon: "🐑",
        name: "Sheep",
        id:   5,
    },
    AnimalCatalogueEntry {
        icon: "🐔",
        name: "Chicken",
        id:   6,
    },
    AnimalCatalogueEntry {
        icon: "🐇",
        name: "Rabbit",
        id:   7,
    },
    AnimalCatalogueEntry {
        icon: "🦙",
        name: "Llama",
        id:   8,
    },
    AnimalCatalogueEntry {
        icon: "🐺",
        name: "Grey wolf",
        id:   9,
    },
    AnimalCatalogueEntry {
        icon: "🐻",
        name: "Brown bear",
        id:   10,
    },
];

fn catalogue_grid<T: CatalogueEntry>(ui: &mut egui::Ui, entries: &[T], mut select: impl FnMut(&T)) {
    ui.horizontal_wrapped(|ui| {
        for entry in entries {
            let label = format!("{}\n{}", entry.icon(), entry.name());
            if ui.add_sized([82.0, 56.0], egui::Button::new(label)).clicked() {
                select(entry);
            }
        }
    });
}

/// The staff dye control shared by every way the administrator can create an
/// item. `Hue(0)` is a useful explicit choice too: it means the item's original
/// art, not the first colour in `hues.mul`.
fn admin_hue_panel(
    ui: &mut egui::Ui,
    item: &mut crate::desk::AdminItem,
    art: &openshard_uofiles::art::Art,
    hue_ramp: &openshard_client_render::hue::HueRamp,
    browser: &mut ItemArtCatalogue,
) -> Option<u16> {
    ui.heading("Item colour");
    ui.label("Hue is required for every item created from this page; use 0 for the original colour.");
    let maximum_hue = u16::try_from(hue_ramp.height()).unwrap_or(u16::MAX);
    let mut hue = parse_u16(&item.hue)
        .filter(|hue| *hue <= maximum_hue)
        .unwrap_or_default();
    ui.horizontal(|ui| {
        ui.strong("Hue *");
        ui.add(egui::DragValue::new(&mut hue).range(0..=maximum_hue).speed(1));
    });
    item.hue = hue.to_string();
    let selected_hue = hue;

    ui.horizontal(|ui| {
        ui.label("Test robe");
        let texture = dyed_robe_texture(ui.ctx(), art, hue_ramp, browser, hue);
        match texture {
            Some(texture) => {
                ui.add(egui::Image::from_texture(texture).max_size(egui::vec2(96.0, 112.0)));
            }
            None => {
                ui.label("Robe art is unavailable in this client.");
            }
        }
        ui.small(format!("Hue {hue:#06x}"));
    });

    ui.add_space(4.0);
    ui.label("Palette — click a swatch to choose its hue.");
    if ui.button("Original colour (hue 0)").clicked() {
        item.hue = "0".to_owned();
    }
    const COLUMNS: usize = 12;
    let hue_count = hue_ramp.height() as usize;
    let rows = hue_count.div_ceil(COLUMNS);
    egui::ScrollArea::vertical()
        .id_salt("admin-hue-palette")
        .max_height(240.0)
        .show_rows(ui, 25.0, rows, |ui, visible_rows| {
            for row in visible_rows {
                ui.horizontal(|ui| {
                    for column in 0..COLUMNS {
                        let index = row * COLUMNS + column;
                        if index >= hue_count {
                            break;
                        }
                        let hue = u16::try_from(index + 1).expect("a wire hue fits in u16");
                        let colour = hue_ramp_colour(hue_ramp, hue, 16)
                            .expect("the palette only asks for installed hues");
                        let response = ui.add_sized(
                            [48.0, 21.0],
                            egui::Button::new(format!("{hue:04X}"))
                                .fill(colour)
                                .selected(hue == selected_hue),
                        );
                        if response.clicked() {
                            item.hue = format!("0x{hue:04X}");
                        }
                        response.on_hover_text(format!("Hue {hue:#06x}"));
                    }
                });
            }
        });

    Some(selected_hue)
}

fn hue_ramp_colour(
    hue_ramp: &openshard_client_render::hue::HueRamp,
    hue: u16,
    rung: usize,
) -> Option<egui::Color32> {
    let row = usize::from(hue & 0x3FFF).checked_sub(1)?;
    let at = (row * 32 + rung.min(31)) * 4;
    let pixels = hue_ramp.pixels();
    Some(egui::Color32::from_rgb(
        *pixels.get(at)?,
        *pixels.get(at + 1)?,
        *pixels.get(at + 2)?,
    ))
}

/// Decode the same hue rule as the item shader on one static-art sprite, so
/// the F1 preview is useful before a staff member puts the item in a backpack.
fn dyed_robe_texture<'a>(
    context: &egui::Context,
    art: &openshard_uofiles::art::Art,
    hue_ramp: &openshard_client_render::hue::HueRamp,
    browser: &'a mut ItemArtCatalogue,
    hue: u16,
) -> Option<&'a egui::TextureHandle> {
    if browser
        .dyed_robe
        .as_ref()
        .is_some_and(|(cached, _)| *cached == hue)
    {
        return browser.dyed_robe.as_ref().map(|(_, texture)| texture);
    }
    let image = art.static_art(Graphic(0x1F03)).ok().flatten()?;
    let size = [usize::from(image.width()), usize::from(image.height())];
    let pixels = image
        .pixels()
        .iter()
        .map(|pixel| preview_pixel(*pixel, hue_ramp, hue))
        .collect();
    let image = egui::ColorImage::new(size, pixels);
    match &mut browser.dyed_robe {
        Some((cached, texture)) => {
            texture.set(image, egui::TextureOptions::NEAREST);
            *cached = hue;
        }
        None => {
            browser.dyed_robe = Some((
                hue,
                context.load_texture("admin-dyed-test-robe", image, egui::TextureOptions::NEAREST),
            ));
        }
    }
    browser.dyed_robe.as_ref().map(|(_, texture)| texture)
}

fn preview_pixel(
    pixel: openshard_uofiles::color::Color16,
    hue_ramp: &openshard_client_render::hue::HueRamp,
    hue: u16,
) -> egui::Color32 {
    if pixel.is_transparent() {
        return egui::Color32::TRANSPARENT;
    }
    let rgb = pixel.rgb8();
    let partial = hue & 0x8000 != 0;
    if !partial || (rgb.red == rgb.green && rgb.green == rgb.blue) {
        if let Some(dyed) = hue_ramp_colour(hue_ramp, hue, usize::from(pixel.red())) {
            return dyed;
        }
    }
    egui::Color32::from_rgb(rgb.red, rgb.green, rgb.blue)
}

/// Lazily decoded thumbnails and the filtered definitions behind the F1 item
/// browser. Art is decoded only for rows egui makes visible in the scroll area.
struct ItemArtCatalogue {
    textures:  BTreeMap<(u16, u16), Option<egui::TextureHandle>>,
    /// One recoloured robe, renewed only when the staff member picks another
    /// hue. This is a deliberately small preview cache, unlike the browser's
    /// thumbnails which follow the visible art rows.
    dyed_robe: Option<(u16, egui::TextureHandle)>,
    matching:  Vec<&'static openshard_protocol::house_inventory::HouseCatalogueEntry>,
    key:       Option<(String, crate::desk::AdminItemCategory)>,
}

impl ItemArtCatalogue {
    fn new() -> Self {
        Self {
            textures:  BTreeMap::new(),
            dyed_robe: None,
            matching:  Vec::new(),
            key:       None,
        }
    }
}

fn item_catalogue(
    ui: &mut egui::Ui,
    catalogue: &mut crate::desk::AdminCatalogue,
    art: &openshard_uofiles::art::Art,
    hue_ramp: &openshard_client_render::hue::HueRamp,
    browser: &mut ItemArtCatalogue,
    request: &mut Request,
) {
    ui.horizontal(|ui| {
        ui.label("Category");
        ui.selectable_value(
            &mut catalogue.category,
            crate::desk::AdminItemCategory::All,
            "All",
        );
        ui.selectable_value(
            &mut catalogue.category,
            crate::desk::AdminItemCategory::Weapons,
            "Weapons",
        );
        ui.selectable_value(
            &mut catalogue.category,
            crate::desk::AdminItemCategory::Armor,
            "Armor",
        );
    });
    ui.add(
        egui::TextEdit::singleline(&mut catalogue.query)
            .hint_text("Name, kind, or graphic ID, e.g. dagger or 0x0f52"),
    );
    ui.horizontal(|ui| {
        ui.label("Amount");
        ui.add(
            egui::TextEdit::singleline(&mut catalogue.amount)
                .desired_width(72.0)
                .hint_text("1"),
        );
        ui.checkbox(&mut catalogue.stackable, "Create as one stack");
    });
    let amount = parse_u16(&catalogue.amount).filter(|amount| *amount > 0);
    if amount.is_none() {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            "Amount must be a whole number from 1 to 65535.",
        );
    }

    let key = (catalogue.query.trim().to_ascii_lowercase(), catalogue.category);
    if browser.key.as_ref() != Some(&key) {
        browser.matching = matching_item_entries(&key.0, key.1);
        browser.key = Some(key);
        browser.textures.clear();
    }
    ui.small(format!("{} registered item variants", browser.matching.len()));

    if browser.textures.len() > 192 {
        browser.textures.clear();
    }
    egui::ScrollArea::vertical()
        .id_salt("admin-gameplay-item-catalogue")
        .max_height(280.0)
        .show_rows(ui, 38.0, browser.matching.len(), |ui, rows| {
            // `show_rows` only asks for visible ranges. Copy those references
            // out so thumbnail caching can mutate independently of the match set.
            let visible = browser.matching[rows].to_vec();
            for entry in visible {
                ui.horizontal(|ui| {
                    let texture = item_catalogue_texture(
                        ui.ctx(),
                        art,
                        hue_ramp,
                        &mut browser.textures,
                        entry.graphic,
                        entry.hue,
                    );
                    let clicked = match texture {
                        Some(texture) => {
                            ui.add(
                                egui::Image::from_texture(texture)
                                    .max_size(egui::vec2(36.0, 28.0))
                                    .sense(egui::Sense::click()),
                            )
                            .clicked()
                        }
                        None => ui.add_sized([36.0, 28.0], egui::Button::new("—")).clicked(),
                    };
                    let (kind, material) = semantic_catalogue_identity(entry)
                        .expect("the F1 list contains only valid semantic entries");
                    if ui
                        .selectable_label(
                            false,
                            format!("{}  ·  kind {}  ·  {:#06x}", entry.name, kind.0, entry.graphic.0),
                        )
                        .clicked()
                        || clicked
                    {
                        if let Some(amount) = amount {
                            request.create_item = Some(AdminItemRequest::Kind {
                                kind,
                                material,
                                amount,
                                stackable: catalogue.stackable,
                            });
                        }
                    }
                });
            }
        });
}

fn matching_item_entries(
    query: &str,
    category: crate::desk::AdminItemCategory,
) -> Vec<&'static openshard_protocol::house_inventory::HouseCatalogueEntry> {
    let id_query = parse_u16(query);
    let kind_query = query.strip_prefix("kind:").unwrap_or(query).parse::<u32>().ok();
    openshard_protocol::house_inventory::HOUSE_ITEM_CATALOGUE
        .iter()
        .filter(|entry| semantic_catalogue_identity(entry).is_some())
        .filter(|entry| {
            match category {
                crate::desk::AdminItemCategory::All => true,
                crate::desk::AdminItemCategory::Weapons => entry.tags.contains(&"weapon"),
                crate::desk::AdminItemCategory::Armor => entry.tags.contains(&"armor"),
            }
        })
        .filter(|entry| {
            let (kind, _) =
                semantic_catalogue_identity(entry).expect("invalid semantic entries were removed above");
            query.is_empty()
                || id_query == Some(entry.graphic.0)
                || kind_query == Some(kind.0)
                || entry.name.to_ascii_lowercase().contains(query)
        })
        .collect()
}

/// Keep only exact identities which the shared registry can project to art.
/// This also removes the house-search catalogue's material-less umbrella
/// selectors for material families: those mean "any material" to search, but
/// do not name constructible items.
fn semantic_catalogue_identity(
    entry: &openshard_protocol::house_inventory::HouseCatalogueEntry,
) -> Option<(
    openshard_protocol::item_kind::ItemKindId,
    Option<openshard_protocol::item_kind::MaterialId>,
)> {
    let openshard_protocol::house_inventory::HouseItemIdentity::Semantic { kind, material } = entry.identity
    else {
        return None;
    };
    let is_material_umbrella = material.is_none()
        && openshard_protocol::house_inventory::HOUSE_ITEM_CATALOGUE
            .iter()
            .any(|candidate| {
                matches!(
                    candidate.identity,
                    openshard_protocol::house_inventory::HouseItemIdentity::Semantic {
                        kind: candidate_kind,
                        material: Some(_),
                    } if candidate_kind == kind
                )
            });
    (!is_material_umbrella).then_some((kind, material))
}

fn item_catalogue_texture<'a>(
    context: &egui::Context,
    art: &openshard_uofiles::art::Art,
    hue_ramp: &openshard_client_render::hue::HueRamp,
    textures: &'a mut BTreeMap<(u16, u16), Option<egui::TextureHandle>>,
    graphic: Graphic,
    hue: Hue,
) -> Option<&'a egui::TextureHandle> {
    let key = (graphic.0, hue.0);
    textures.entry(key).or_insert_with(|| {
        let image = art.static_art(graphic).ok().flatten()?;
        let size = [usize::from(image.width()), usize::from(image.height())];
        let pixels = image
            .pixels()
            .iter()
            .map(|pixel| preview_pixel(*pixel, hue_ramp, hue.0))
            .collect();
        Some(context.load_texture(
            format!("admin-item-kind-{:#06x}-{:#06x}", graphic.0, hue.0),
            egui::ColorImage::new(size, pixels),
            egui::TextureOptions::NEAREST,
        ))
    });
    textures.get(&key).and_then(Option::as_ref)
}

fn parse_admin_item(item: &crate::desk::AdminItem) -> Result<AdminItemRequest, &'static str> {
    let graphic = parse_u16(&item.graphic).ok_or("Graphic must be a decimal or 0x hexadecimal number.")?;
    let hue = parse_u16(&item.hue).ok_or("Hue must be a decimal or 0x hexadecimal number.")?;
    let amount = parse_u16(&item.amount)
        .filter(|amount| *amount > 0)
        .ok_or("Amount must be a whole number from 1 to 65535.")?;
    Ok(AdminItemRequest::LegacyArt {
        graphic,
        hue,
        amount,
        stackable: item.stackable,
    })
}

fn parse_u16(text: &str) -> Option<u16> {
    let text = text.trim();
    text.strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .map_or_else(|| text.parse().ok(), |hex| u16::from_str_radix(hex, 16).ok())
}

/// Every number the lighting is turned by, live.
///
/// **The values themselves and not a request.** Every other tab reads a `Hud`
/// and posts what it wants done into [`Request`], because what it is asking for
/// belongs to the app — a camera to move, a tile to walk to. These are the
/// [`Desk`]'s own fields: they are saved with the layout, read straight off it
/// when the next frame's lighting is collected, and there is nothing for the app
/// to decide in between. A round trip through `Request` would be a second copy
/// of each number and one more place for them to disagree.
///
/// The ranges are [`light::Tuning::MOST`] where the number is a factor, so a
/// slider cannot ask for what [`light::Tuning::clamped`] would take back — a
/// control that can be dragged to a value the frame refuses is a control that
/// lies. Time of day and the local night-lighting comparison are included here
/// because the first controls whether a changing shard clock reaches the frame,
/// while the second controls whether the frame is compared under map lights.
fn light_panel(ui: &mut egui::Ui, hud: &Hud, light: &mut crate::desk::Light, request: &mut Request) {
    let most = light::Tuning::MOST;
    ui.label("Time and light");
    let mut time_of_day = hud.time_of_day;
    if ui.checkbox(&mut time_of_day, "use time of day").changed() {
        request.time_of_day = Some(time_of_day);
    }
    let mut night = hud.night;
    if ui.checkbox(&mut night, "night lighting (F10)").changed() {
        request.night = Some(night);
    }
    ui.label(
        egui::RichText::new(
            "Untick time of day to keep the ordinary picture at daylight; F10 or the second box compares it under map lights.",
        )
        .small()
        .weak(),
    );
    ui.separator();
    // What the numbers mean where they mean something in the world's own units,
    // rather than as a bare factor: a person turning "reach" up is asking how
    // far a torch throws, and the answer is a distance in tiles.
    let torch = light::flame(openshard_protocol::wire::Graphic(0x0A0F));

    ui.label("Shadows");
    ui.add(
        egui::Slider::new(&mut light.flame_radius, 0.0..=1.0)
            .text("flame size (tiles)")
            .fixed_decimals(3),
    );
    ui.label(
        egui::RichText::new("0 is a point source and a razor edge; wider is a softer penumbra.")
            .small()
            .weak(),
    );
    ui.add(egui::Slider::new(&mut light.shadow_rays, 1..=light::ShadowRays::MOST).text("rays per flame"));
    ui.label(
        egui::RichText::new("More rays cost the frame and take the grain out of a soft edge.")
            .small()
            .weak(),
    );

    ui.separator();
    ui.label("Flames");
    ui.add(
        egui::Slider::new(&mut light.brightness, 0.0..=most)
            .text("brightness")
            .fixed_decimals(2),
    );
    ui.add(
        egui::Slider::new(&mut light.reach, 0.0..=most)
            .text("reach")
            .fixed_decimals(2),
    );
    ui.label(
        egui::RichText::new(format!("a torch reaches {:.1} tiles", torch.radius * light.reach))
            .small()
            .weak(),
    );

    ui.separator();
    ui.label("Lanterns");
    ui.horizontal(|ui| {
        ui.label("colour");
        ui.color_edit_button_rgb(&mut light.lantern_color);
    });
    ui.label(
        egui::RichText::new(
            "A tint over every lantern the map itself burns — white leaves a \
             torch and a campfire their own colour. Global until light.mul is \
             read, so one lantern cannot yet be told from another.",
        )
        .small()
        .weak(),
    );

    ui.separator();
    ui.label("Headlight");
    ui.horizontal(|ui| {
        ui.label("colour");
        ui.color_edit_button_rgb(&mut light.headlight_color);
    });
    ui.label(
        egui::RichText::new("A tint over the player's own light — the lanterns above are untouched by it.")
            .small()
            .weak(),
    );

    ui.separator();
    ui.label("Ambient");
    ui.add(
        egui::Slider::new(&mut light.sky, 0.0..=most)
            .text("sky")
            .fixed_decimals(2),
    );
    ui.add(
        egui::Slider::new(&mut light.ground, 0.0..=most)
            .text("ground")
            .fixed_decimals(2),
    );
    ui.label(
        egui::RichText::new(
            "Sky is what an open column gets; ground is the floor a windowless \
             cellar still has.",
        )
        .small()
        .weak(),
    );
    ui.horizontal(|ui| {
        ui.label("colour");
        ui.color_edit_button_rgb(&mut light.ambient_color);
    });
    ui.label(
        egui::RichText::new("A tint over both the sky and the floor above, on top of their own brightness.")
            .small()
            .weak(),
    );

    ui.separator();
    ui.label("Sun (F8)");
    ui.add(
        egui::Slider::new(&mut light.sun_azimuth, 0.0..=360.0)
            .text("azimuth (°)")
            .fixed_decimals(0),
    );
    ui.add(
        egui::Slider::new(&mut light.sun_rise, 0.0..=most)
            .text("rise per tile")
            .fixed_decimals(2),
    );
    ui.add(
        egui::Slider::new(&mut light.sun_intensity, 0.0..=most)
            .text("intensity")
            .fixed_decimals(3),
    );
    ui.horizontal(|ui| {
        ui.label("colour");
        ui.color_edit_button_rgb(&mut light.sun_color);
    });

    ui.separator();
    ui.label("World visibility");
    let mut cutaway_disabled = hud.cutaway_disabled;
    if ui
        .checkbox(&mut cutaway_disabled, "disable architectural cutaway")
        .changed()
    {
        request.cutaway_disabled = Some(cutaway_disabled);
    }
    let mut body_overlap_transparency_disabled = hud.body_overlap_transparency_disabled;
    if ui
        .checkbox(
            &mut body_overlap_transparency_disabled,
            "disable neighbour transparency",
        )
        .changed()
    {
        request.body_overlap_transparency_disabled = Some(body_overlap_transparency_disabled);
    }
    ui.label(
        egui::RichText::new(
            "Diagnostic: keeps walls, bridges and roofs opaque while testing the cutaway transparency bug.",
        )
        .small()
        .weak(),
    );

    // The way back. Every number above is remembered across launches, which is
    // what makes this necessary rather than tidy: a client left at somebody's
    // experiment reopens as that experiment, and "what does this look like
    // untouched" has to be one click and not nine numbers typed in.
    if ui.button("back to the defaults").clicked() {
        *light = crate::desk::Light::new();
    }
}

/// Sound controls are live mixer gains, kept in the desk for the same reason
/// the Light tab keeps its numbers: the sliders are the source of truth until
/// the next frame applies their one-shot request to the platform subsystem.
fn audio_panel(ui: &mut egui::Ui, audio: &mut crate::desk::Audio, request: &mut Request) {
    ui.label("Volume");
    let effects_changed = ui
        .add(
            egui::Slider::new(&mut audio.effects, 0.0..=1.0)
                .text("effects")
                .custom_formatter(|value, _| format!("{:.0}%", value * 100.0)),
        )
        .changed();
    let music_changed = ui
        .add(
            egui::Slider::new(&mut audio.music, 0.0..=1.0)
                .text("music")
                .custom_formatter(|value, _| format!("{:.0}%", value * 100.0)),
        )
        .changed();
    if effects_changed || music_changed {
        request.audio = Some(*audio);
    }
    ui.label(
        egui::RichText::new(
            "Effects include world sounds such as swings and spells. Music changes the current track immediately and applies to the next one too.",
        )
        .small()
        .weak(),
    );
    if ui.button("back to the defaults").clicked() {
        *audio = crate::desk::Audio::default();
        request.audio = Some(*audio);
    }
}

/// How big the HUD chat box's glyphs draw, and what colour the player's own
/// line takes.
///
/// The same four role controls apply whichever face is active. A TrueType face
/// rasterizes at the selected pixel size; `fonts.mul` uses the corresponding
/// fractional scale of its baked glyphs. See `docs/render/design_text_sizes.md`.
struct ChatPanel<'a> {
    chat:               &'a mut crate::desk::Chat,
    fonts:              &'a mut crate::desk::FontSizes,
    face:               &'a mut crate::desk::FontFace,
    override_all_fonts: &'a mut bool,
    bitmap_font:        &'a mut crate::desk::BitmapFont,
    ttf_active:         bool,
    ttf_available:      bool,
}

fn chat_panel(ui: &mut egui::Ui, settings: ChatPanel<'_>) {
    let ChatPanel {
        chat,
        fonts,
        face,
        override_all_fonts,
        bitmap_font,
        ttf_active,
        ttf_available,
    } = settings;
    use openshard_client_render::atlas::TextSize;

    use crate::desk::{
        BitmapFont,
        FontFace,
    };

    ui.label("Face");
    ui.radio_value(
        face,
        FontFace::Automatic,
        "Automatic (use configured TrueType when available)",
    );
    ui.radio_value(face, FontFace::Classic, "Classic bitmap (fonts.mul)");
    ui.add_enabled_ui(ttf_available, |ui| {
        ui.radio_value(face, FontFace::TrueType, "Configured TrueType");
    });
    if !ttf_available {
        ui.label(
            egui::RichText::new("The bundled TrueType face was unavailable in this runtime.")
                .small()
                .weak(),
        );
    }
    if !ttf_active {
        ui.separator();
        ui.checkbox(override_all_fonts, "Override all bitmap fonts");
        ui.add_enabled_ui(*override_all_fonts, |ui| {
            let mut selected = bitmap_font.index();
            egui::ComboBox::from_label("classic face")
                .selected_text(format!("fonts.mul #{selected}"))
                .show_ui(ui, |ui| {
                    for index in 0..BitmapFont::COUNT {
                        ui.selectable_value(&mut selected, index, format!("fonts.mul #{index}"));
                    }
                });
            *bitmap_font = BitmapFont::new(selected);
        });
        ui.label(
            egui::RichText::new(
                "When on, speech, chat, tooltips, window captions and pile counts use this face. \
                 When off, each packet/window keeps the face it asked for.",
            )
            .small()
            .weak(),
        );
    }
    ui.separator();
    ui.label("Size");
    // One row per role, each a real pixel size — see `FontSizes`. A tenth of a
    // pixel remains useful for the bitmap path too: its finished quads are
    // allowed to scale fractionally on a dense display.
    let row = |ui: &mut egui::Ui, label: &str, size: &mut TextSize| {
        let mut pixels = size.pixels();
        if ui
            .add(
                egui::Slider::new(&mut pixels, TextSize::MIN..=TextSize::MAX)
                    .step_by(0.1)
                    .suffix(" px")
                    .text(label),
            )
            .changed()
        {
            *size = TextSize::new(pixels);
        }
    };
    row(ui, "speech", &mut fonts.speech);
    row(ui, "window", &mut fonts.window);
    row(ui, "form", &mut fonts.form);
    row(ui, "tooltip", &mut fonts.tooltip);
    row(ui, "count", &mut fonts.stack_count);
    ui.label(
        egui::RichText::new(
            "`speech` is a line over a head and the box below; `window` is this \
             client's own window captions; `form` is server gump text and remains \
             a fixed real size; `tooltip` is hover text; `count` is \
             the number written on a pile. TrueType is rasterized at these real \
             pixel sizes; bitmap glyphs use the same per-role fractional scale.",
        )
        .small()
        .weak(),
    );

    ui.separator();
    ui.label("Colour");
    ui.add(
        egui::DragValue::new(&mut chat.hue)
            .range(0..=u16::MAX)
            .prefix("hue "),
    );
    ui.label(
        egui::RichText::new(
            "Tints the player's own compose line and its caret. 0 is the \
             font's own ink, untinted — what everybody else's line already \
             draws in, since a journal row carries whatever hue the speaker \
             sent.",
        )
        .small()
        .weak(),
    );

    ui.separator();
    if ui.button("back to the defaults").clicked() {
        *chat = crate::desk::Chat::default();
        *fonts = crate::desk::FontSizes::default();
        *face = FontFace::default();
        *override_all_fonts = false;
        *bitmap_font = BitmapFont::default();
    }
}

/// How big the client's own windows draw — a bag, a doll, a shop, a sheet.
///
/// One knob for all of them rather than one per kind: see
/// [`crate::desk::WindowScale`], whose doc says why an item that changed size
/// on its way between two windows is the reason.
fn windows_panel(
    ui: &mut egui::Ui,
    scale: &mut crate::desk::WindowScale,
    status_frame: &mut crate::desk::StatusFrame,
) {
    use crate::desk::{
        StatusFrame,
        WindowScale,
    };

    ui.label("Size");
    let mut factor = scale.factor();
    if ui
        .add(
            egui::Slider::new(&mut factor, WindowScale::MIN..=WindowScale::MAX)
                .step_by(0.05)
                .fixed_decimals(2)
                .text("scale"),
        )
        .changed()
    {
        *scale = WindowScale::new(factor);
    }
    ui.label(
        egui::RichText::new(
            "An upscale on the window art's own pixels, on top of the HUD's \
             zoom. 1.00 is the reference client exactly — which had no display \
             scaling at all, so its windows are postage stamps on a modern \
             screen.",
        )
        .small()
        .weak(),
    );
    ui.label(
        egui::RichText::new(
            "A whole number draws every art pixel as the same square block. A \
             fraction does not: gump art is pixel art, sampled nearest, so 1.50 \
             repeats every other row and leaves a window's border two pixels \
             thick along part of an edge and one along the rest. Worth it for \
             the size, if that is the size you want.",
        )
        .small()
        .weak(),
    );
    ui.label(
        egui::RichText::new(
            "The shard's own tooltip and the HUD chat box are not this — they \
             are drawn over the world as well as over a window, and the chat \
             box has its own scale on the Chat tab.",
        )
        .small()
        .weak(),
    );

    ui.separator();
    ui.label("Status window");
    ui.horizontal(|ui| {
        for choice in [StatusFrame::Old, StatusFrame::Modern] {
            ui.selectable_value(status_frame, choice, choice.label());
        }
    });
    ui.label(
        egui::RichText::new(
            "Which frame the paperdoll's Status button opens. The classic one \
             is 282x151 with its labels painted into the art; the modern one is \
             the 560x196 AoS frame, whose six columns of icons include suit \
             bonuses no item on this shard grants yet.",
        )
        .small()
        .weak(),
    );

    ui.separator();
    if ui.button("back to the defaults").clicked() {
        *scale = WindowScale::default();
        *status_frame = StatusFrame::Old;
    }
}

/// Where the eye is, what it is looking at, and whether it is following.
fn camera_panel(
    ui: &mut egui::Ui,
    hud: &Hud,
    camera: Camera,
    movement: &mut crate::desk::Movement,
    request: &mut Request,
) {
    let eye = camera.eye();
    egui::Grid::new("camera").num_columns(2).show(ui, |ui| {
        ui.label("zoom");
        ui.label(camera.zoom().to_string());
        ui.end_row();
        ui.label("eye");
        ui.label(format!("{}, {} px", eye.x, eye.y));
        ui.end_row();
        ui.label("tile");
        let (x, y) = camera.eye_tile();
        ui.label(format!("{x}, {y}"));
        ui.end_row();
        ui.label("viewport");
        ui.label(format!("{}x{}", camera.width, camera.height));
        ui.end_row();
        ui.label("drawn");
        // The offscreen image, which is the viewport only at zoom 1 and
        // is what the GPU's texture limit applies to.
        ui.label(format!("{}x{}", camera.render_width(), camera.render_height()));
        ui.end_row();
    });
    ui.horizontal(|ui| {
        // The lock is state the player can otherwise only infer from
        // the camera not moving, which is why it is shown as well as
        // toggled.
        let mut locked = hud.locked;
        if ui.checkbox(&mut locked, "follow the body").changed() {
            request.relock = locked;
            request.unlock = !locked;
        }
        if ui.button("return (Home)").clicked() {
            request.relock = true;
            request.unlock = false;
        }
    });
    ui.separator();
    ui.label("Movement");
    if ui.checkbox(&mut movement.always_run, "always run").changed() {
        request.always_run = Some(movement.always_run);
    }
    if ui
        .checkbox(&mut movement.auto_open_doors, "auto open doors")
        .changed()
    {
        request.auto_open_doors = Some(movement.auto_open_doors);
    }
}

/// What the view has decoded, with the serials the renderer drops.
///
/// Reads `world` — the same [`WorldState`] the world pass draws from — directly
/// rather than through a `Hud` snapshot: unlike the camera and the picks, there
/// is no per-frame reading this panel must agree with, so a live borrow costs
/// nothing a clone would have bought.
/// The coarse navigation graph: what this client has, and the one control there
/// is for it.
///
/// **The graph is not a picture**, which is why it sits at the top of the panel
/// about the world rather than among the drawing switches under it: nothing here
/// changes a pixel. What it changes is whether a click further than eight tiles
/// can be answered at all — see `steer.rs`'s `Readings::path`, where a bounded
/// search that failed falls through to the corridor this graph is.
///
/// **Rebuilding is a button and not a setting.** It is the one case a stamp
/// cannot decide: the artifact validates, and a person has a reason to disbelieve
/// it anyway — a bake from a build whose routing rules have since changed under
/// the same `ROUTING_VERSION`, a file that was copied rather than built. Eleven
/// seconds on a facet, on a thread of its own, and the strip counts it up.
fn navigation_panel(ui: &mut egui::Ui, hud: &Hud, request: &mut Request) {
    ui.label("Navigation graph");
    match &hud.navigation {
        Navigation::Absent => {
            ui.label(
                egui::RichText::new(
                    "None. A route past a few tiles is the bounded search alone, so a click \
                     that has to leave a building may be refused.",
                )
                .small()
                .weak(),
            );
        }
        Navigation::Baking { since } => {
            ui.label(format!("building… {:.1}s", since.elapsed().as_secs_f64()));
        }
        Navigation::Ready {
            regions,
            nodes,
            edges,
            path,
        } => {
            ui.label(format!("{regions} regions · {nodes} nodes · {edges} edges"));
            ui.label(egui::RichText::new(path.display().to_string()).small().weak());
        }
    }
    // Nothing to press while one is being built: the worker is already running,
    // and a second one would read the same world into a second hundred megabytes
    // to answer the same question.
    let building = matches!(hud.navigation, Navigation::Baking { .. });
    if ui
        .add_enabled(!building, egui::Button::new("rebake"))
        .on_hover_text(
            "Build the graph again from the world in hand and keep it beside that world. \
             About twenty seconds on a facet; the client keeps playing while it runs.",
        )
        .clicked()
    {
        request.rebake_navigation = true;
    }
}

fn world_panel(ui: &mut egui::Ui, hud: &Hud, world: &WorldState, request: &mut Request) {
    navigation_panel(ui, hud, request);
    ui.separator();
    let view = world.authoritative.view.as_ref();
    // **What the frame draws**, which is the only way to look at a surface
    // something else is standing in front of: the G-buffer holds one answer per
    // pixel, so a wall behind a body is not dimmed or half-shown in a diagnostic,
    // it is simply not in the picture — the body's pixels are the body's. Ticking
    // the crowd off draws the same street with nobody in it. Houses are server
    // multis expanded into item pieces, so they have their own switch: an item
    // dropped under a roof remains visible when only houses are unticked.
    //
    // Everything still stands in the occlusion grid and still casts its own
    // shadow whatever is ticked here — see `frame::Draw`. That is the difference
    // between this and a world with the thing taken out of it, and it is why the
    // label says *drawn*.
    ui.label("Drawn");
    let mut draw = hud.draw;
    let mut changed = false;
    for (on, label) in [
        (&mut draw.land, "land"),
        (
            &mut draw.statics,
            "the map's statics — walls, floors, roofs, furniture",
        ),
        (&mut draw.items, "items — dropped or placed things"),
        (&mut draw.houses, "houses"),
        (&mut draw.mobiles, "mobiles"),
    ] {
        changed |= ui.checkbox(on, label).changed();
    }
    if changed {
        request.draw = Some(draw);
    }
    ui.label(
        egui::RichText::new(
            "Unticked is not removed: the light, the shadows and the grid are the \
             whole world's either way.",
        )
        .small()
        .weak(),
    );

    ui.separator();
    let mut show_interiors = hud.show_interiors;
    if ui
        .checkbox(
            &mut show_interiors,
            "interiors — baked wall topology; whole buildings",
        )
        .changed()
    {
        request.show_interiors = Some(show_interiors);
    }
    if let Some(interiors) = &hud.interiors {
        ui.label(format!(
            "{} buildings in view; whole-house pass; {} doors",
            interiors.buildings,
            interiors.doors.len(),
        ));
        ui.label(
            egui::RichText::new(format!(
                "facet bake — {} coloured tiles in this view; no camera-local topology",
                interiors.cells.len(),
            ))
            .small()
            .weak(),
        );
    } else {
        ui.label(
            egui::RichText::new("off — ordinary rendering is unchanged")
                .small()
                .weak(),
        );
    }
    ui.separator();
    let mut buildings = hud.buildings;
    if ui
        .checkbox(&mut buildings, "Buildings — rooms and floors")
        .changed()
    {
        request.buildings = Some(buildings);
    }
    let mut z_slice = hud.z_slice;
    if ui
        .checkbox(&mut z_slice, "Z band — black outside (diagnostic)")
        .changed()
    {
        request.z_slice = Some(z_slice);
    }
    if z_slice {
        let mut z_slice_view = hud.z_slice_view;
        ui.horizontal(|ui| {
            ui.label("Z range:");
            ui.radio_value(
                &mut z_slice_view,
                openshard_client_render::interiors::ZSliceView::Auto,
                "Auto",
            );
            let (mut lower, mut upper) = match z_slice_view {
                openshard_client_render::interiors::ZSliceView::Auto => (0, 20),
                openshard_client_render::interiors::ZSliceView::Manual { lower, upper } => (lower, upper),
            };
            if ui
                .radio(
                    matches!(
                        z_slice_view,
                        openshard_client_render::interiors::ZSliceView::Manual { .. }
                    ),
                    "Manual",
                )
                .clicked()
            {
                z_slice_view = openshard_client_render::interiors::ZSliceView::Manual { lower, upper };
            }
            if matches!(
                z_slice_view,
                openshard_client_render::interiors::ZSliceView::Manual { .. }
            ) {
                let lower_changed = ui
                    .add(
                        egui::DragValue::new(&mut lower)
                            .range(i8::MIN..=i8::MAX)
                            .prefix("low "),
                    )
                    .changed();
                let upper_changed = ui
                    .add(
                        egui::DragValue::new(&mut upper)
                            .range(i8::MIN..=i8::MAX)
                            .prefix("high "),
                    )
                    .changed();
                if lower_changed || upper_changed {
                    z_slice_view = openshard_client_render::interiors::ZSliceView::Manual { lower, upper };
                }
            }
        });
        if z_slice_view != hud.z_slice_view {
            request.z_slice_view = Some(z_slice_view);
        }
    } else {
        let mut floor_view = hud.floor_view;
        ui.horizontal(|ui| {
            ui.label("floor:");
            ui.radio_value(
                &mut floor_view,
                openshard_client_render::interiors::FloorView::Auto,
                "Auto",
            );
            let mut relative = match floor_view {
                openshard_client_render::interiors::FloorView::Auto => 0,
                openshard_client_render::interiors::FloorView::Manual { relative } => relative,
            };
            if ui
                .radio(
                    matches!(
                        floor_view,
                        openshard_client_render::interiors::FloorView::Manual { .. }
                    ),
                    "Manual",
                )
                .clicked()
            {
                floor_view = openshard_client_render::interiors::FloorView::Manual { relative };
            }
            if matches!(
                floor_view,
                openshard_client_render::interiors::FloorView::Manual { .. }
            ) && ui
                .add(egui::DragValue::new(&mut relative).range(-127..=127))
                .changed()
            {
                floor_view = openshard_client_render::interiors::FloorView::Manual { relative };
            }
        });
        if floor_view != hud.floor_view {
            request.floor_view = Some(floor_view);
        }
    }

    ui.separator();
    let mobiles = view.map_or(0, |view| view.mobiles.len());
    let items = view.map_or(0, |view| view.items.len());
    ui.label(format!("{mobiles} mobiles, {items} ground items"));
    if let Some(view) = view {
        // Sorted, so a `HashMap`'s iteration order does not reshuffle the list
        // under the reader's eyes every frame.
        let mut mobiles: Vec<_> = view.mobiles.iter().collect();
        mobiles.sort_unstable_by_key(|(serial, _)| **serial);
        for (serial, mobile) in mobiles {
            let at = mobile.position;
            ui.label(format!(
                "{serial}  body {}  {}, {}, {}",
                mobile.body.0, at.x, at.y, at.z
            ));
        }
        if !view.items.is_empty() {
            ui.separator();
        }
        let mut items: Vec<_> = view.items.iter().collect();
        items.sort_unstable_by_key(|(serial, _)| **serial);
        for (serial, item) in items {
            let at = item.position;
            ui.label(format!(
                "{serial}  item {}  {}, {}, {}",
                item.graphic.0, at.x, at.y, at.z
            ));
        }
    }
}

/// The route journal's switch, and what it has written this session.
///
/// **Beside the route counts and under the terrain overlay**, because it is the
/// third control about the same question — *where would a click walk* — and the
/// only one of the three whose answer outlives the frame.
///
/// It is on unless somebody turns it off: a route walks into a wall once, in
/// the middle of playing, and a diagnostic that has to be switched on before
/// that session is one that is never on when it matters. The counts under it
/// are what tells a person there is something to replay — see
/// `docs/world/reference/path_journal.md`.
fn path_journal_controls(ui: &mut egui::Ui, hud: &Hud, request: &mut Request) {
    let Some(tally) = &hud.path_journal else {
        return;
    };
    let mut writing = tally.writing;
    if ui
        .checkbox(&mut writing, "route journal — every plan, to path-journal.jsonl")
        .on_hover_text(
            "one line per click and per replan, written as it happens. Afterwards: cargo run \
             --release -p openshard-movement --example path_replay -- --list",
        )
        .changed()
    {
        request.path_journal = Some(writing);
    }
    ui.label(match &tally.stopped {
        // A journal that gave up says so where the switch is, because the
        // switch is the only place anybody would look for it.
        Some(openshard_pathlog::write::Stopped::SizeCap) => {
            format!(
                "stopped at the {} MiB cap — {} orders, {} plans on disk",
                openshard_pathlog::write::SIZE_CAP / (1024 * 1024),
                tally.orders,
                tally.plans,
            )
        }
        Some(openshard_pathlog::write::Stopped::Trouble(error)) => format!("stopped: {error}"),
        None => {
            match tally.writing {
                true => {
                    format!(
                        "{} orders, {} plans, {} KiB",
                        tally.orders,
                        tally.plans,
                        tally.bytes / 1024
                    )
                }
                false => {
                    format!(
                        "off — {} orders, {} plans already written",
                        tally.orders, tally.plans
                    )
                }
            }
        }
    });
}

/// The overlays over the ground, what the cursor is on, and what a click holds.
///
/// Named for the tab and not for [`tile_panel`], which is the readout of *one*
/// tile that this calls twice — for what is hovered and for what is selected.
fn tile_tab(ui: &mut egui::Ui, hud: &Hud, world: &WorldState, request: &mut Request) {
    let mut show = hud.show_terrain;
    if ui
        .checkbox(&mut show, "terrain — walkable green, blocked red")
        .on_hover_text("the route to the cursor is drawn with this on, and to a Ctrl-drag's tile always")
        .changed()
    {
        request.show_terrain = Some(show);
    }
    match &hud.terrain {
        Some(terrain) => {
            ui.label(format!(
                "{} open, {} blocked",
                terrain.open.len(),
                terrain.blocked.len(),
            ));
        }
        // The counts are the overlay's own companion: an empty picture
        // is a client that found nothing and a client that asked
        // nothing, and those look identical on the ground.
        None => {
            ui.label("off");
        }
    }
    // The route's own counts, beside them and not inside them: it is drawn
    // whether that overlay is on or off, and how far the way gets before
    // something is in it is the number worth reading — "12 steps, barred after
    // 5" is a shut door said in figures.
    match &hud.route {
        Some(route) => {
            let walked = route.open.len().saturating_sub(1);
            let label = match route.barred.len() {
                0 => format!("route {walked} steps"),
                barred => format!("route {walked} steps, then {barred} barred"),
            };
            ui.label(label);
            ui.label("route height: low green → high blue");
        }
        None => {
            ui.label("no route");
        }
    }
    path_journal_controls(ui, hud, request);
    ui.separator();
    let mut sight = hud.show_sight;
    if ui
        .checkbox(&mut sight, "sight — the ray a shot is allowed by")
        .on_hover_text(
            "the shard's own line of sight, run here: at whoever you are attacking, \
             or at the tile under the cursor",
        )
        .changed()
    {
        request.show_sight = Some(sight);
    }
    // The reach, named by a person. The shard refuses a shot for two reasons and
    // the ray is only one of them; the other is this number against the distance,
    // and nothing on the wire carries it — see `GraphicsSettings::sight_reach`.
    // A drag value rather than a slider: the interesting numbers are few and
    // exact (1 for a fist, 10 for a bow), and a slider invites sweeping past them.
    ui.horizontal(|ui| {
        let mut reach = hud.sight_reach.get();
        if ui
            .add(
                egui::DragValue::new(&mut reach)
                    .range(1..=u8::MAX)
                    .prefix("reach "),
            )
            .on_hover_text(
                "how far the weapon in your hands strikes, in tiles — 1 is arm's length, \
                 a bow is 10. The shard does not send this, so it is named here; `.sight` \
                 says what the shard itself would use",
            )
            .changed()
        {
            // The knob cannot reach zero, so the newtype cannot fail; a reach of
            // no tiles is not a weapon that strikes its own square.
            request.sight_reach = RangedRange::new(reach);
        }
        ui.label(match &hud.sight {
            Some(sight) => format!("{} tiles away", sight.distance()),
            None => "nothing aimed at".to_owned(),
        });
    });
    // The verdict in words, because the picture says *where* the ray stopped
    // and only this says what stopped it. A refusal names the tile, the art,
    // the span it occupies and the height the ray was at — which is the whole
    // of what "why can I not shoot that" needs answering with.
    match &hud.sight {
        Some(sight) => {
            let aimed = match sight.at_quarry {
                true => "at your quarry",
                false => "at the cursor",
            };
            match sight.trace.stopped {
                None => {
                    // A clear ray is not yet a shot, and this is the line that
                    // used to say it was: the reach test is the other half of the
                    // shard's refusal, and a look that gets there over a distance
                    // an arrow does not cover is exactly the case a person reads
                    // this overlay to understand.
                    let far = match sight.within_reach() {
                        true => String::new(),
                        false => {
                            format!(
                                " — but out of reach, {} > {}",
                                sight.distance(),
                                sight.reach.get()
                            )
                        }
                    };
                    ui.label(format!(
                        "sight {aimed}: clear, {} tiles{far}",
                        sight.trace.steps.len()
                    ));
                }
                Some(step) => {
                    let ray = step.ray_z;
                    let what = match step.stop {
                        Some(Stop::Ground { z }) => format!("ground z {z} over ray {ray}"),
                        Some(Stop::Static {
                            graphic,
                            base,
                            top,
                            wallish,
                        }) => {
                            let reading = match wallish {
                                true => "wall",
                                false => "platform",
                            };
                            format!("{reading} 0x{:04X} z {base}..{top} over ray {ray}", graphic.0)
                        }
                        Some(Stop::Door) => format!("a shut door, ray {ray}"),
                        Some(Stop::LiveWall { base, top }) => {
                            format!("a house wall z {base}..{top} over ray {ray}")
                        }
                        // The verdict is the first stop, so it always has one;
                        // this arm exists because the type says it may not, not
                        // because the picture can reach it.
                        None => "nothing".to_owned(),
                    };
                    ui.label(format!(
                        "sight {aimed}: blocked at ({}, {}), {what}",
                        step.tile.x, step.tile.y
                    ));
                }
            }
        }
        None => {
            ui.label("sight off");
        }
    }
    ui.separator();
    let mut boxes = hud.show_occluders;
    if ui
        .checkbox(
            &mut boxes,
            "occluders — the surfaces that stop light: \
             floor amber, wall red, whole-tile violet, pane cyan",
        )
        .changed()
    {
        request.show_occluders = Some(boxes);
    }
    match &hud.occluders {
        // Both numbers, and the second is the one the picture does not
        // show: an empty picture is a grid with nothing in it and a grid
        // that was never built, and on screen those two are one thing —
        // the same reason the terrain overlay has counts. What the second
        // number is made of is the cut below.
        Some(occluders) => {
            let mut total = 0usize;
            let mut drawn = 0usize;
            for surface in occluders.iter() {
                total += 1;
                if hud.solid_cut.shows(&surface.solid) {
                    drawn += 1;
                }
            }
            ui.label(match hud.solid_cut {
                Cut::Nothing => format!("{drawn} surfaces, the whole grid"),
                Cut::BelowFeet(_) => {
                    format!(
                        "{drawn} surfaces above your feet, {} below and not drawn",
                        total - drawn
                    )
                }
            });
        }
        None => {
            ui.label("off");
        }
    }
    let mut solids = hud.show_solids;
    if ui
        .checkbox(
            &mut solids,
            "…as solids (F5) — the same surfaces given a nominal thickness and \
             drawn as boxes standing in the world, translucent so the art shows \
             through",
        )
        .changed()
    {
        request.show_solids = Some(solids);
    }
    let mut solids_only = hud.solids_only;
    if ui
        .checkbox(
            &mut solids_only,
            "…and nothing else (F3) — the world image skipped, boxes over a \
             blank frame",
        )
        .changed()
    {
        request.solids_only = Some(solids_only);
    }
    let mut solids_opaque = hud.solids_opaque;
    if ui
        .checkbox(
            &mut solids_opaque,
            "…opaque — a straight overwrite instead of blended in, so a \
             nearer face genuinely hides a farther one",
        )
        .changed()
    {
        request.solids_opaque = Some(solids_opaque);
    }
    // The pass's own count and not a second walk of the grid: what is on screen
    // is what the pass drew, and a number derived beside it would be a claim
    // about a list nothing rendered. The pair matters — held against drawn is
    // how much of the grid is off the edge of the picture, which at the widest
    // zoom is most of it.
    match hud.solids {
        (0, 0) => {
            ui.label("off");
        }
        (held, drawn) => {
            ui.label(format!(
                "{drawn} solids drawn, {} off screen",
                held.saturating_sub(drawn)
            ));
        }
    }
    // The second datum, and it governs both views above — see
    // [`Cut`](openshard_client_render::solid::Cut). Standing on a floor, that
    // floor and everything under it are below your feet and are not drawn, so a
    // hole in a floor and a floor the cut took away are the same picture; this
    // is the switch that tells them apart. "Everything" is unreadable in a town
    // on purpose — a pier is a slab on every plank — and that is what makes it
    // an answer to a question rather than a default.
    ui.horizontal(|ui| {
        ui.label("draw (F4)");
        for (cut, name) in [
            (
                Cut::BelowFeet(world.motion.hud_state().predicted.position.z),
                "above your feet",
            ),
            (Cut::Nothing, "everything"),
        ] {
            let picked = std::mem::discriminant(&hud.solid_cut) == std::mem::discriminant(&cut);
            if ui.selectable_label(picked, name).clicked() {
                request.solid_cut = Some(cut);
            }
        }
    });
    ui.separator();
    // The two axes of the highlight, side by side because they are read
    // together: what may be lit, and how an item says it is.
    ui.horizontal(|ui| {
        ui.label("highlight");
        for (target, name) in [
            (HighlightTarget::Auto, "item, else tile"),
            (HighlightTarget::Items, "items"),
            (HighlightTarget::Tiles, "tiles"),
        ] {
            if ui.selectable_label(hud.highlight == target, name).clicked() {
                request.highlight = Some(target);
            }
        }
    });
    ui.horizontal(|ui| {
        ui.label("item shows as");
        for (style, name) in [
            (HighlightStyle::Hue, "hue"),
            (HighlightStyle::Outline, "outline"),
            (HighlightStyle::Both, "both"),
        ] {
            if ui.selectable_label(hud.highlight_style == style, name).clicked() {
                request.highlight_style = Some(style);
            }
        }
    });
    ui.separator();
    ui.label(format!(
        "under cursor — mobile {}, item {}, static {}",
        match hud.pick.mobile {
            Some(index) => index.position().to_string(),
            None => "—".to_string(),
        },
        match hud.pick.item {
            Some(index) => index.position().to_string(),
            None => "—".to_string(),
        },
        // The graphic and the tile it stands on, and *that tile* is the point of
        // printing it: it is the one a click will hold, and it is not the tile
        // under the cursor — a wall's picture stands up the screen from the cell
        // it is built on. The hover readout below names the other one, so the two
        // rows together are the whole of why they differ.
        match &hud.pick.static_ {
            Some(picked) =>
                format!(
                    "0x{:04X} at {}, {}, {}",
                    picked.graphic.0, picked.at.x, picked.at.y, picked.at.z
                ),
            None => "—".to_string(),
        },
    ));
    // The held pick first and the live hover under it — glows cyan on the
    // world. The hover readout changes on every mouse move, so the selection
    // is the one thing on this tab that only changes when the player clicks,
    // which is what makes it worth reading and copying.
    selected_panel(ui, hud.selected.as_ref(), request);
    ui.separator();
    ui.monospace("hover");
    tile_panel(ui, "hover", hud.pick.tile.as_ref(), None);
}

/// What a left click actually landed on, printed as one of the four things it
/// could be rather than a shape that reads the same for a wall as for a body
/// standing on it — see [`Selection`].
fn selected_header(selection: &Selection) -> String {
    match selection {
        Selection::Tile(tile) => format!("selected: TILE  {}, {}", tile.at.x, tile.at.y),
        Selection::Static { static_, .. } => {
            let Graphic(id) = static_.graphic;
            format!(
                "selected: STATIC  {id} (0x{id:04X})  at {}, {}, {}",
                static_.at.x, static_.at.y, static_.at.z
            )
        }
        Selection::Mobile(Some((mobile, _))) => {
            let Graphic(body) = mobile.body;
            let Hue(hue) = mobile.hue;
            let who = match (mobile.you, mobile.serial) {
                (true, _) => "you".to_string(),
                (false, Some(serial)) => serial.to_string(),
                (false, None) => "?".to_string(),
            };
            format!(
                "selected: MOBILE  {who}  body {body} (0x{body:04X})  hue {hue}  at {}, {}, {}",
                mobile.at.x, mobile.at.y, mobile.at.z
            )
        }
        Selection::Mobile(None) => "selected: MOBILE  — walked out of view".to_string(),
        Selection::Item(Some((item, _))) => {
            let Graphic(id) = item.graphic;
            let Hue(hue) = item.hue;
            format!(
                "selected: ITEM  {}  {id} (0x{id:04X})  hue {hue}  at {}, {}, {}",
                item.serial, item.at.x, item.at.y, item.at.z
            )
        }
        Selection::Item(None) => "selected: ITEM  — no longer on the ground".to_string(),
    }
}

/// Which single row of the tile column below [`selected_header`] is the thing
/// the header just named — the header already gives its numbers, but among
/// several statics or items on one tile only this says which line is which.
fn selected_marked(selection: &Selection) -> Option<Marked> {
    match selection {
        Selection::Static { static_, .. } => {
            Some(Marked::Static {
                graphic: static_.graphic,
                height:  Height(static_.at.z),
            })
        }
        Selection::Item(Some((item, _))) => {
            Some(Marked::Item {
                graphic: item.graphic,
                height:  Height(item.at.z),
            })
        }
        _ => None,
    }
}

/// What "copy" on the selected panel puts on the clipboard — the header and
/// the tile column under it, the same two things the panel shows.
fn selected_text(selection: &Selection) -> String {
    let mut text = selected_header(selection);
    text.push('\n');
    if let Some(tile) = selection.tile() {
        text.push_str(&tile_text(tile));
    }
    text
}

/// The panel for [`Hud::selected`]: an explicit header naming which of the
/// four kinds a click landed on, and the tile column under it with that one
/// row marked. Monospace throughout and nothing but `Label`s in it, so
/// dragging across any part of it — the header, one row, or the whole box —
/// selects that text the way a terminal does, with a `Ctrl+C` at the end of
/// the drag rather than a "copy" button beside every number.
fn selected_panel(ui: &mut egui::Ui, selection: Option<&Selection>, request: &mut Request) {
    let Some(selection) = selection else {
        ui.monospace("selected: —  (click the world to hold something)");
        return;
    };
    ui.horizontal(|ui| {
        ui.monospace(selected_header(selection));
        if ui.small_button("copy").clicked() {
            ui.ctx().copy_text(selected_text(selection));
        }
    });
    tile_panel(ui, "selected", selection.tile(), selected_marked(selection));
    if let Selection::Static {
        static_,
        prism: Some(prism),
        ..
    } = selection
    {
        prism_editor(ui, static_.graphic, *prism, request);
    }
}

/// A live, in-memory edit to a selected stair's own prism: drag a height or
/// pick a different climb axis and the same static redraws with it, in the
/// running client, on the next frame.
///
/// Sandbox only — see [`Request::authored_prism`] and `App::apply`. Nothing
/// here writes to disk; this is for finding the right numbers by eye before
/// they are written down by hand in the art table this graphic's row lives
/// in.
fn prism_editor(ui: &mut egui::Ui, graphic: Graphic, prism: Prism, request: &mut Request) {
    ui.separator();
    ui.monospace("edit prism");
    let mut up = prism.up();
    let mut heights: Vec<u8> = prism.treads().to_vec();
    let mut changed = false;
    ui.horizontal(|ui| {
        for face in [Face::North, Face::East, Face::South, Face::West] {
            changed |= ui.selectable_value(&mut up, face, format!("{face:?}")).changed();
        }
    });
    for (tread, height) in heights.iter_mut().enumerate() {
        // A generous sandbox bound and not `facing::MAX_PRISM`, which is
        // private to the crate that runs the fit search this editor plays no
        // part in — see the module doc.
        changed |= ui
            .add(egui::Slider::new(height, 0..=40).text(format!("tread {tread}")))
            .changed();
    }
    if changed {
        if let Some(edited) = Prism::new(up, &heights) {
            request.authored_prism = Some((graphic, edited));
        }
    }
}

/// The three things drawn *on* the world rather than beside it: the terrain
/// wash, the occluder boxes, and the tile markers.
///
/// Taken out of [`layout`] with the rest of the panels' bodies, and for the same
/// reason — what is left in `layout` is then the arrangement, one screenful of
/// it, and nothing else.
fn draw_world_overlays(
    context: &egui::Context,
    hud: &Hud,
    camera: Camera,
    viewport: egui::Rect,
    map_editor: &mut crate::editor_mode::MapEditor,
) {
    // Every panel has claimed its edge by now, so what is left of the root `Ui`
    // is the world's own rectangle — the very rect `Shell::run` reads back a
    // moment later and hands the camera. Read *here*, at the foot of the layout
    // and not in the middle of it: taken before the speech strip took its edge,
    // this was a rectangle the world is not drawn in, and the markers clipped to
    // it were painted over the strip. Windows do not narrow it and must not: they
    // float over the world, and a marker under one is correctly hidden by it
    // rather than clipped away.
    let world = world_painter(context, viewport);
    // The terrain map goes down first: it is a wash over the ground, and the
    // three markers below are read against it.
    if let Some(terrain) = &hud.terrain {
        draw_terrain(&world, &camera, terrain, viewport.min);
    }
    if let Some(interiors) = hud.interiors.as_ref().filter(|_| hud.show_interiors) {
        draw_interiors(&world, &camera, interiors, viewport.min);
    }
    // Then what stands up out of it. Over the wash and under the markers: the
    // boxes are read against the ground the wash colours, and a highlight the
    // player is pointing with must not be hidden by a diagnostic.
    //
    // The **wireframe** only. Its neighbour, the solids view, is a real pass in
    // the frame (`openshard_client_render::solids`) and is drawn before this
    // ever runs — deliberately, so that a stroke of the plane the walk actually
    // tests lands over the box that only says how thick it is drawn. That order
    // is the pair of facts the two views exist to be read as; the other one
    // hides the measurement behind the drawing.
    if let Some(occluders) = hud.occluders.as_ref().filter(|_| hud.show_occluders) {
        draw_occluders(&world, &camera, occluders, hud.solid_cut, viewport.min);
    }
    // The way the body is going, over the wash and under everything the cursor
    // is doing: it is a standing answer to an order already given, and must not
    // cover the marker that says what the *next* click would do.
    if let Some(route) = &hud.route {
        draw_route(&world, &camera, route, viewport.min);
    }
    // The ray over the route, and drawn after it: where a walk and a look
    // disagree — a wall a body goes round and an arrow does not — the answer
    // being read is the look's.
    if let Some(sight) = hud.sight.as_ref().filter(|_| hud.show_sight) {
        draw_sight(&world, &camera, sight, viewport.min);
    }
    for tile in &hud.editor_preview {
        let corners = facet_corners(
            &world,
            &camera,
            openshard_protocol::world::Point {
                x: tile.x,
                y: tile.y,
                z: tile.corners[0],
            },
            tile.corners,
            viewport.min,
        );
        world.add(egui::Shape::convex_polygon(
            corners,
            egui::Color32::from_rgba_unmultiplied(255, 70, 220, 45),
            egui::Stroke::new(1.5, egui::Color32::from_rgb(255, 90, 225)),
        ));
    }
    for item in &hud.editor_static_draft {
        if let Some(texture) = map_editor.static_preview_texture(context, item.tile) {
            draw_editor_static_preview(
                &world,
                &camera,
                openshard_protocol::world::Point::new(item.x, item.y, item.z),
                texture,
                viewport.min,
                205,
            );
        }
    }
    if let Some((at, graphic)) = hud.editor_static_preview {
        if let Some(texture) = map_editor.static_preview_texture(context, graphic) {
            draw_editor_static_preview(&world, &camera, at, texture, viewport.min, 135);
        }
    }
    draw_health_bars(&world, &camera, &hud.health_bars, viewport.min);
    draw_action_bars(&world, &camera, &hud.action_bars, viewport.min);
    // The tile marker, and only when the tile is what is lit: an item under the
    // cursor takes the highlight, and a diamond drawn under its ring would be
    // the client answering "what would a click do here" twice.
    //
    // The ring first and underneath: it is the relief the marker is read
    // against — which way the ground runs, where the stair's next tread is —
    // and a neighbour drawn over the tile being pointed at would compete with
    // the answer instead of framing it. Bare wireframes, no fill: eight filled
    // boxes around one is a lantern, and the tile under the cursor stops being
    // the brightest thing on screen.
    if hud.hover_lit {
        for tile in &hud.pick.neighbours {
            draw_tile_highlight(
                &world,
                &camera,
                tile,
                viewport.min,
                egui::Color32::TRANSPARENT,
                egui::Stroke::new(1.2, egui::Color32::from_rgba_unmultiplied(255, 255, 0, 170)),
            );
        }
    }
    if let Some(tile) = hud.pick.tile.as_ref().filter(|_| hud.hover_lit) {
        draw_tile_highlight(
            &world,
            &camera,
            tile,
            viewport.min,
            egui::Color32::from_rgba_unmultiplied(255, 255, 0, 40),
            egui::Stroke::new(1.5, egui::Color32::from_rgba_unmultiplied(255, 255, 0, 180)),
        );
    }
    if let Some(tile) = hud.selected.as_ref().and_then(Selection::tile) {
        draw_tile_highlight(
            &world,
            &camera,
            tile,
            viewport.min,
            egui::Color32::from_rgba_unmultiplied(0, 220, 255, 60),
            egui::Stroke::new(2.5, egui::Color32::from_rgb(0, 220, 255)),
        );
    }
    // Where the body is walking to, and gone the moment it arrives or gives up.
    if let Some(tile) = &hud.goal {
        draw_tile_highlight(
            &world,
            &camera,
            tile,
            viewport.min,
            egui::Color32::from_rgba_unmultiplied(0, 255, 120, 50),
            egui::Stroke::new(2.0, egui::Color32::from_rgb(0, 255, 120)),
        );
    }
}

fn draw_health_bars(
    painter: &egui::Painter,
    camera: &Camera,
    bars: &[HealthBar],
    viewport_origin: egui::Pos2,
) {
    const WIDTH: f32 = 42.0;
    const HEIGHT: f32 = 5.0;
    const RESOURCE_GAP: f32 = 2.0;
    const GAP: f32 = 8.0;

    // The anchor belongs to the rendered world, not to egui. Project it
    // through the camera first: this spends the same zoom/blit transform as
    // the world image and prevents a second, guessed projection here.
    let physical_to_points = 1.0 / painter.ctx().pixels_per_point();
    for bar in bars {
        let ratio = if bar.max.get() == 0 {
            0.0
        } else {
            f32::from(bar.current.get()).min(f32::from(bar.max.get())) / f32::from(bar.max.get())
        };
        let estimated_ratio = if bar.max.get() == 0 {
            0.0
        } else {
            f32::from(bar.estimated.get()).min(f32::from(bar.max.get())) / f32::from(bar.max.get())
        };
        let projected = camera.to_viewport(bar.anchor);
        let centre = viewport_origin
            + egui::vec2(
                projected.x * physical_to_points,
                projected.y * physical_to_points - GAP,
            );
        let rect = egui::Rect::from_center_size(centre, egui::vec2(WIDTH, HEIGHT));
        let fill = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() * ratio, rect.height()));
        painter.rect_filled(
            rect.expand(1.0),
            1.0,
            egui::Color32::from_rgba_unmultiplied(0, 0, 0, 180),
        );
        painter.rect_filled(fill, 1.0, health_colour(bar.notoriety));
        // A delayed estimate is the "fake HP" familiar from HotS. Damage
        // leaves red health behind the authoritative bar; healing leaves a
        // green preview ahead of it. Each packet retargets the estimate, so a
        // DoT reads as a chain of ticks instead of a series of hard snaps.
        if estimated_ratio > ratio {
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(rect.left() + rect.width() * ratio, rect.top()),
                    egui::pos2(rect.left() + rect.width() * estimated_ratio, rect.bottom()),
                ),
                1.0,
                egui::Color32::from_rgba_unmultiplied(220, 55, 45, 210),
            );
        } else if estimated_ratio < ratio {
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(rect.left() + rect.width() * estimated_ratio, rect.top()),
                    egui::pos2(rect.left() + rect.width() * ratio, rect.bottom()),
                ),
                1.0,
                egui::Color32::from_rgba_unmultiplied(75, 225, 125, 210),
            );
        }
        let stroke = match bar.targeted {
            true => egui::Stroke::new(1.2, egui::Color32::from_rgb(255, 230, 80)),
            false => egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(20, 20, 20, 220)),
        };
        painter.rect_stroke(rect, 1.0, stroke, egui::StrokeKind::Middle);
        if let Some(mana) = bar.mana {
            let ratio = if mana.max.get() == 0 {
                0.0
            } else {
                f32::from(mana.current.get()).min(f32::from(mana.max.get())) / f32::from(mana.max.get())
            };
            let mana_rect = rect.translate(egui::vec2(0.0, HEIGHT + RESOURCE_GAP));
            painter.rect_filled(
                mana_rect.expand(1.0),
                1.0,
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 180),
            );
            painter.rect_filled(
                egui::Rect::from_min_size(
                    mana_rect.min,
                    egui::vec2(mana_rect.width() * ratio, mana_rect.height()),
                ),
                1.0,
                egui::Color32::from_rgb(75, 120, 255),
            );
            painter.rect_stroke(
                mana_rect,
                1.0,
                egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(20, 20, 20, 220)),
                egui::StrokeKind::Middle,
            );
        }
    }
}

/// The preparation bars, and beside each one the state it is in.
///
/// `docs/combat/evidence/2026-08-27-the-action-phases.md`'s Ф4. Three marks and not
/// one, and the two extra
/// ones are the point: a bar answers *how far along*, and it cannot answer
/// *what of* — a blow and a drawn bow fill the same rectangle — nor *and then
/// what happened*, which is the question a fight actually leaves behind. So the
/// glyph on the left names what is being prepared, the filling names the phase,
/// and the word on the right carries the last outcome, including the one no
/// picture in this client has ever been able to state: the reason an action
/// stopped.
///
/// The two halves are drawn together on purpose. A fighter's next gesture opens
/// on the tick the last one landed, so an outcome drawn *instead of* a bar would
/// be legible only for the final blow of a fight — see `crowd::ActionRecord`,
/// where the measurement is. An exchange therefore reads as a bar filling with
/// the previous blow's verdict standing beside it.
///
/// Above the health line rather than below it. The health bar is what a player
/// reads first and its place is learned; a row inserted underneath would move
/// the mana bar every time somebody swung.
fn draw_action_bars(
    painter: &egui::Painter,
    camera: &Camera,
    bars: &[ActionBar],
    viewport_origin: egui::Pos2,
) {
    const WIDTH: f32 = 42.0;
    const HEIGHT: f32 = 3.0;
    // Clear of the health bar, which `draw_health_bars` centres 8 above the
    // anchor and draws 5 tall: its top edge is 10.5 up, and this is the first
    // row above that does not touch it.
    const GAP: f32 = 15.0;
    // The glyph's box, and how far its centre sits from the bar's left edge.
    const GLYPH: f32 = 8.0;
    const MARGIN: f32 = 4.0;

    let physical_to_points = 1.0 / painter.ctx().pixels_per_point();
    let font = egui::FontId::proportional(9.0);
    for bar in bars {
        let projected = camera.to_viewport(bar.anchor);
        let centre = viewport_origin
            + egui::vec2(
                projected.x * physical_to_points,
                projected.y * physical_to_points - GAP,
            );
        let rect = egui::Rect::from_center_size(centre, egui::vec2(WIDTH, HEIGHT));
        let glyph_at = egui::pos2(rect.left() - MARGIN - GLYPH / 2.0, rect.center().y);
        let label_at = egui::pos2(rect.right() + MARGIN, rect.center().y);
        // What is being prepared, if anything. No bar at all between gestures:
        // an empty rectangle reads as a fighter about to do something, and the
        // one thing a body between blows is not doing is preparing.
        if let Some(running) = bar.progress.running {
            let colour = action_colour(running.kind);
            painter.rect_filled(
                rect.expand(1.0),
                1.0,
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 180),
            );
            match running.fill {
                ActionFill::Arming { filled } |
                // The whole bar, held: an armed action is *ready*, and a
                // fraction here would be its endurance running out — which is
                // not what a watcher is being told. The stroke is what says it
                // is waiting rather than landing.
                ActionFill::Releasing { filled } => painter.rect_filled(
                    egui::Rect::from_min_size(
                        rect.min,
                        egui::vec2(rect.width() * filled.clamp(0.0, 1.0), rect.height()),
                    ),
                    1.0,
                    colour,
                ),
                ActionFill::Armed => painter.rect_filled(rect, 1.0, colour),
            };
            let stroke = match running.fill {
                ActionFill::Armed => egui::Stroke::new(1.2, egui::Color32::from_rgb(255, 230, 80)),
                ActionFill::Arming { .. } | ActionFill::Releasing { .. } => {
                    egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(20, 20, 20, 220))
                }
            };
            painter.rect_stroke(rect, 1.0, stroke, egui::StrokeKind::Middle);
            draw_kind_glyph(painter, glyph_at, GLYPH, colour, running.kind);
        }
        // A fighter who wants to act and cannot. It takes the bar's own place
        // rather than a line of its own, because it is the *answer to the same
        // question*: there is no bar here, and this is why there is no bar here.
        // Nothing else can be in that place at the same time — the shard clears
        // a refusal at the commit that ends it.
        if let Some(reason) = bar.progress.balked {
            let colour = balk_colour();
            painter.text(
                rect.left_center(),
                egui::Align2::LEFT_CENTER,
                interrupt_label(reason),
                font.clone(),
                colour,
            );
            // The left box is shared with the outcome mark, and for the moment
            // after a blow both are true: the swing missed *and* the next one
            // cannot start. The verdict has a hold of barely a second and this
            // has none at all, so the fading one gets the box and this takes it
            // back when it goes.
            if bar.progress.ended.is_none() {
                draw_balk_glyph(painter, glyph_at, GLYPH, colour);
            }
        }
        // How the last one ended, on the right, in its own colour. It outlives
        // the action it belongs to and fades on its own — see
        // `crowd::OUTCOME_HOLD` — so this is the one mark that can be here with
        // no bar under it, and the one that stays while the next bar fills.
        if let Some(outcome) = bar.progress.ended {
            let colour = outcome_colour(outcome);
            painter.text(
                label_at,
                egui::Align2::LEFT_CENTER,
                outcome_label(outcome),
                font.clone(),
                colour,
            );
            // With nothing running the left box is free, and the ending takes
            // it: a word alone beside an empty stretch of sky is harder to
            // attach to a body than a mark where the glyph always is.
            if bar.progress.running.is_none() {
                draw_outcome_glyph(painter, glyph_at, GLYPH, colour, outcome);
            }
        } else if let Some(running) = bar.progress.running {
            // The state in a word, and only when no outcome is competing for
            // the same place: what a fighter *is* doing is already drawn twice
            // over there, and what just happened is the scarcer fact.
            let (state, context) = action_state_labels(
                running.kind,
                running.fill,
                running.stage,
                running.released_from_held_draw,
            );
            let state_rect = painter.text(
                label_at,
                egui::Align2::LEFT_CENTER,
                state,
                font.clone(),
                action_colour(running.kind),
            );
            if let Some(context) = context {
                painter.text(
                    egui::pos2(state_rect.right() + MARGIN, label_at.y),
                    egui::Align2::LEFT_CENTER,
                    context,
                    font.clone(),
                    egui::Color32::from_rgb(255, 230, 80),
                );
            }
        }
    }
}

/// What each kind of impact is drawn as.
///
/// Drawn and not written: a glyph out of a font this client does not ship is a
/// box on somebody else's machine, and these have to read at eight pixels beside
/// a moving body.
fn draw_kind_glyph(
    painter: &egui::Painter,
    centre: egui::Pos2,
    size: f32,
    colour: egui::Color32,
    kind: CombatActionKind,
) {
    let half = size / 2.0;
    let stroke = egui::Stroke::new(1.4, colour);
    match kind {
        // A stroke through the box: the blade's path, which is the whole of what
        // a swing is.
        CombatActionKind::Swing => {
            painter.line_segment(
                [
                    egui::pos2(centre.x - half, centre.y + half),
                    egui::pos2(centre.x + half, centre.y - half),
                ],
                stroke,
            );
        }
        // A shaft and a head, pointing the way it will travel.
        CombatActionKind::Shot => {
            painter.line_segment(
                [
                    egui::pos2(centre.x - half, centre.y),
                    egui::pos2(centre.x + half, centre.y),
                ],
                stroke,
            );
            for corner in [centre.y - half * 0.7, centre.y + half * 0.7] {
                painter.line_segment(
                    [
                        egui::pos2(centre.x + half, centre.y),
                        egui::pos2(centre.x, corner),
                    ],
                    stroke,
                );
            }
        }
        // A cone widening away from the mouth: what a breath does that an arrow
        // does not is spread.
        CombatActionKind::Breath => {
            for corner in [centre.y - half, centre.y + half] {
                painter.line_segment(
                    [
                        egui::pos2(centre.x - half, centre.y),
                        egui::pos2(centre.x + half, corner),
                    ],
                    stroke,
                );
            }
        }
    }
}

/// What each ending is drawn as: it landed, it found air, it was stopped, or the
/// arm gave out.
fn draw_outcome_glyph(
    painter: &egui::Painter,
    centre: egui::Pos2,
    size: f32,
    colour: egui::Color32,
    outcome: CombatActionOutcome,
) {
    let half = size / 2.0;
    let stroke = egui::Stroke::new(1.4, colour);
    match outcome {
        CombatActionOutcome::Hit => {
            painter.circle_filled(centre, half * 0.8, colour);
        }
        CombatActionOutcome::Miss => {
            painter.circle_stroke(centre, half * 0.8, stroke);
        }
        // A cross, the one mark that reads as *stopped* rather than as any
        // amount of anything.
        CombatActionOutcome::Interrupted(_) => {
            for slope in [-1.0, 1.0] {
                painter.line_segment(
                    [
                        egui::pos2(centre.x - half, centre.y + half * slope),
                        egui::pos2(centre.x + half, centre.y - half * slope),
                    ],
                    stroke,
                );
            }
        }
        // A bar and nothing crossing it: the wait ran out, and nothing happened
        // at all.
        CombatActionOutcome::Expired => {
            painter.line_segment(
                [
                    egui::pos2(centre.x - half, centre.y),
                    egui::pos2(centre.x + half, centre.y),
                ],
                stroke,
            );
        }
    }
}

/// The colour a kind of impact is drawn in, kept apart from the health palette
/// on purpose: a bar over a body already means notoriety, and a second bar in
/// the same colours would be read as more of the same fact.
fn action_colour(kind: CombatActionKind) -> egui::Color32 {
    match kind {
        CombatActionKind::Swing => egui::Color32::from_rgb(190, 200, 215),
        CombatActionKind::Shot => egui::Color32::from_rgb(235, 185, 90),
        CombatActionKind::Breath => egui::Color32::from_rgb(215, 95, 205),
    }
}

/// The colour an ending is drawn in. An interruption is the shard's *answer* to
/// a question the player asked by attacking, so it is the one that is warned
/// about rather than merely reported.
fn outcome_colour(outcome: CombatActionOutcome) -> egui::Color32 {
    match outcome {
        CombatActionOutcome::Hit => egui::Color32::from_rgb(220, 55, 45),
        CombatActionOutcome::Miss => egui::Color32::from_rgb(180, 180, 185),
        CombatActionOutcome::Interrupted(_) => egui::Color32::from_rgb(255, 200, 60),
        CombatActionOutcome::Expired => egui::Color32::from_rgb(140, 140, 160),
    }
}

/// What a running action says about itself in one word.
///
/// A released action names the *stretch* it is in and not merely its kind: a bow
/// coming up, a bow bending and a bow held on a mark are three different things
/// to be looking at, and until the shard began announcing them they were all the
/// word "shot" beside a rectangle. The stretch is the shard's own — see
/// `state::action_stages` — so the word here is a translation and never a guess.
///
/// An armed action ignores the stretch, because *held* is the whole of what it
/// is doing: it is not part way through anything, it is waiting on the world.
fn action_state_labels(
    kind: CombatActionKind,
    fill: ActionFill,
    stage: ActionStage,
    released_from_held_draw: bool,
) -> (&'static str, Option<&'static str>) {
    let state = match fill {
        ActionFill::Arming { .. } => {
            match (kind, stage) {
                (CombatActionKind::Swing, ActionStage::Ready) => "raising",
                (CombatActionKind::Swing, _) => "winding up",
                (CombatActionKind::Shot, ActionStage::Ready) => "raising bow",
                (CombatActionKind::Shot, _) => "drawing",
                (CombatActionKind::Breath, ActionStage::Ready) => "rearing",
                (CombatActionKind::Breath, _) => "inhaling",
            }
        }
        ActionFill::Armed => {
            match kind {
                CombatActionKind::Swing => "swing · held",
                CombatActionKind::Shot => "aim · held",
                CombatActionKind::Breath => "breath · held",
            }
        }
        ActionFill::Releasing { .. } => {
            match (kind, stage) {
                (CombatActionKind::Swing, ActionStage::Ready) => "raising",
                (CombatActionKind::Swing, ActionStage::Load) => "winding up",
                (CombatActionKind::Swing, ActionStage::Aim) => "set",
                (CombatActionKind::Swing, ActionStage::Release) => "striking",
                (CombatActionKind::Shot, ActionStage::Ready) => "raising bow",
                (CombatActionKind::Shot, ActionStage::Load) => "drawing",
                (CombatActionKind::Shot, ActionStage::Aim) => "aiming",
                (CombatActionKind::Shot, ActionStage::Release) => "loosing",
                (CombatActionKind::Breath, ActionStage::Ready) => "rearing",
                (CombatActionKind::Breath, ActionStage::Load) => "inhaling",
                (CombatActionKind::Breath, ActionStage::Aim) => "fixing",
                (CombatActionKind::Breath, ActionStage::Release) => "breathing",
            }
        }
    };
    let context = (kind == CombatActionKind::Shot && released_from_held_draw).then_some("bow drawn");
    (state, context)
}

/// The colour a standing refusal is drawn in.
///
/// Neither a kind's colour nor an outcome's: what it says is *nothing is
/// happening, and here is why*, which is a third thing. Dimmer than an
/// interruption's warning yellow, because it is a state a fighter can sit in
/// harmlessly for a minute, not an event that just cost them a blow.
fn balk_colour() -> egui::Color32 {
    egui::Color32::from_rgb(170, 155, 110)
}

/// A refusal's mark: a bar with a slash through it — *not this, and here is what
/// is in the way*. Deliberately unlike the interruption cross, which is an
/// action that was stopped rather than one that never started.
fn draw_balk_glyph(painter: &egui::Painter, centre: egui::Pos2, size: f32, colour: egui::Color32) {
    let half = size / 2.0;
    let stroke = egui::Stroke::new(1.4, colour);
    painter.circle_stroke(centre, half * 0.85, stroke);
    painter.line_segment(
        [
            egui::pos2(centre.x - half * 0.6, centre.y + half * 0.6),
            egui::pos2(centre.x + half * 0.6, centre.y - half * 0.6),
        ],
        stroke,
    );
}

/// Why an action stopped, or why one cannot start — the same list read by two
/// different questions, and deliberately one function so the two never drift
/// into two vocabularies for one fact.
fn interrupt_label(reason: InterruptReason) -> &'static str {
    match reason {
        InterruptReason::TargetGone => "target gone",
        InterruptReason::OutOfReach => "out of reach",
        InterruptReason::NoLineOfSight => "no line of sight",
        InterruptReason::Pacified => "pacified",
        InterruptReason::Abandoned => "abandoned",
        InterruptReason::NoAmmo => "no ammo",
        InterruptReason::Moved => "moved",
        InterruptReason::Struck => "struck",
        InterruptReason::NoTarget => "no target",
    }
}

/// How an action ended, in the words the wire's own reasons are named by.
///
/// Every one of `InterruptReason`'s arms has a phrase here rather than a default:
/// the reason a swing vanished is the whole thing this packet was added to say,
/// and a new arm that fell through to "stopped" would silently un-say it for
/// exactly the case somebody had just gone to the trouble of naming.
fn outcome_label(outcome: CombatActionOutcome) -> &'static str {
    match outcome {
        CombatActionOutcome::Hit => "hit",
        CombatActionOutcome::Miss => "miss",
        CombatActionOutcome::Expired => "expired",
        CombatActionOutcome::Interrupted(reason) => interrupt_label(reason),
    }
}

fn health_colour(notoriety: Notoriety) -> egui::Color32 {
    match notoriety {
        Notoriety::Innocent => egui::Color32::from_rgb(70, 150, 255),
        Notoriety::Friend => egui::Color32::from_rgb(70, 210, 110),
        Notoriety::Neutral | Notoriety::Criminal => egui::Color32::from_rgb(170, 170, 170),
        Notoriety::Enemy => egui::Color32::from_rgb(230, 145, 55),
        Notoriety::Murderer => egui::Color32::from_rgb(220, 55, 45),
        Notoriety::Invulnerable => egui::Color32::from_rgb(240, 220, 70),
        _ => egui::Color32::from_rgb(170, 170, 170),
    }
}

/// The scope: what the eye is doing, what it is doing it with, and a scenario
/// to make it do it.
///
/// `docs/client/evidence/2026-08-14-the-camera-rig-record.md`, C4. From here on every remaining decision about the camera
/// is a matter of looking rather than arguing, and this is what there is to look
/// at: a preset and a slider per number, the last few seconds of the eye's own
/// speed and jerk, the same [`Metrics`] the offline bench prints, and the bench's
/// scenarios walked by the real body.
///
/// The numbers and the curves come off one arithmetic — `bench::readings` — so a
/// figure that disagrees with the shape beside it means the metric is wrong,
/// which is a thing to be able to see rather than to reason about.
fn rig_panel(ui: &mut egui::Ui, hud: &Hud, world: &WorldState, request: &mut Request) {
    let mut rig = hud.rig;
    ui.horizontal(|ui| {
        ui.label("preset");
        // The two that exist, and neither is called `DEFAULT`: which camera
        // this client ships is decided on this panel, not in a name.
        if ui.button("HARD").clicked() {
            rig = Rig::HARD;
        }
        if ui.button("LIFT").clicked() {
            rig = Rig::LIFT;
        }
    });
    egui::Grid::new("rig").num_columns(2).show(ui, |ui| {
        ui.label("plane τ");
        ui.add(egui::Slider::new(&mut rig.plane_tau, 0.0..=0.5).suffix(" s"));
        ui.end_row();
        ui.label("lift τ");
        ui.add(egui::Slider::new(&mut rig.lift_tau, 0.0..=0.5).suffix(" s"));
        ui.end_row();
        ui.label("lift cut");
        ui.horizontal(|ui| {
            // Infinity is a real setting — it never cuts — and it is not a
            // point on a slider, so it is the checkbox and the slider holds
            // what the last finite value was.
            let mut cuts = rig.lift_cut.is_finite();
            let mut pixels = match rig.lift_cut.is_finite() {
                true => rig.lift_cut,
                false => openshard_client_render::follow::FLOOR,
            };
            ui.checkbox(&mut cuts, "");
            ui.add_enabled(cuts, egui::Slider::new(&mut pixels, 0.0..=256.0).suffix(" px"));
            rig.lift_cut = match cuts {
                true => pixels,
                false => f32::INFINITY,
            };
        });
        ui.end_row();
    });
    if rig != hud.rig {
        request.rig = Some(rig);
    }
    ui.horizontal(|ui| {
        // The whole point of the sliders: a setting that felt right is a value
        // that can be pasted into `follow.rs` and committed as the preset it
        // turned out to be.
        let literal = literal(&rig);
        ui.label(egui::RichText::new(&literal).monospace().small());
        if ui.small_button("copy").clicked() {
            ui.ctx().copy_text(literal);
        }
    });

    // The body's ease, under its own heading and with its own copy button,
    // because it is not part of the rig: a rig is the eye's parameter set and
    // this is a property of the body the eye is looking at (`docs/client/design_camera_rig.md`
    // D10). They are on one panel because they are looked at together — which
    // is a fact about the sitting, not about the types.
    ui.separator();
    ui.horizontal(|ui| {
        ui.label("body ease");
        let world_ease = world.presentation.crowd.ease();
        let mut ease = world_ease;
        ui.add(egui::Slider::new(&mut ease.tau, 0.0..=0.5).suffix(" s"));
        if ease != world_ease {
            request.ease = Some(ease);
        }
        let literal = format!("Ease {{ tau: {:?} }}", ease.tau);
        ui.label(egui::RichText::new(&literal).monospace().small());
        if ui.small_button("copy").clicked() {
            ui.ctx().copy_text(literal);
        }
    });

    ui.horizontal(|ui| {
        ui.label("scope");
        let mut span = hud.perf.scope_span.as_secs_f32();
        // Logarithmic: the useful settings are a second apart at one end and
        // several seconds apart at the other, and a linear slider spends most
        // of its length on the end nobody is looking at.
        if ui
            .add(
                egui::Slider::new(&mut span, 0.5..=20.0)
                    .logarithmic(true)
                    .suffix(" s"),
            )
            .changed()
        {
            request.scope_span = Some(Duration::from_secs_f32(span));
        }
    });

    ui.separator();
    match hud.perf.metrics {
        Some(metrics) => {
            egui::Grid::new("metrics").num_columns(4).show(ui, |ui| {
                ui.label("lag");
                ui.label(format!("{:.1} px", metrics.lag_max));
                ui.label("speed");
                ui.label(format!("{:.0} px/s", metrics.speed_max));
                ui.end_row();
                ui.label("accel");
                ui.label(format!("{:.0}", metrics.accel_max));
                ui.label("jerk rms");
                ui.label(format!("{:.0}", metrics.jerk_rms));
                ui.end_row();
                ui.label("step σ²");
                ui.label(format!("{:.2}", metrics.step_var));
                // The two companions, and they are on the panel rather than in
                // a comment: a metric over a scene where nothing moved is
                // green and means nothing, and this repository has produced
                // that result before.
                ui.label("travel");
                ui.label(format!("{:.0} px / {} frames", metrics.travel, metrics.frames));
                ui.end_row();
            });
        }
        None => {
            ui.label("no frames yet");
        }
    }

    let span = hud.perf.scope_span.as_secs_f32().max(0.001);
    let last = hud
        .perf
        .readings
        .last()
        .map_or(0.0, |reading| reading.at.as_secs_f32());
    let series = |of: fn(&Reading) -> Option<f64>| -> Vec<(f32, f32)> {
        hud.perf
            .readings
            .iter()
            .filter_map(|reading| {
                of(reading).map(|value| (reading.at.as_secs_f32() - (last - span), value as f32))
            })
            .collect()
    };
    strip(
        ui,
        "the eye's speed, px/s",
        &series(|reading| Some(reading.speed)),
        span,
        egui::Color32::from_rgb(80, 170, 255),
    );
    strip(
        ui,
        "jerk — what ragged is, as a number",
        &series(|reading| reading.jerk),
        span,
        egui::Color32::from_rgb(255, 140, 90),
    );

    ui.separator();
    match hud.replay {
        Some((name, progress)) => {
            ui.add(egui::ProgressBar::new(progress).text(name));
            if ui.button("stop").clicked() {
                request.script = Some(ScriptRequest::Stop);
            }
        }
        None => {
            // The viewer, and not merely "no link": a scenario walks the body
            // itself, which is exactly what a client that has *lost* its shard
            // must not do — see `world::Shard`.
            let offline = world.shard.is_viewer();
            ui.horizontal_wrapped(|ui| {
                for name in &hud.scripts {
                    if ui.add_enabled(offline, egui::Button::new(*name)).clicked() {
                        request.script = Some(ScriptRequest::Run(name));
                    }
                }
            });
            if !offline {
                ui.label(
                    egui::RichText::new(
                        "a scenario walks the body itself, so it needs a client with no shard",
                    )
                    .weak()
                    .small(),
                );
            }
        }
    }
}

/// The frame rate, what is setting it, and which half of the frame the time went
/// into.
///
/// A drop is either *cost* — the frame took too long to build — or *pacing*:
/// nothing asked for a frame sooner. Watched, this client is paced by the display
/// and a drop is a cost; unwatched it falls back to the animation clock on
/// purpose, and 12.5 frames a second there looks exactly like a stall and is not
/// one. So the pacer is printed beside the rate.
///
/// And the cost is two curves rather than one, because a frame is built by two
/// independent things: `egui` laying out the panels, and the world. The wait is
/// neither — it is the display holding the last frame — and it is the number
/// that says how much of the frame was still free. See [`crate::frames`].
fn frames_panel(ui: &mut egui::Ui, hud: &Hud) {
    let ms = |duration: Duration| duration.as_secs_f64() * 1_000.0;
    let mib = |bytes: u64| bytes as f64 / (1024.0 * 1024.0);
    let last = hud.perf.frames.last();
    egui::Grid::new("frames").num_columns(4).show(ui, |ui| {
        ui.label("fps");
        match last {
            // The last frame's own rate, not an average: the thing worth seeing
            // is the one frame that took 80ms, and a mean over a second is
            // exactly what hides it.
            Some(frame) => ui.label(format!("{:.0}", frame.fps())),
            None => ui.label("—"),
        };
        ui.label("worst");
        match hud.perf.worst_fps {
            Some(worst) => ui.label(format!("{worst:.0}")),
            None => ui.label("—"),
        };
        ui.end_row();
        ui.label("ui");
        ui.label(last.map_or("—".to_string(), |frame| format!("{:.1} ms", ms(frame.ui))));
        ui.label("world");
        ui.label(last.map_or("—".to_string(), |frame| format!("{:.1} ms", ms(frame.scene))));
        ui.end_row();
        ui.label("build");
        ui.label(last.map_or("—".to_string(), |frame| format!("{:.1} ms", ms(frame.build()))));
        // The acquire stall. **Not** simply the slack: it is also what a GPU
        // still drawing the last frame looks like from this thread, which is why
        // it is now printed beside `gpu` rather than on its own. See
        // [`crate::frames::Frame::wait`].
        ui.label("waited");
        ui.label(last.map_or("—".to_string(), |frame| format!("{:.1} ms", ms(frame.wait))));
        ui.end_row();
        // The device's own number, and the row that decides what `waited` above
        // meant. A dash here is an adapter with no timestamp queries — see the
        // line under the grid — and not a device that cost nothing.
        ui.label("gpu");
        ui.label(match last.and_then(|frame| frame.gpu) {
            Some(gpu) => format!("{:.1} ms", ms(gpu)),
            None => "—".to_string(),
        });
        ui.label("of interval");
        ui.label(match last.and_then(|frame| Some((frame.gpu?, frame.interval))) {
            Some((gpu, interval)) if !interval.is_zero() => {
                format!("{:.0}%", 100.0 * gpu.as_secs_f64() / interval.as_secs_f64())
            }
            _ => "—".to_string(),
        });
        ui.end_row();
    });
    // The sentence the whole GPU column exists to make sayable. A client asleep
    // on vsync and a client blocked on its own last frame hold identical `fps`,
    // `build` and `waited`, and differ only here — so the panel says which of
    // them this is rather than leaving the reader to do the arithmetic.
    match last.and_then(|frame| Some((frame.gpu?, frame.build(), frame.interval))) {
        Some((gpu, build, interval)) if gpu > interval.saturating_sub(build) => {
            ui.label(
                egui::RichText::new(
                    "the device is the bottleneck: the wait above is this client blocked on its own last frame, not slack",
                )
                .color(egui::Color32::YELLOW),
            );
        }
        Some(_) => {}
        // Absent, and said in words: a zero here would read as "the GPU cost
        // nothing", which is the one answer that is certainly wrong.
        None => {
            ui.label(
                egui::RichText::new(
                    "this adapter cannot write timestamp queries: the GPU's half of the frame is unmeasured",
                )
                .weak()
                .small(),
            );
        }
    }
    // Which pass, once the row above has said it is the device. Ordered as the
    // frame recorded them, so this reads down a frame in the order it was drawn
    // — and it is the last resolved frame rather than the one just built, a
    // couple behind whatever `gpu` above belongs to.
    if !hud.perf.gpu_passes.is_empty() {
        egui::CollapsingHeader::new("what the gpu drew")
            .default_open(false)
            .show(ui, |ui| {
                egui::Grid::new("gpu passes").num_columns(2).show(ui, |ui| {
                    for pass in &hud.perf.gpu_passes {
                        // Nested scopes are indented rather than added up: the
                        // total above counts the outermost only, and a reader
                        // has to be able to see which rows are inside which.
                        ui.label(format!("{}{}", "  ".repeat(pass.depth), pass.label));
                        ui.label(format!("{:.2} ms", ms(pass.cost)));
                        ui.end_row();
                    }
                });
            });
    }
    ui.separator();
    ui.label("map composites");
    egui::Grid::new("map composites").num_columns(4).show(ui, |ui| {
        ui.label("ready");
        ui.label(hud.composites.ready.to_string());
        ui.label("queue");
        ui.label(format!(
            "{} pending, {} prepared, {} in flight",
            hud.composites.pending, hud.composites.prepared, hud.composites.in_flight
        ));
        ui.end_row();
        ui.label("gpu cache");
        ui.label(format!("{:.1} MiB", mib(hud.composites.gpu_bytes)));
        ui.label("budget");
        ui.label(format!("{:.1} MiB", mib(hud.composites.gpu_budget_bytes)));
        ui.end_row();
        ui.label("quarantined");
        ui.label(hud.composites.quarantined.to_string());
        ui.label("latest");
        ui.label(match hud.composites.latest_quarantine {
            Some(quarantine) => {
                format!(
                    "{:?} block {:?}, key {:?}, owner {:?}",
                    quarantine.reason, quarantine.block, quarantine.key, quarantine.ground
                )
            }
            None => "none".to_owned(),
        });
        ui.end_row();
    });
    radar_report(ui, hud, mib);
    // The counter `docs/client/evidence/2026-08-14-the-camera-rig-record.md` asks for: without it, a full atlas repack
    // is indistinguishable from an ordinary heavy frame, both being a large
    // number in `world` above. `repacked` marks which frame in the window
    // paid for one; the total survives past that window.
    if last.is_some_and(|frame| frame.repacked) {
        ui.label(egui::RichText::new("this frame repacked the atlas").color(egui::Color32::YELLOW));
    }
    if hud.perf.repacks > 0 {
        ui.label(
            egui::RichText::new(format!("atlas repacks this session: {}", hud.perf.repacks))
                .weak()
                .small(),
        );
    }
    // The sentence that turns "the frame rate dropped" from a bug report into a
    // reading. What is asking for frames is the whole answer, and when it is the
    // animation clock that is a rule rather than a symptom — see `App::pacing`.
    ui.label(
        egui::RichText::new(match hud.perf.pacing {
            crate::frames::Pacing::Display => {
                "the display is the pacer: a frame is asked for as soon as the last is queued, and the surface presents in FIFO"
            }
            crate::frames::Pacing::Timer(_) => {
                "nobody is watching the window: the loop is on the animation clock and draws only what the animation needs"
            }
        })
        .weak()
        .small(),
    );

    let span = hud.perf.frames_span.as_secs_f32().max(0.001);
    let end = hud.perf.frames.last().map_or(0.0, |frame| frame.at.as_secs_f32());
    let series = |of: fn(&crate::frames::Frame) -> f64| -> Vec<(f32, f32)> {
        hud.perf
            .frames
            .iter()
            .map(|frame| (frame.at.as_secs_f32() - (end - span), of(frame) as f32))
            .collect()
    };
    strip(
        ui,
        "frames per second",
        &series(|frame| frame.fps()),
        span,
        egui::Color32::from_rgb(120, 220, 120),
    );
    // One chart and one scale for the two halves, deliberately: the question is
    // which of them is the bigger, and two charts each normalised to their own
    // peak would draw a tenth of a millisecond exactly as tall as ten.
    strips(
        ui,
        "what a frame cost, ms",
        &[
            Curve {
                name:   "ui",
                points: series(|frame| frame.ui.as_secs_f64() * 1_000.0),
                colour: egui::Color32::from_rgb(150, 180, 240),
            },
            Curve {
                name:   "world",
                points: series(|frame| frame.scene.as_secs_f64() * 1_000.0),
                colour: egui::Color32::from_rgb(220, 200, 90),
            },
            // On the same scale as the two above, which is the whole reason it
            // is in this chart and not one of its own: the question a low frame
            // rate asks is which of the three is the biggest, and the answer is
            // only readable when they share an axis. Flat at zero on an adapter
            // that cannot time itself — the grid above says so in words, and a
            // curve cannot.
            Curve {
                name:   "gpu",
                points: series(|frame| frame.gpu.map_or(0.0, |gpu| gpu.as_secs_f64() * 1_000.0)),
                colour: egui::Color32::from_rgb(230, 130, 200),
            },
        ],
        span,
    );
}

/// The radar's own rows: what each open view chose, how its demand was
/// answered, and how much room the three bounds have left.
///
/// Its own function rather than more rows inside [`frames_panel`] because the
/// three counter sets it reads are one subsystem's and are only comparable to
/// one another. `mib` is handed in so this and the composite grid above spell
/// a byte count the same way.
///
/// R7 of `docs/world/design_radar.md`. Every number here was already being written and
/// none of it was readable, which for `over_capacity_draws` in particular means
/// a truncated draw — chunks silently dropped from a region wider than the page
/// array — looked exactly like terrain that had not finished loading.
fn radar_report(ui: &mut egui::Ui, hud: &Hud, mib: impl Fn(u64) -> f64) {
    let radar = &hud.radar;
    ui.separator();
    ui.label("radar");
    // A closed window is absent from this list rather than shown at some
    // default level: a selector's remembered level means nothing while nothing
    // is asking it, and printing one would invite reading it as a live choice.
    if radar.frame.levels.is_empty() {
        ui.label(
            egui::RichText::new("no radar window is open: nothing below is being asked for")
                .weak()
                .small(),
        );
    }
    egui::Grid::new("radar").num_columns(4).show(ui, |ui| {
        for (subject, lod, tiles_per_pixel) in &radar.frame.levels {
            ui.label(match subject {
                crate::windows::WindowSubject::Minimap => "minimap",
                crate::windows::WindowSubject::WorldMap => "facet map",
                // Unreachable while `radar_views` builds only those two, and
                // written out anyway: a third radar window would otherwise
                // appear here as one of the other two.
                _ => "radar window",
            });
            ui.label(format!("level {}", lod.value()));
            ui.label("tiles/px");
            ui.label(format!("{tiles_per_pixel:.2}"));
            ui.end_row();
        }
        let demand = radar.frame.demand;
        ui.label("chunks asked for");
        ui.label(demand.total().to_string());
        ui.label("answered");
        ui.label(format!(
            "{} exact, {} coarser, {} stale, {} missing",
            demand.exact, demand.coarser, demand.stale, demand.missing
        ));
        ui.end_row();
        ui.label("raster");
        ui.label(format!("{:.2} ms", radar.frame.raster.as_secs_f64() * 1_000.0));
        ui.label("built");
        ui.label(format!("{} chunks", radar.frame.built));
        ui.end_row();
        ui.label("cpu cache");
        ui.label(format!(
            "{:.1} / {:.1} MiB",
            mib(radar.cache.retained_bytes),
            mib(radar.cache.tail_budget)
        ));
        ui.label("chunks");
        ui.label(format!(
            "{} ready, {} stale, {} rebuilt, {} evicted",
            radar.cache.ready, radar.cache.stale, radar.cache.rebuilt, radar.cache.evicted
        ));
        ui.end_row();
        ui.label("queue");
        ui.label(format!(
            "{} of {}",
            radar.queue.queued + radar.queue.in_flight,
            radar.queue.max_queued
        ));
        ui.label("split");
        ui.label(format!(
            "{} queued, {} in flight, {} requested this session",
            radar.queue.queued, radar.queue.in_flight, radar.cache.requested
        ));
        ui.end_row();
        ui.label("gpu pages");
        ui.label(format!("{} of {}", radar.pages.resident, radar.pages.capacity));
        ui.label("evicted");
        ui.label(radar.pages.evicted.to_string());
        ui.end_row();
    });
    // The one line in this block that is a defect rather than a reading. A
    // truncated draw is chunks the region named and the page array could not
    // hold, dropped by `cap_draws_by_distance` — and what it looks like on
    // screen is terrain that has not arrived, which is also what an ordinary
    // filling-in minimap looks like. Said in words, and only when it has
    // happened, because a zero here is the whole of the good news.
    if radar.pages.over_capacity_draws > 0 {
        ui.label(
            egui::RichText::new(format!(
                "{} draws exceeded the page array and were truncated by distance: chunks were dropped, not late",
                radar.pages.over_capacity_draws
            ))
            .color(egui::Color32::YELLOW),
        );
    }
    // The stale-fallback reading, which is the one a number alone gets wrong.
    // Coarser is the ladder working as designed and clears as the family
    // completes; stale is the *revision* ladder, and it means the picture on
    // screen is of terrain that has since changed.
    if radar.frame.demand.stale > 0 {
        ui.label(
            egui::RichText::new(format!(
                "{} chunks are drawn from a superseded revision: this terrain has changed since it was rastered",
                radar.frame.demand.stale
            ))
            .weak()
            .small(),
        );
    }
}

/// A rig as the source line it would be, for pasting into `follow.rs`.
///
/// The one output of this panel that outlives the session, which is why it is a
/// function with a test rather than a `format!` in the middle of a widget.
fn literal(rig: &Rig) -> String {
    let cut = match rig.lift_cut.is_finite() {
        true => format!("{:?}", rig.lift_cut),
        // `inf` is not Rust, and a preset pasted with it in would not compile —
        // which is a thing to find out here rather than in a build.
        false => "f32::INFINITY".to_string(),
    };
    format!(
        "Rig {{ plane_tau: {:?}, lift_tau: {:?}, lift_cut: {cut} }}",
        rig.plane_tau, rig.lift_tau,
    )
}

/// One strip chart: a curve of the last few seconds, scaled to its own peak.
///
/// Scaled to the peak of what is on screen and the peak printed on it, because
/// the axis is not the point — the *shape* is, and a reversal that is a square
/// corner on one rig and a rounded one on another is the whole reason this is
/// drawn rather than tabulated. A fixed axis would flatten every scenario that
/// is not a walk.
fn strip(ui: &mut egui::Ui, title: &str, series: &[(f32, f32)], span: f32, colour: egui::Color32) {
    strips(
        ui,
        title,
        &[Curve {
            // Unnamed, because a chart with one curve names it in the title.
            name: "",
            points: series.to_vec(),
            colour,
        }],
        span,
    );
}

/// One named curve of a strip chart: a point per frame, as (seconds into the
/// window, value).
struct Curve<'a> {
    /// What to call it in the legend, or empty for the one-curve chart.
    name:   &'a str,
    points: Vec<(f32, f32)>,
    colour: egui::Color32,
}

/// Several curves in one chart, on one scale.
///
/// One scale and not one each, which is the whole reason this exists: two costs
/// worth comparing are worth comparing, and a chart that normalised each curve
/// to its own peak would draw a tenth of a millisecond exactly as tall as ten
/// and answer the question backwards. Each curve is named in its own colour
/// beside the peak they share.
fn strips(ui: &mut egui::Ui, title: &str, series: &[Curve<'_>], span: f32) {
    let width = ui.available_width().max(180.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 56.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
    let peak = series
        .iter()
        .flat_map(|curve| curve.points.iter().map(|(_, value)| *value))
        .fold(0.0f32, f32::max);
    // A flat run has a peak of zero and would divide by it. Drawn along the
    // floor instead, which is what a still eye *is*.
    let scale = match peak > 0.0 {
        true => rect.height() / peak,
        false => 0.0,
    };
    for curve in series {
        let points: Vec<egui::Pos2> = curve
            .points
            .iter()
            .map(|(at, value)| {
                egui::pos2(
                    rect.left() + rect.width() * (at / span).clamp(0.0, 1.0),
                    rect.bottom() - value * scale,
                )
            })
            .collect();
        if points.len() > 1 {
            painter.add(egui::Shape::line(points, egui::Stroke::new(1.0, curve.colour)));
        }
    }
    let mut at = rect.left_top() + egui::vec2(4.0, 2.0);
    let font = egui::FontId::proportional(10.0);
    let colour = series
        .first()
        .map_or(ui.visuals().text_color(), |curve| curve.colour);
    let head = painter.text(
        at,
        egui::Align2::LEFT_TOP,
        format!("{title} — peak {peak:.0}"),
        font.clone(),
        colour,
    );
    at.x = head.right() + 8.0;
    // The legend, and only where there is one: a single curve is named by the
    // title it was drawn under.
    for curve in series.iter().filter(|curve| !curve.name.is_empty()) {
        let drawn = painter.text(at, egui::Align2::LEFT_TOP, curve.name, font.clone(), curve.colour);
        at.x = drawn.right() + 8.0;
    }
}

/// Which single row of a tile's column [`tile_rows`] draws should flag as the
/// thing a click actually landed on — [`selected_marked`] is the one caller
/// that ever asks for this; `tile_panel`'s other caller (`hover`) always
/// passes `None`, since a live hover has no held identity to mark.
#[derive(Clone, Copy)]
enum Marked {
    Static { graphic: Graphic, height: Height },
    Item { graphic: Graphic, height: Height },
}

impl Marked {
    /// `"→ "` on the row this names, two spaces of the same width otherwise —
    /// so every row still lines up in the monospace column regardless of
    /// which one, if any, is marked.
    fn arrow(marked: Option<Marked>, graphic: Graphic, height: Height) -> &'static str {
        let hit = match marked {
            Some(Marked::Static {
                graphic: g,
                height: h,
            })
            | Some(Marked::Item {
                graphic: g,
                height: h,
            }) => (g, h) == (graphic, height),
            None => false,
        };
        match hit {
            true => "→ ",
            false => "  ",
        }
    }
}

/// One tile's numbers, monospace and selectable by dragging across them —
/// egui merges a drag across several `Label`s in one `Ui` into a single text
/// selection, so the whole box copies with one drag and a `Ctrl+C` the same
/// way a terminal's scrollback does.
///
/// A fixed-height box, scrolled inside, and that is the point of it: a tile's
/// readout is as many rows as it has statics, so a panel sized to its content
/// changes height under the cursor and moves everything below it — including
/// the other tile panel — while it is being read. The height is spent whether
/// or not there is a tile to put in it; `id` is what keeps the two boxes'
/// scroll offsets apart, the same way the tabs' own salt does.
fn tile_panel(ui: &mut egui::Ui, id: &str, tile: Option<&PickedTile>, marked: Option<Marked>) {
    /// Four rows and a little: the header, the levels, the land, and one static
    /// — past that the box scrolls rather than grows.
    const HEIGHT: f32 = 108.0;
    egui::ScrollArea::vertical()
        .id_salt(id)
        .max_height(HEIGHT)
        .auto_shrink([false; 2])
        // Hidden rather than shown-when-needed: a bar down the right edge is a
        // second draggable thing inside a box whose whole point is now that a
        // drag anywhere in it selects text — the two fight over the same
        // gesture, and the bar was winning it at the edge a drag most often
        // starts from. The wheel still scrolls; only the widget is gone.
        .scroll_bar_visibility(egui::containers::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .show(ui, |ui| tile_rows(ui, tile, marked));
}

/// The rows themselves, inside the box [`tile_panel`] fixes the height of.
fn tile_rows(ui: &mut egui::Ui, tile: Option<&PickedTile>, marked: Option<Marked>) {
    let Some(tile) = tile else {
        ui.monospace("(none)");
        return;
    };
    // Both heights, because the gap between them is the thing worth seeing:
    // on a pier the land is water far below the deck a body stands on, and
    // every marker on this tile is drawn at the second one.
    ui.monospace(format!(
        "tile {}, {}   stand z {}   depth {}",
        tile.at.x, tile.at.y, tile.stand_z.0, tile.tile_depth.0
    ));
    // The mobile's own sort key, beside the statics below it — the two are
    // meant to be read against each other: a static whose `priority_z` here is
    // `>=` the mobile's, on this same `depth`, is what wins the draw order tie
    // and covers it.
    if let Some(order) = tile.mobile_order {
        let (mobile_tile, mobile_priority_z) = (order.tile, order.priority_z);
        ui.monospace(format!(
            "mobile order: tile {mobile_tile}  priority_z {mobile_priority_z}"
        ));
    }
    // The column in words, in the same green and red the box is drawn in: the
    // picture says *where* the levels are and this says which, so a level hidden
    // behind a wall on screen is still countable here.
    ui.horizontal_wrapped(|ui| {
        ui.monospace("levels");
        for &(Height(z), standable) in &tile.levels {
            let colour = match standable {
                true => STANDABLE,
                false => BLOCKED,
            };
            ui.label(egui::RichText::new(format!("{z}")).monospace().color(colour));
        }
        if let Some(Height(ceiling)) = tile.ceiling {
            ui.monospace(format!("· ceiling {ceiling}"));
        }
    });
    match tile.land {
        // `.0` here and below because this is the presentation seam: a panel
        // printing an id in decimal *and* hex is exactly the place a newtype is
        // supposed to be unwrapped, the same licence the wire and SQL get.
        Some(Graphic(id)) => ui.monospace(format!("land {id} (0x{id:04X})  z {}", tile.land_z.0)),
        None => ui.monospace("land: block not loaded"),
    };
    for &(graphic @ Graphic(id), height @ Height(z), Hue(hue), PriorityZ(priority_z)) in &tile.statics {
        let arrow = Marked::arrow(marked, graphic, height);
        ui.monospace(format!(
            "{arrow}static {id} (0x{id:04X})  z {z}  hue {hue}  priority_z {priority_z}"
        ));
    }
    // The shard's own decoration — a different source from `statics` above,
    // and the one a static-only panel missed: see `PickedTile::items`.
    for &(graphic @ Graphic(id), height @ Height(z), Hue(hue), PriorityZ(priority_z)) in &tile.items {
        let arrow = Marked::arrow(marked, graphic, height);
        ui.monospace(format!(
            "{arrow}item {id} (0x{id:04X})  z {z}  hue {hue}  priority_z {priority_z}"
        ));
    }
}

/// What "copy all" puts on the clipboard: the same rows the panel draws, as
/// text.
///
/// Written out rather than screenshotted because the numbers are the evidence —
/// a report that says "the wall is at the wrong height" is an opinion until the
/// column it was read off is pasted under it. Hex beside decimal for the
/// graphics, since the client's own files and every reference emulator disagree
/// about which of the two they print.
fn tile_text(tile: &PickedTile) -> String {
    use std::fmt::Write;

    let mut text = format!(
        "tile {}, {}  stand z {}  land z {}  depth {}\n",
        tile.at.x, tile.at.y, tile.stand_z.0, tile.land_z.0, tile.tile_depth.0
    );
    if let Some(order) = tile.mobile_order {
        let (mobile_tile, mobile_priority_z) = (order.tile, order.priority_z);
        let _ = writeln!(
            text,
            "mobile order: tile {mobile_tile}  priority_z {mobile_priority_z}"
        );
    }
    text.push_str("levels");
    // The panel says "a body does not fit here" in red, and red does not
    // survive a paste; `!` after the height is the same fact in text.
    for &(Height(z), standable) in &tile.levels {
        let verdict = match standable {
            true => "",
            false => "!",
        };
        // `unwrap` is not needed and `?` cannot happen: writing into a `String`
        // is infallible, which is why the result is dropped here.
        let _ = write!(text, " {z}{verdict}");
    }
    if let Some(Height(ceiling)) = tile.ceiling {
        let _ = write!(text, " · ceiling {ceiling}");
    }
    text.push('\n');
    match tile.land {
        Some(Graphic(id)) => {
            let _ = writeln!(text, "land {id} (0x{id:04X})  z {}", tile.land_z.0);
        }
        None => text.push_str("land: block not loaded\n"),
    }
    for &(Graphic(id), Height(z), Hue(hue), PriorityZ(priority_z)) in &tile.statics {
        let _ = writeln!(
            text,
            "static {id} (0x{id:04X})  z {z}  hue {hue}  priority_z {priority_z}"
        );
    }
    for &(Graphic(id), Height(z), Hue(hue), PriorityZ(priority_z)) in &tile.items {
        let _ = writeln!(
            text,
            "item {id} (0x{id:04X})  z {z}  hue {hue}  priority_z {priority_z}"
        );
    }
    text
}

/// The painter every world marker is drawn with: behind the UI, and inside the
/// world.
///
/// Two properties, and each of them is a bug that was there before:
///
/// * **Order.** [`egui::Order::Background`] puts these under the windows, which
///   is where a thing lying on the ground belongs — in
///   [`Foreground`](egui::Order::Foreground) a tile highlight was drawn *over*
///   the panel the cursor was hovering, so the world leaked onto the UI.
/// * **Clip.** Layers inside one order are painted in the order they are
///   created, and the panels' own background layer exists before this one — so
///   the order alone does not keep a marker off a docked panel. The clip rect
///   does, and it is the same rectangle the world itself is drawn into, so
///   nothing can be painted where the world is not.
fn world_painter(context: &egui::Context, viewport: egui::Rect) -> egui::Painter {
    context
        .layer_painter(egui::LayerId::new(
            egui::Order::Background,
            egui::Id::new("world-overlay"),
        ))
        .with_clip_rect(viewport)
}

/// Where a tile's diamond falls in the root `Ui`'s own space.
///
/// [`Camera::tile_diamond`] gives the corners in *viewport* pixels, physical and
/// post-blit, so they are scaled by `1 / pixels_per_point` and offset by where
/// the viewport starts — the same points-against-pixels conversion `Shell::run`
/// does for the rect, the other way round.
fn tile_corners(
    painter: &egui::Painter,
    camera: &Camera,
    point: openshard_protocol::world::Point,
    viewport_origin: egui::Pos2,
) -> Vec<egui::Pos2> {
    facet_corners(painter, camera, point, [point.z; 4], viewport_origin)
}

/// The same, for a surface that is not level: the corners stand at their own
/// heights. See [`Camera::tile_facet`] for the order they come in.
fn facet_corners(
    painter: &egui::Painter,
    camera: &Camera,
    point: openshard_protocol::world::Point,
    corners: [i8; 4],
    viewport_origin: egui::Pos2,
) -> Vec<egui::Pos2> {
    let scale = 1.0 / painter.ctx().pixels_per_point();
    camera
        .tile_facet(point, corners)
        .map(|corner| viewport_origin + egui::vec2(corner.x * scale, corner.y * scale))
        .to_vec()
}

/// Draw selected static art where the renderer would anchor it after a click.
fn draw_editor_static_preview(
    painter: &egui::Painter,
    camera: &Camera,
    at: openshard_protocol::world::Point,
    texture: &egui::TextureHandle,
    viewport_origin: egui::Pos2,
    alpha: u8,
) {
    let [width, height] = texture.size();
    let centre = camera.to_screen(at);
    let left = centre.x - (i32::try_from(width).unwrap_or(i32::MAX) >> 1);
    let top = centre.y + openshard_client_render::camera::TILE_HEIGHT / 2
        - i32::try_from(height).unwrap_or(i32::MAX);
    let top_left = openshard_client_render::camera::ViewPoint::new(left as f32, top as f32);
    let bottom_right = openshard_client_render::camera::ViewPoint::new(
        left as f32 + width as f32,
        top as f32 + height as f32,
    );
    let scale = 1.0 / painter.ctx().pixels_per_point();
    let place = |point| {
        let point = camera.to_viewport_exact(point);
        viewport_origin + egui::vec2(point.x * scale, point.y * scale)
    };
    painter.image(
        texture.id(),
        egui::Rect::from_two_pos(place(top_left), place(bottom_right)),
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        egui::Color32::from_white_alpha(alpha),
    );
}

/// Where a tile's centre falls there — the route's own polyline runs through
/// these.
fn tile_centre(
    painter: &egui::Painter,
    camera: &Camera,
    point: openshard_protocol::world::Point,
    viewport_origin: egui::Pos2,
) -> egui::Pos2 {
    let scale = 1.0 / painter.ctx().pixels_per_point();
    let centre = camera.to_viewport(camera.to_screen(point));
    viewport_origin + egui::vec2(centre.x * scale, centre.y * scale)
}

/// A walkable level, drawn green.
const STANDABLE: egui::Color32 = egui::Color32::from_rgb(60, 255, 90);
/// The high end of a walkable route's height gradient.
const ROUTE_HIGH_Z: egui::Color32 = egui::Color32::from_rgb(60, 135, 255);
/// A level a body does not fit on, drawn red — the same pair the terrain
/// overlay washes tiles with, so one vocabulary answers "can I stand there"
/// wherever the question is asked.
const BLOCKED: egui::Color32 = egui::Color32::from_rgb(255, 40, 40);
/// Ground the sight ray crosses but the weapon does not reach, drawn amber.
///
/// A third colour rather than the red above, because it is a different refusal:
/// red is the world in the way and this is the arm too short, and a person
/// reading the overlay has to be able to tell "move" from "get closer". See
/// `docs/combat/design_sight.md`.
const OUT_OF_REACH: egui::Color32 = egui::Color32::from_rgb(255, 190, 60);

/// The same colour at a chosen alpha — a wash and an outline of one hue.
fn washed(colour: egui::Color32, alpha: u8) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(colour.r(), colour.g(), colour.b(), alpha)
}

/// A stable, high-contrast colour for a stitched-room ordinal.
///
/// Unlike a short palette this keeps neighbouring rooms distinguishable even
/// in a large inn or dungeon level.  The ordinal is diagnostic-only, so it is
/// deliberately turned into a colour here rather than becoming presentation
/// data in the map index.
fn room_colour(room: u32) -> egui::Color32 {
    let [red, green, blue, _] = room
        .wrapping_mul(0x9E37_79B1)
        .wrapping_add(0x7F4A_7C15)
        .to_le_bytes();
    egui::Color32::from_rgb(88 + red / 2, 88 + green / 2, 88 + blue / 2)
}

/// The glow over one tile, drawn as the space the tile actually occupies: the
/// surface a body would stand on, the column from the world's datum up to
/// whatever roofs the tile over, and every height in between a body could stand
/// at — green where one fits, red where it does not.
///
/// A diamond alone is ambiguous in an isometric picture — the same screen
/// position is every height on that column, so a marker on a pier's deck and a
/// marker in the water beside it are drawn a few pixels apart and read as the
/// same place. The column removes the ambiguity by drawing the height itself:
/// the base sits at `z = 0` and how far the surface floats above it *is* the
/// tile's height, legible without reading the panel.
///
/// `z = 0` and not the land's height, because the base has to be the same datum
/// under every tile — a base that moved with the ground would make the column a
/// difference between two unknowns rather than a height.
///
/// **Up to the ceiling and not to the surface.** A tile is a column of space and
/// a floor is a thing *inside* it: stopping the box at the planks says nothing
/// about the room over them, and indoors the useful fact is exactly that — how
/// much headroom there is, where the storey above starts, whether the thing
/// under the cursor is a floor or a lid. The lid is [`PickedTile::ceiling`], the
/// top of the tallest static on the tile, and where nothing stands the box tops
/// out at the surface as before.
///
/// **The levels are the answer to "why can I not stand here".** A stair tile
/// carries the floor below and the tread above; a doorway carries the threshold
/// and the lintel over it. Each is a diamond at its own height, coloured by
/// whether a body fits — which is asked of the cluttered terrain the walk asks,
/// so a red level is a level the step will refuse.
///
/// The two south-facing sides are filled and the two behind them are not: they
/// are the faces the eye can see, and filling all four would make the box a
/// solid blob with no depth to it.
fn draw_tile_highlight(
    painter: &egui::Painter,
    camera: &Camera,
    tile: &PickedTile,
    viewport_origin: egui::Pos2,
    fill: egui::Color32,
    stroke: egui::Stroke,
) {
    // The surface, not the land: on a pier the two are thirteen z-units apart
    // and the land's height puts the diamond in the water beside the boards.
    // `.0` here is the presentation seam: `Point` stays a bare coordinate by
    // project convention (`docs/protocol/design_wire_types.md`), so this is where
    // `Height` is unwrapped to meet it.
    let at = |z: Height, corners: [Height; 4]| {
        facet_corners(
            painter,
            camera,
            openshard_protocol::world::Point {
                x: tile.at.x,
                y: tile.at.y,
                z: z.0,
            },
            corners.map(|Height(z)| z),
            viewport_origin,
        )
    };
    // The surface's own shape — sloped on a hillside, flat on a deck — while the
    // base is the datum plane and therefore always level, and so is a roof.
    let surface = at(tile.stand_z, tile.corners);
    // The lid, when something stands *over* the surface rather than under it: a
    // ceiling at or below the tile's own floor is the floor's own static seen
    // from the other side, and a box drawn to it would be inside out.
    let lid = tile.ceiling.filter(|&z| z > tile.stand_z);
    let top = match lid {
        Some(z) => at(z, [z; 4]),
        None => surface.clone(),
    };
    let base = at(Height(0), [Height(0); 4]);
    // A tile whose whole column lies in the datum plane has no box, and the loop
    // below would draw four zero-length segments over the diamond's own edges.
    if lid.is_some() || tile.stand_z != Height(0) || tile.corners != [Height(0); 4] {
        // The sides are quieter than the surface: the column is context for the
        // marker, not a second marker. A fill at full strength doubled up over
        // the ground wash and read as the brighter of the two shapes.
        let side = egui::Color32::from_rgba_unmultiplied(
            fill.r(),
            fill.g(),
            fill.b(),
            fill.a().saturating_sub(fill.a() / 3),
        );
        let edge = egui::Stroke::new(stroke.width * 0.6, stroke.color);
        // Corners run north, east, south, west (`Camera::tile_diamond`), so the
        // faces the eye sees are east-south and south-west — the two whose
        // screen positions are lower than the tile's centre.
        for (a, b) in [(1, 2), (2, 3)] {
            painter.add(egui::Shape::convex_polygon(
                vec![top[a], top[b], base[b], base[a]],
                side,
                egui::Stroke::NONE,
            ));
        }
        painter.add(egui::Shape::closed_line(base.clone(), edge));
        for (top, base) in top.iter().zip(base) {
            painter.line_segment([*top, base], edge);
        }
        // The lid gets its own outline. Without it a roofed tile is a box with
        // nothing across the top of it, which reads as an unfinished column
        // rather than as a ceiling.
        if lid.is_some() {
            painter.add(egui::Shape::closed_line(top, edge));
        }
    }
    // Every standable height, under the marker so the marker stays the answer to
    // "where would a click go" and these stay the answer to "what else is here".
    // The surface's own level is drawn too and lands under the marker exactly —
    // its colour showing at the edges is the useful case, because a *red* rim on
    // the tile the cursor resolved to is the client saying the step it is
    // offering will be refused.
    for &(z, standable) in &tile.levels {
        let colour = match standable {
            true => STANDABLE,
            false => BLOCKED,
        };
        // The surface's shape for the surface's own height, flat for the rest: a
        // level is only sloped when it is the land, and the land is what
        // `corners` describes.
        let corners = match z == tile.stand_z {
            true => tile.corners,
            false => [z; 4],
        };
        painter.add(egui::Shape::closed_line(
            at(z, corners),
            egui::Stroke::new(stroke.width * 0.8, colour),
        ));
    }
    painter.add(egui::Shape::convex_polygon(surface, fill, stroke));
}

/// The walkability wash.
///
/// Fills without strokes for the tiles: one outlined diamond per visible tile is
/// a thousand strokes a frame and a picture nobody can read through, while a
/// translucent wash leaves the art underneath legible — which is the point, since
/// what is being looked for is a red tile somewhere the art says there is a way
/// through.
fn draw_terrain(
    painter: &egui::Painter,
    camera: &Camera,
    terrain: &TerrainOverlay,
    viewport_origin: egui::Pos2,
) {
    // Faint on purpose. The overlay is read *against* the art — a red diamond
    // matters because the ground under it looks like a way through — so a wash
    // heavy enough to hide what it is covering answers the wrong question. These
    // are the weakest values the two are still tellable apart at.
    let open = washed(STANDABLE, 14);
    let blocked = washed(BLOCKED, 30);
    let clip = painter.clip_rect();
    for (tiles, fill) in [(&terrain.open, open), (&terrain.blocked, blocked)] {
        for &point in tiles {
            let corners = tile_corners(painter, camera, point, viewport_origin);
            let bounds = corners.iter().fold(egui::Rect::NOTHING, |rect, point| {
                rect.union(egui::Rect::from_pos(*point))
            });
            // `visible_tiles` is deliberately an over-cover: it has no terrain
            // height yet, so it includes a wide safety margin. Egui would clip
            // these diamonds eventually, but avoiding the shape entirely saves
            // an allocation and tessellation for every off-screen tile.
            if !clip.intersects(bounds) {
                continue;
            }
            painter.add(egui::Shape::convex_polygon(corners, fill, egui::Stroke::NONE));
        }
    }
}

/// R1's map index, projected over the same ground the ordinary renderer is
/// drawing. Every indexed room is a contiguous coloured tile area; a room the
/// reachability walk did not show is deliberately black.  Door portals are
/// circles and height-changing walk edges (stairs, ramps or drops) are cyan
/// arrows.  This is an inspection picture only — see `InteriorOverlay`.
fn draw_interiors(
    painter: &egui::Painter,
    camera: &Camera,
    interiors: &crate::diagnostics::InteriorOverlay,
    viewport_origin: egui::Pos2,
) {
    let clip = painter.clip_rect();
    for cell in &interiors.cells {
        let corners = tile_corners(painter, camera, cell.at, viewport_origin);
        let bounds = corners.iter().fold(egui::Rect::NOTHING, |rect, point| {
            rect.union(egui::Rect::from_pos(*point))
        });
        if !clip.intersects(bounds) {
            continue;
        }
        let colour = room_colour(cell.room);
        let (fill, outline) = match cell.shown {
            // The room colour identifies the connected area.  Varying its
            // strength very slightly by floor preserves the distinction where
            // two floors project over the same tile without turning the view
            // into a per-tile rainbow.
            true => {
                (
                    washed(colour, 72 + (cell.floor % 3) as u8 * 12),
                    washed(colour, 180),
                )
            }
            false => {
                (
                    egui::Color32::from_rgba_unmultiplied(0, 0, 0, 220),
                    egui::Color32::from_rgba_unmultiplied(80, 80, 80, 230),
                )
            }
        };
        painter.add(egui::Shape::convex_polygon(
            corners,
            fill,
            egui::Stroke::new(0.8, outline),
        ));
    }
    for stair in &interiors.stairs {
        let from = tile_centre(painter, camera, stair.from, viewport_origin);
        let to = tile_centre(painter, camera, stair.to, viewport_origin);
        painter.line_segment(
            [from, to],
            egui::Stroke::new(2.5, egui::Color32::from_rgb(55, 245, 245)),
        );
        painter.circle_filled(to, 3.5, egui::Color32::from_rgb(55, 245, 245));
    }
    for door in &interiors.doors {
        let centre = tile_centre(painter, camera, door.at, viewport_origin);
        let colour = if door.shown {
            egui::Color32::from_rgb(95, 255, 125)
        } else {
            egui::Color32::from_rgb(255, 80, 80)
        };
        painter.circle_filled(centre, 5.0, washed(colour, 225));
        painter.circle_stroke(centre, 5.0, egui::Stroke::new(1.2, egui::Color32::WHITE));
    }
}

/// The way the body is going: a line through the tile centres, with a dot on
/// each step so a diagonal can be told from a pair of orthogonals.
///
/// **Green at the route's lowest level, blue at its highest, red only past an
/// obstacle.** The open half's gradient is scaled from its own minimum and
/// maximum `z`, so a stair or a change between storeys is readable even when its
/// absolute elevation differs from the next route. The two halves are
/// [`Route`]'s and the split is the walk's own — where the red starts is where
/// the body will stop, standing at whatever is in the way. Keeping red for that
/// only preserves the walkability wash's vocabulary ([`BLOCKED`]) instead of
/// making a higher floor look impassable.
///
/// Full strength, unlike the wash. A route is what the player just asked for and
/// is looking at; a tile wash is a diagnostic read against the art beneath it.
fn draw_route(painter: &egui::Painter, camera: &Camera, route: &Route, viewport_origin: egui::Pos2) {
    let centres = |tiles: &[openshard_protocol::world::Point]| -> Vec<egui::Pos2> {
        tiles
            .iter()
            .map(|&point| tile_centre(painter, camera, point, viewport_origin))
            .collect()
    };
    let open = centres(&route.open);
    let barred = centres(&route.barred);
    let (min_z, max_z) = route_height_range(&route.open);
    // A route that does not reach what was clicked is **dashed**, and the reason
    // is the picture's whole job here: a walk toward an unreachable place is the
    // same list of steps as a walk to a reachable one, so a solid line ending at
    // a wall is a client claiming it planned a way through it. A shut door is
    // not one of these — that route *does* have a far side, and it is the red
    // half below.
    let unreachable = route
        .refusal
        .is_some_and(|refusal| refusal != crate::steer::Refusal::Barred);
    // `egui::Shape::line` takes one colour, while a route's useful fact is the
    // height of every step. Draw its segments separately and colour each by the
    // tile it enters, matching the dot that marks that same step.
    for (segment, point) in open.windows(2).zip(route.open.iter().skip(1)) {
        let stroke = egui::Stroke::new(2.0, route_height_colour(point.z, min_z, max_z));
        if unreachable {
            painter.add(egui::Shape::dashed_line(
                &[segment[0], segment[1]],
                stroke,
                5.0,
                4.0,
            ));
        } else {
            painter.line_segment([segment[0], segment[1]], stroke);
        }
    }
    // And where it gives up: a cross on the last tile the body can reach, in the
    // colour the wash uses for ground nobody can stand on. Without it a dashed
    // line that happens to end on open ground says only "somewhere about here".
    if unreachable {
        if let Some(&end) = open.last() {
            let arm = 4.0;
            let stroke = egui::Stroke::new(2.0, BLOCKED);
            painter.line_segment([end + egui::vec2(-arm, -arm), end + egui::vec2(arm, arm)], stroke);
            painter.line_segment([end + egui::vec2(-arm, arm), end + egui::vec2(arm, -arm)], stroke);
        }
    }
    if barred.len() > 1 {
        painter.add(egui::Shape::line(barred.clone(), egui::Stroke::new(2.0, BLOCKED)));
    }
    // A dot per tile *stepped onto*, which is why both halves drop their first
    // point: the open half begins on the tile the body is already standing on,
    // and the barred half begins on the last tile it can reach — the one it will
    // stand on and wait. A dot on either would read as a step still to take, and
    // the red one would paint the tile the body *can* stand on in the colour of
    // the ones it cannot.
    for (centre, point) in open.iter().skip(1).zip(route.open.iter().skip(1)) {
        painter.circle_filled(*centre, 2.5, route_height_colour(point.z, min_z, max_z));
    }
    for centre in barred.iter().skip(1) {
        painter.circle_filled(*centre, 2.5, BLOCKED);
    }
}

/// The lowest and highest floors a route can actually walk.
///
/// `open` is always seeded with the route origin, but keeping this total makes
/// the painter safe for a diagnostic route assembled by a future caller.
fn route_height_range(route: &[openshard_protocol::world::Point]) -> (i8, i8) {
    route.iter().fold((i8::MAX, i8::MIN), |(min_z, max_z), point| {
        (min_z.min(point.z), max_z.max(point.z))
    })
}

/// A route floor's colour, from green at its minimum `z` to blue at its maximum.
///
/// A level route deliberately remains green: without a height range there is no
/// floor distinction to encode, and green continues to say that its steps are
/// walkable. The blue endpoint stays far from [`BLOCKED`]'s red, leaving that
/// colour unambiguous for route points the body cannot reach.
fn route_height_colour(z: i8, min_z: i8, max_z: i8) -> egui::Color32 {
    if min_z >= max_z {
        return STANDABLE;
    }
    let position = i16::from(z) - i16::from(min_z);
    let range = i16::from(max_z) - i16::from(min_z);
    let mix = |low: u8, high: u8| {
        let low = i16::from(low);
        let high = i16::from(high);
        (low + (high - low) * position / range) as u8
    };
    egui::Color32::from_rgb(
        mix(STANDABLE.r(), ROUTE_HIGH_Z.r()),
        mix(STANDABLE.g(), ROUTE_HIGH_Z.g()),
        mix(STANDABLE.b(), ROUTE_HIGH_Z.b()),
    )
}

/// Every surface in the grid, with the tile it stands on.
///
/// **A surface and not a tile**, which is the whole of what this view is for
/// since `docs/archive/render/lighting.md`'s step 21.2. `Occlusion::at` — what `boxes()` hands
/// out and what this drew until now — is the *merged* view: the union of the
/// spans, the largest opacity, and the union of the sides. Drawn, that is the
/// picture of a world that no longer exists. A floor and the wall on its tile
/// came out as one box from the floor's `z` to the wall's top, two walls with a
/// storey of air between them came out as one box through the air, and which
/// edge a panel stands on — the whole of decision 3 — was not in the picture at
/// all. Every gap this view is opened to look for is a gap between two of those
/// things, and the merge is exactly what closes them on screen.
/// The occlusion grid, drawn as the **solid** it is.
///
/// `docs/archive/render/lighting.md`, step 14, and it is an instrument rather than a picture:
/// what a shadow ray walks through is a list of surfaces — a plane on one edge, a
/// lid, a whole-tile body — and until this nothing drew them, so "why is there a
/// shadow where nothing stands" could only be answered by reading the map by
/// hand.
///
/// **Filled faces, in depth order, shaded by which way each looks.** The first
/// version of this was twelve strokes a box and no fill, on the argument that a
/// filled box hides the art it is a claim about — and what it produced was a
/// thicket: a wireframe carries no occlusion, so every box in a street was drawn
/// through every box in front of it and the eye could not tell which edge
/// belonged to which surface. A diagnostic about *geometry* has to read as
/// geometry. So the faces are filled, sorted back to front and lit by a fixed
/// direction, and what is given up — seeing the art under it — is what turning
/// the checkbox off is for.
///
/// The depth order is the projection's own: a tile further from the eye has a
/// smaller `x + y`, and within one tile a lower surface is drawn first. That is
/// the painter's algorithm on a grid where it happens to be exact, because
/// nothing in this world is bigger than the tile it stands on.
///
/// The **shade** is what makes an edge legible where two faces meet at one line:
/// a lid looks up and is the brightest, an east face is next, a south face is
/// darker, and the two a camera cannot see are darker still — a fixed light from
/// above and to the east, which is what every isometric picture does and what the
/// eye reads without being told.
///
/// The **hue is the kind**: a lid amber, a panel red, a whole-tile body violet,
/// and a pane cyan whatever shape it is. It was the opacity first, and that is
/// the mistake worth keeping written down — opacity is nearly a constant in a
/// real town (5,459 `OPAQUE` against 74 panes over the block this was built on),
/// so the picture came out one flat red and the geometry, which is the entire
/// question, had no colour left to be told in. A pane is rare enough to be the
/// exception rather than the axis.
///
/// What `cut` leaves and not every surface — see [`Cut`], and the count beside
/// the checkbox for what that leaves out.
fn draw_occluders(
    painter: &egui::Painter,
    camera: &Camera,
    occluders: &[crate::diagnostics::OccluderSurface],
    cut: Cut,
    viewport_origin: egui::Pos2,
) {
    use openshard_client_render::occlusion::Edges;
    use openshard_client_render::solid::Side;

    let clip = painter.clip_rect();
    // Already back-to-front from `App::occluders_shown`. The grid only changes
    // when that cache is refreshed, so sorting again every redraw is work that
    // cannot change this picture.
    for surface in occluders.iter().filter(|surface| cut.shows(&surface.solid)) {
        let (x, y, solid) = (surface.x, surface.y, &surface.solid);
        // The grid is grown past the map's own corner by the widest pool's
        // reach — see `light::lit_tiles` — so a tile of it can be off the map
        // entirely. Skipped rather than clamped, for `Occlusion::add`'s reason:
        // folding it onto the edge draws a wall where the map has none.
        let (Ok(x), Ok(y)) = (u16::try_from(x), u16::try_from(y)) else {
            continue;
        };
        // The same clamp `Occlusion::bytes` makes on the way to the shader: a
        // static's top is `z + height` and does not have to fit an `i8`, and a
        // wall reaching past the top of the world may as well stop there. The
        // surface is then drawn where the shader thinks it is, which is the point.
        let height = |z: i32| z.clamp(i32::from(i8::MIN), i32::from(i8::MAX)) as i8;
        let at = |z: i8| {
            tile_corners(
                painter,
                camera,
                openshard_protocol::world::Point { x, y, z },
                viewport_origin,
            )
        };
        let low = at(height(solid.bottom()));
        let high = at(height(solid.top()));
        // Off screen before anything is allocated. The clip rect would keep it
        // from being painted anyway, but a town at the widest zoom is thousands
        // of solids and most of them are outside the viewport — this is the
        // difference between building shapes for each of them and none.
        let bounds = low.iter().chain(&high).fold(egui::Rect::NOTHING, |rect, point| {
            rect.union(egui::Rect::from_pos(*point))
        });
        if !clip.intersects(bounds) {
            continue;
        }
        let [red, green, blue] = openshard_client_render::solid::kind_colour(solid);
        // A face's own shade, and a **near-black edge** under it. Two faces of
        // one box meet at a line and the two tones alone leave finding it to the
        // eye; the stroke is also what makes a tile of floor a tile rather than
        // part of a field of floor. The fill is let down to two thirds so that
        // the art keeps saying where the picture's own edges are — a solid
        // overlay is a second world, and the question is always about this one.
        let face = |points: Vec<egui::Pos2>, shade: f32| {
            let tone = |c: f32| (c * shade) as u8;
            let fill = egui::Color32::from_rgba_unmultiplied(tone(red), tone(green), tone(blue), 170);
            let edge = egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(10, 8, 16, 220));
            painter.add(egui::Shape::convex_polygon(points, fill, edge));
        };
        // The quad standing on one edge of the tile, from the solid's bottom to
        // its top: `[a, b]` are the two corners of that edge.
        let wall_of = |ends: [usize; 2]| vec![low[ends[0]], low[ends[1]], high[ends[1]], high[ends[0]]];
        match solid.edges {
            // A **lid** — a floor, a rug, a roof. One horizontal quad at the `z`
            // it lies at: a ray is stopped by crossing it, not by travelling
            // through it, so a box would draw a solid where the model has a
            // plane. Brightest, because it is the face that looks at the light.
            Edges::NONE => face(high.clone(), 1.0),
            // A **body** — a tree, a post, a graphic whose art names no edge. A
            // solid the ray travels through, so it is a box; and only the three
            // faces a camera can see are drawn, which is what makes it read as a
            // box rather than as a tangle. `Face::outward` names the two: an
            // isometric camera sees `+x` and `+y`.
            Edges::ANY => {
                face(wall_of(panel_edge(Edges::SOUTH)), Side::SOUTH_SHADE);
                face(wall_of(panel_edge(Edges::EAST)), Side::EAST_SHADE);
                face(high.clone(), 1.0);
            }
            // A **panel** — a wall standing on one named edge of its tile. One
            // vertical quad on that edge and nothing across the tile: a ray is
            // stopped where it pierces this plane and nowhere else, and a box
            // drawn round the whole tile is the picture decision 3 was written
            // against. The two faces a camera cannot see are drawn darker still
            // rather than dropped — a wall the eye cannot find is a wall this
            // view failed to report.
            named => {
                face(
                    wall_of(panel_edge(named)),
                    match named {
                        Edges::EAST => Side::EAST_SHADE,
                        Edges::SOUTH => Side::SOUTH_SHADE,
                        Edges::NORTH => 0.42,
                        _ => 0.58,
                    },
                )
            }
        }
    }
}

/// The sight line the shard decides a shot by: the ray, and what stopped it.
///
/// **Drawn at the ray's own height, not on the ground.** A look from a hill to a
/// hollow crosses tiles it is metres above, and a line laid on the land would
/// draw a ray bending over every rise it passes — which is the picture of a rule
/// this engine does not have. The height each segment is drawn at is the one
/// `sight::trace` decided the tile by, so a person can see the ray pass over the
/// low wall it passes over.
///
/// Green to the stop and red past it, in [`draw_route`]'s own vocabulary: this
/// is a second answer about the same ground and a second palette would make the
/// two read as unrelated pictures. The blocking body is drawn as a box from its
/// `base` to its `top`, which is the half of the answer no line can carry — a
/// wall the table gives no height is lent a whole storey, and this is where that
/// becomes a thing to look at rather than a thing to deduce.
fn draw_sight(painter: &egui::Painter, camera: &Camera, sight: &SightLine, viewport_origin: egui::Pos2) {
    // A `z` outside `i8` is not a height this world has: the ray can be lent one
    // by an interpolation over two extreme ends, and the picture stops at the
    // top of the world rather than wrapping round it.
    let height = |z: i32| z.clamp(i32::from(i8::MIN), i32::from(i8::MAX)) as i8;
    let at = |x: u16, y: u16, z: i32| {
        tile_centre(
            painter,
            camera,
            openshard_protocol::world::Point { x, y, z: height(z) },
            viewport_origin,
        )
    };
    let trace = &sight.trace;
    // The two ends are not steps of the line — an archer and their quarry do not
    // stand in their own way — so the polyline is built with them put back.
    let mut points = vec![at(trace.from.x, trace.from.y, i32::from(trace.from.z) + EYE)];
    points.extend(
        trace
            .steps
            .iter()
            .map(|step| at(step.tile.x, step.tile.y, step.ray_z)),
    );
    points.push(at(trace.to.x, trace.to.y, i32::from(trace.to.z) + EYE));
    // Where the ray gave up, as an index into that polyline: one past the
    // leading end, which the steps are offset by.
    let stopped = trace.stopped.and_then(|stop| {
        trace
            .steps
            .iter()
            .position(|step| step.tile == stop.tile)
            .map(|index| index + 1)
    });
    let reached = stopped.map_or(points.len(), |index| index + 1);
    // Where the weapon runs out, as an index into the same polyline: one past the
    // leading end again, since the steps are offset by it. The whole line is
    // within reach when the aim itself is — the count then runs to the last step
    // and the far endpoint is the target, which the shard would allow.
    let in_reach = match sight.within_reach() {
        true => points.len(),
        false => sight.steps_within_reach() + 1,
    };
    for (index, segment) in points[..reached].windows(2).enumerate() {
        // Two reasons a segment is not a shot, and the one that comes first
        // along the ray is the one drawn: a wall five tiles out is why the arrow
        // stops, whether or not the bow could have carried further. Past the
        // reach and short of the wall, the line is amber — the ray goes on and
        // the arrow does not.
        let colour = match index + 1 < in_reach {
            true => STANDABLE,
            false => OUT_OF_REACH,
        };
        painter.line_segment([segment[0], segment[1]], egui::Stroke::new(2.0, colour));
    }
    // And where it ran out, marked the way the stop is marked, so the two limits
    // read as the same kind of fact. Only when the aim is actually beyond it:
    // with the target in reach there is no limit crossed to point at.
    if in_reach < points.len() {
        let arm = 4.0;
        let at = points[in_reach - 1];
        let stroke = egui::Stroke::new(2.0, OUT_OF_REACH);
        painter.line_segment([at + egui::vec2(-arm, 0.0), at + egui::vec2(arm, 0.0)], stroke);
        painter.line_segment([at + egui::vec2(0.0, -arm), at + egui::vec2(0.0, arm)], stroke);
    }
    // Past the blocker the ray does not go, and the dashes say so — the same
    // thing a route's dashed half says about a walk that does not arrive.
    if reached < points.len() {
        for segment in points[reached - 1..].windows(2) {
            painter.add(egui::Shape::dashed_line(
                &[segment[0], segment[1]],
                egui::Stroke::new(2.0, BLOCKED),
                5.0,
                4.0,
            ));
        }
    }
    let Some(step) = trace.stopped else {
        return;
    };
    let stop_at = points[reached - 1];
    let arm = 5.0;
    let stroke = egui::Stroke::new(2.0, BLOCKED);
    painter.line_segment(
        [stop_at + egui::vec2(-arm, -arm), stop_at + egui::vec2(arm, arm)],
        stroke,
    );
    painter.line_segment(
        [stop_at + egui::vec2(-arm, arm), stop_at + egui::vec2(arm, -arm)],
        stroke,
    );
    // And the body itself, where it has one. A door has no span to draw — the
    // live layer is asked without a height at all — so the cross above is the
    // whole of what can be said about it.
    let span = match step.stop {
        Some(Stop::Static { base, top, .. }) | Some(Stop::LiveWall { base, top }) => Some((base, top)),
        // Ground is not a box: it is the tile's own surface, drawn as the
        // diamond it is, from the ray's height up to it.
        Some(Stop::Ground { z }) => Some((step.ray_z, z)),
        Some(Stop::Door) | None => None,
    };
    let Some((base, top)) = span else {
        return;
    };
    let corners = |z: i32| {
        tile_corners(
            painter,
            camera,
            openshard_protocol::world::Point {
                x: step.tile.x,
                y: step.tile.y,
                z: height(z),
            },
            viewport_origin,
        )
    };
    let (low, high) = (corners(base), corners(top));
    let fill = egui::Color32::from_rgba_unmultiplied(255, 40, 40, 70);
    let edge = egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(10, 8, 16, 220));
    // The three faces a camera looking from the north-west can see, in the order
    // the occluder wireframe draws them, so the two diagnostics read alike.
    for ends in [[3usize, 2], [1, 2]] {
        painter.add(egui::Shape::convex_polygon(
            vec![low[ends[0]], low[ends[1]], high[ends[1]], high[ends[0]]],
            fill,
            edge,
        ));
    }
    painter.add(egui::Shape::convex_polygon(high.to_vec(), fill, edge));
}

/// Which two corners of a tile's diamond a panel on `named` stands between.
///
/// `Camera::tile_facet` hands the corners back as `(x, y)`, `(x+1, y)`,
/// `(x+1, y+1)`, `(x, y+1)`, and a face is named for the world direction its
/// edge faces out of the tile — `crate::facing::Face`. So this is a table
/// between two orders, which is exactly the kind of thing that is written down
/// once, looks obvious, and is off by one corner in the picture; the test beside
/// it derives the same pairs from `Face::place_at`, which is what the *shader*
/// places a face pixel with, so the wireframe and the pixels cannot disagree
/// about which edge a wall is on.
fn panel_edge(named: openshard_client_render::occlusion::Edges) -> [usize; 2] {
    use openshard_client_render::occlusion::Edges;

    match named {
        Edges::NORTH => [0, 1],
        Edges::EAST => [1, 2],
        Edges::SOUTH => [3, 2],
        _ => [0, 3],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_legacy_house_query_requires_graphic_and_hue() {
        assert_eq!(
            parse_legacy_house_identity("0x0eed:0x0481"),
            Some(openshard_protocol::house_inventory::HouseItemIdentity::Legacy {
                graphic: Graphic(0x0EED),
                hue:     Hue(0x0481),
            })
        );
        assert_eq!(parse_legacy_house_identity("gold"), None);
    }

    /// Far is a smaller `x + y`, which is the order the solid is painted in.
    ///
    /// The one thing a painter's algorithm can get exactly backwards, and
    /// backwards it would look like a street drawn inside out — near walls behind
    /// far ones — which is a picture a person might well believe, because a
    /// wireframe looked like that too and nobody could tell. So the direction is
    /// taken from the projection rather than from the diagram in somebody's head:
    /// the tile one step along both axes lands *lower* on the screen, so it is
    /// nearer the eye and is painted later.
    #[test]
    fn a_tile_further_along_both_axes_is_nearer_the_eye() {
        let camera = Camera::new(openshard_protocol::world::Point::new(100, 100, 0), 800, 600);
        let middle = |point| {
            let corners = camera.tile_diamond(point);
            corners.iter().map(|corner| corner.y).sum::<f32>() / corners.len() as f32
        };
        let near = middle(openshard_protocol::world::Point::new(101, 101, 0));
        let far = middle(openshard_protocol::world::Point::new(100, 100, 0));
        assert!(near > far, "the nearer tile is at {near}, the further at {far}");
    }

    /// The wireframe stands a panel on the same edge the shader draws its pixels
    /// on.
    ///
    /// Two orders meet in [`panel_edge`] — the diamond's corners and
    /// `facing::Face`'s naming — and a table between two orders is the kind of
    /// thing that reads as obvious and is off by one corner on screen. So the
    /// pairs are derived here from `Face::place_at`, which is the Rust copy of
    /// what `statics.wgsl` places a face pixel with: the two ends of a face's run
    /// are its fractions at `0` and `1`, each fraction is a corner of the tile,
    /// and the corner's place in the diamond is `tile_facet`'s order. A view that
    /// drew a wall on the wrong side of its tile would be worse than no view —
    /// it is opened to answer exactly that question.
    #[test]
    fn the_wireframe_puts_a_panel_on_the_edge_its_pixels_are_drawn_on() {
        use openshard_client_render::facing::Face;
        use openshard_client_render::occlusion::Edges;

        // `Camera::tile_facet`'s own order, as the corner offsets it means.
        const DIAMOND: [(f32, f32); 4] = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
        let corner = |at: (f32, f32)| {
            DIAMOND
                .iter()
                .position(|it| *it == at)
                .expect("a face's end is a corner of its tile")
        };

        for (face, named) in [
            (Face::North, Edges::NORTH),
            (Face::East, Edges::EAST),
            (Face::South, Edges::SOUTH),
            (Face::West, Edges::WEST),
        ] {
            let want = [corner(face.place_at(0.0)), corner(face.place_at(1.0))];
            let drawn = panel_edge(named);
            // Either way round the edge is the same edge: what must not happen is
            // a different pair.
            let same = drawn == want || drawn == [want[1], want[0]];
            assert!(same, "{face:?} is drawn between {drawn:?}, its run is {want:?}");
        }
    }

    /// A rig printed by the panel is a line that compiles.
    ///
    /// The promise the sliders are for: a setting that felt right in the window
    /// is pasted into `follow.rs` and committed as the preset it turned out to
    /// be. Pinned because the failure is silent at the point it is made — the
    /// paste is a build error hours later, in another file.
    #[test]
    fn a_rig_prints_as_the_source_line_it_would_be() {
        assert_eq!(
            literal(&Rig::LIFT),
            "Rig { plane_tau: 0.0, lift_tau: 0.15, lift_cut: 64.0 }",
        );
        // `inf` is what `Display` would give, and it is not Rust.
        let never = Rig {
            lift_cut: f32::INFINITY,
            ..Rig::HARD
        };
        assert_eq!(
            literal(&never),
            "Rig { plane_tau: 0.0, lift_tau: 0.0, lift_cut: f32::INFINITY }",
        );
    }

    #[test]
    fn health_bars_keep_the_notoriety_palette_after_their_facts_leave_the_shell() {
        for (notoriety, colour) in [
            (Notoriety::Innocent, egui::Color32::from_rgb(70, 150, 255)),
            (Notoriety::Friend, egui::Color32::from_rgb(70, 210, 110)),
            (Notoriety::Neutral, egui::Color32::from_rgb(170, 170, 170)),
            (Notoriety::Criminal, egui::Color32::from_rgb(170, 170, 170)),
            (Notoriety::Enemy, egui::Color32::from_rgb(230, 145, 55)),
            (Notoriety::Murderer, egui::Color32::from_rgb(220, 55, 45)),
            (Notoriety::Invulnerable, egui::Color32::from_rgb(240, 220, 70)),
        ] {
            assert_eq!(health_colour(notoriety), colour, "{notoriety:?}");
        }
    }

    /// The reason an action stopped is the whole of what `CombatActionEnded`
    /// was added to say, so every arm of the wire's list has a phrase of its
    /// own. A new one that fell through to a catch-all would un-say it for
    /// exactly the case somebody had just gone to the trouble of naming — which
    /// is what this asserts by naming them all here too.
    #[test]
    fn every_way_an_action_can_end_has_a_word_of_its_own() {
        let mut said = std::collections::HashSet::new();
        for outcome in [
            CombatActionOutcome::Hit,
            CombatActionOutcome::Miss,
            CombatActionOutcome::Expired,
            CombatActionOutcome::Interrupted(InterruptReason::TargetGone),
            CombatActionOutcome::Interrupted(InterruptReason::OutOfReach),
            CombatActionOutcome::Interrupted(InterruptReason::NoLineOfSight),
            CombatActionOutcome::Interrupted(InterruptReason::Pacified),
            CombatActionOutcome::Interrupted(InterruptReason::Abandoned),
            CombatActionOutcome::Interrupted(InterruptReason::NoAmmo),
            CombatActionOutcome::Interrupted(InterruptReason::Moved),
            CombatActionOutcome::Interrupted(InterruptReason::Struck),
        ] {
            assert!(
                said.insert(outcome_label(outcome)),
                "{outcome:?} shares its word with another ending",
            );
        }
    }

    /// A kind and an outcome are two different questions, and a palette that
    /// answered both in one colour would make a landed blow and a swing the
    /// same picture.
    #[test]
    fn the_action_palette_separates_the_three_kinds_and_the_four_endings() {
        let kinds = [
            CombatActionKind::Swing,
            CombatActionKind::Shot,
            CombatActionKind::Breath,
        ]
        .map(action_colour);
        let endings = [
            CombatActionOutcome::Hit,
            CombatActionOutcome::Miss,
            CombatActionOutcome::Expired,
            CombatActionOutcome::Interrupted(InterruptReason::Moved),
        ]
        .map(outcome_colour);
        let distinct: std::collections::HashSet<_> = kinds.iter().chain(endings.iter()).collect();
        assert_eq!(distinct.len(), kinds.len() + endings.len());
    }

    /// The brief loose after overwatch is a second state beside the release,
    /// not the start of another draw.  The two labels must stay separate so the
    /// short bar cannot be mistaken for the bow being pulled again.
    #[test]
    fn a_loosed_held_bow_names_its_drawn_state_beside_the_release() {
        assert_eq!(
            action_state_labels(
                CombatActionKind::Shot,
                ActionFill::Releasing { filled: 0.0 },
                ActionStage::Release,
                true,
            ),
            ("loosing", Some("bow drawn")),
        );
        assert_eq!(
            action_state_labels(
                CombatActionKind::Shot,
                ActionFill::Releasing { filled: 0.0 },
                ActionStage::Release,
                false,
            ),
            ("loosing", None),
            "a new shot may not pretend it was already held",
        );
    }

    #[test]
    fn route_height_gradient_uses_the_open_route_minimum_and_maximum_z() {
        let route = [
            openshard_protocol::world::Point::new(10, 10, -15),
            openshard_protocol::world::Point::new(11, 10, 0),
            openshard_protocol::world::Point::new(12, 10, 45),
        ];
        let (min_z, max_z) = route_height_range(&route);

        assert_eq!((min_z, max_z), (-15, 45));
        assert_eq!(route_height_colour(min_z, min_z, max_z), STANDABLE);
        assert_eq!(route_height_colour(max_z, min_z, max_z), ROUTE_HIGH_Z);
        assert_eq!(
            route_height_colour(0, min_z, max_z),
            egui::Color32::from_rgb(60, 225, 131),
            "an intermediate floor stays between green and blue",
        );
    }

    #[test]
    fn a_level_route_remains_walkable_green() {
        assert_eq!(route_height_colour(20, 20, 20), STANDABLE);
    }

    #[test]
    fn the_admin_item_form_accepts_hex_and_rejects_zero_amount() {
        let valid = crate::desk::AdminItem {
            graphic:   "0x0eed".to_owned(),
            hue:       "0x0481".to_owned(),
            amount:    "25".to_owned(),
            stackable: true,
        };
        assert_eq!(
            parse_admin_item(&valid),
            Ok(AdminItemRequest::LegacyArt {
                graphic:   0x0eed,
                hue:       0x0481,
                amount:    25,
                stackable: true,
            })
        );

        let empty = crate::desk::AdminItem {
            amount: "0".to_owned(),
            ..valid
        };
        assert_eq!(
            parse_admin_item(&empty),
            Err("Amount must be a whole number from 1 to 65535.")
        );
    }

    #[test]
    fn the_admin_catalogue_contains_constructible_semantic_items_only() {
        let matches = matching_item_entries("dagger", crate::desk::AdminItemCategory::Weapons);
        assert!(matches.iter().any(|entry| {
            semantic_catalogue_identity(entry)
                == Some((
                    openshard_protocol::item_kind::ItemKindId(68),
                    Some(openshard_protocol::item_kind::MaterialId(1)),
                ))
        }));
        assert!(
            matches
                .iter()
                .all(|entry| semantic_catalogue_identity(entry).is_some()),
            "the F1 list never offers the material-less umbrella selectors used only by house search"
        );
    }

    #[test]
    fn the_admin_skill_form_makes_the_existing_staff_command() {
        let skill = crate::desk::AdminSkill {
            name:  "Item Identification".to_owned(),
            value: "95.5".to_owned(),
        };
        assert_eq!(
            parse_admin_skill(&skill),
            Ok(".skill ItemIdentification 95.5".to_owned())
        );

        let invalid = crate::desk::AdminSkill {
            value: "95.55".to_owned(),
            ..skill
        };
        assert_eq!(
            parse_admin_skill(&invalid),
            Err("Enter a whole value or one decimal place, e.g. 95 or 95.5.")
        );
    }

    #[test]
    fn craft_filter_combines_text_and_ready_state() {
        let row = openshard_protocol::craft::CraftCatalogueRow {
            button:           8,
            admin_button:     9,
            result:           Graphic(0x13EB),
            result_hue:       Hue::NONE,
            result_item_kind: None,
            name:             ClilocId(0),
            skill:            ClilocId(0),
            skill_min:        0,
            ready:            false,
            weapon:           None,
            components:       Vec::new(),
        };

        assert!(craft_matches(
            &row,
            "0x13eb",
            CraftAvailability::All,
            None,
            CraftMaterials::Any,
            None
        ));
        assert!(!craft_matches(
            &row,
            "0x13eb",
            CraftAvailability::Ready,
            None,
            CraftMaterials::Any,
            None
        ));
        assert!(!craft_matches(
            &row,
            "dagger",
            CraftAvailability::All,
            None,
            CraftMaterials::Any,
            None
        ));

        let compound = openshard_protocol::craft::CraftCatalogueRow {
            skill: ClilocId(7),
            components: vec![
                openshard_protocol::craft::CraftCatalogueComponent {
                    stock_key: openshard_protocol::craft::CraftKey(0),
                    item_kind: None,
                    material:  None,
                    graphic:   Graphic(0x1BF2),
                    hue:       Hue::NONE,
                    name:      ClilocId(0),
                    amount:    2,
                },
                openshard_protocol::craft::CraftCatalogueComponent {
                    stock_key: openshard_protocol::craft::CraftKey(1),
                    item_kind: None,
                    material:  None,
                    graphic:   Graphic(0x0F8D),
                    hue:       Hue::NONE,
                    name:      ClilocId(0),
                    amount:    1,
                },
            ],
            ..row
        };
        assert!(craft_matches(
            &compound,
            "0x1bf2",
            CraftAvailability::All,
            None,
            CraftMaterials::Several,
            None
        ));
        assert!(!craft_matches(
            &compound,
            "",
            CraftAvailability::All,
            None,
            CraftMaterials::One,
            None
        ));
        assert!(craft_matches(
            &compound,
            "",
            CraftAvailability::All,
            Some(7),
            CraftMaterials::Any,
            None
        ));
        assert!(!craft_matches(
            &compound,
            "",
            CraftAvailability::All,
            Some(8),
            CraftMaterials::Any,
            None
        ));
    }

    #[test]
    fn craft_labels_drop_gump_markup_and_slots_owned_by_typed_fields() {
        assert_eq!(
            craft_plain_text("<CENTER>CARPENTRY MENU</CENTER>"),
            "CARPENTRY MENU"
        );
        assert_eq!(craft_plain_text("WOOD (~1_AMT~)"), "WOOD");
        assert_eq!(craft_plain_text("  plain   recipe  "), "plain recipe");
    }

    #[test]
    fn craft_page_replacement_keeps_the_single_window_state() {
        let mut panel = CraftWindowPanel {
            gump_id: Some(0x0052_0001),
            query: "dagger".to_owned(),
            table_scroll: egui::vec2(37.0, 19.0),
            row_scroll: 420.0,
            recipe_scroll: 84.0,
            awaiting_page: true,
            ..CraftWindowPanel::default()
        };

        // AnswerGump removes the authoritative page before its replacement
        // arrives. That gap is navigation, not a newly opened craft window.
        panel.page_missing();
        assert_eq!(panel.gump_id, Some(0x0052_0001));
        assert_eq!(panel.query, "dagger");
        assert_eq!(panel.table_scroll, egui::vec2(37.0, 19.0));
        assert_eq!(panel.row_scroll, 420.0);
        assert_eq!(panel.recipe_scroll, 84.0);

        // A genuinely absent window still releases the session state.
        panel.awaiting_page = false;
        panel.page_missing();
        assert_eq!(panel.gump_id, None);
        assert_eq!(panel.table_scroll, egui::Vec2::ZERO);
        assert_eq!(panel.row_scroll, 0.0);
        assert_eq!(panel.recipe_scroll, 0.0);
    }
}
