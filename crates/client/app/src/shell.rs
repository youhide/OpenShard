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

use std::time::Duration;

use openshard_client_render::bench::Reading;
use openshard_client_render::blit::ViewportRect;
use openshard_client_render::camera::Camera;
use openshard_client_render::facing::{Face, Prism};
use openshard_client_render::follow::Rig;
use openshard_client_render::light;
use openshard_client_render::solid::Cut;
use openshard_protocol::mobile::Notoriety;
use openshard_protocol::wire::{Graphic, Hue};
use winit::window::Window;

use crate::desk::{Desk, Tab};
use crate::diagnostics::{
    HealthBar, Height, Hud, Navigation, PickedTile, PriorityZ, Route, Selection, TerrainOverlay,
};
use crate::graphics::{HighlightStyle, HighlightTarget};
use crate::world::{Shard, WorldState};

/// What the panels asked for this frame.
///
/// No longer `Copy`: one of these carries what the player typed. A request is
/// built fresh each frame and spent by the caller, so cloning it is not a thing
/// that happens on any path.
#[derive(Clone, Default, Debug)]
pub struct Request {
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
}

/// What the script picker asked for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ScriptRequest {
    /// Walk this scenario from its start.
    Run(&'static str),
    /// Stop wherever it got to.
    Stop,
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
            viewport: ViewportRect {
                x: 0,
                y: 0,
                width: size.width.max(1),
                height: size.height.max(1),
            },
            // Until the first frame has run there is nothing to wait for; the
            // animation clock is what wakes the loop.
            repaint_after: std::time::Duration::MAX,
            desk,
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
    /// is only refreshed when the client exits and writes `client_ui.toml`.
    pub fn fonts(&self) -> crate::desk::FontSizes {
        self.desk.fonts
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

    /// Show or hide the dev window — the strip's `dev` toggle, reached from a key.
    ///
    /// It has to come through here, and not through the app's own [`Desk`]: the
    /// one the panels are laid out against is *this* one. The app's copy is what
    /// was loaded at startup and what will be written at exit, and between those
    /// two moments nothing draws from it, so a key that flipped it would change a
    /// value nobody reads and take effect on the next launch.
    pub fn toggle_dev(&mut self) {
        self.desk.open = !self.desk.open;
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
    /// Today this is always `false`: there is not one `egui::TextEdit` in this
    /// client, because every box a player types into is drawn by `chat.rs` or by
    /// `panes.rs`. It is asked rather than assumed because the day a text field
    /// appears in a panel is the day it has to work, and a hard-coded `false`
    /// would be a keyboard that never reached it.
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
    pub fn run(
        &mut self,
        window: &Window,
        hud: &Hud,
        camera: Camera,
        world: &WorldState,
        map_editor: &mut crate::editor_mode::MapEditor,
        authority: openshard_protocol::access::AccessLevel,
    ) -> (Request, egui::FullOutput) {
        let input = self.state.take_egui_input(window);
        let mut request = Request::default();
        // What the panels leave behind, taken from the root `Ui` *after* they
        // have claimed their edges. That rectangle is the world's viewport, so
        // a docked panel shrinks the world and a floating window sits over it.
        let mut free = egui::Rect::from_min_size(egui::Pos2::ZERO, self.context.content_rect().size());
        let desk = &mut self.desk;
        let output = self.context.run_ui(input, |ui| {
            request = layout(ui, hud, camera, world, map_editor, authority, desk);
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
            |ui| draw_world_overlays(ui.ctx(), hud, camera, free),
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
        Self::paint_output(
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
            Self::paint_output(
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
        output: egui::FullOutput,
        size_in_pixels: [u32; 2],
    ) {
        let pixels_per_point = output.pixels_per_point;
        debug_assert_eq!(
            pixels_per_point,
            context.pixels_per_point(),
            "{} used an output from a different egui context",
            layer.pass_label(),
        );
        let jobs = context.tessellate(output.shapes, pixels_per_point);
        for (id, delta) in &output.textures_delta.set {
            renderer.update_texture(device, queue, *id, delta);
        }
        let descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels,
            pixels_per_point,
        };
        renderer.update_buffers(device, queue, encoder, &jobs, &descriptor);

        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(layer.pass_label()),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    // Over the world, not instead of it.
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            multiview_mask: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        renderer.render(&mut pass.forget_lifetime(), &jobs, &descriptor);

        for id in &output.textures_delta.free {
            renderer.free_texture(id);
        }
    }
}

/// The panels, and the server's own dialogs.
///
/// Deliberately absent: the paperdoll, containers, and the speech line and
/// journal, which are now `App::chat`'s and drawn through
/// `openshard_client_render::gump::GumpRenderer` — see `App::draw` — rather
/// than egui's. Building the paperdoll and containers here would decide M4
/// without arguing it; the speech line already had that argument, in the
/// commit that moved it off this file.
fn layout(
    root: &mut egui::Ui,
    hud: &Hud,
    camera: Camera,
    world: &WorldState,
    map_editor: &mut crate::editor_mode::MapEditor,
    authority: openshard_protocol::access::AccessLevel,
    desk: &mut Desk,
) -> Request {
    let mut request = Request::default();
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
        Some(panel) => window
            .default_pos([panel.x, panel.y])
            .default_size([panel.width, panel.height]),
        None => window.default_pos([16.0, 48.0]).default_size([360.0, 420.0]),
    };
    let placed = window.show(&context, |ui| {
        ui.horizontal(|ui| {
            for tab in Tab::ALL {
                if ui.selectable_label(desk.tab == tab, tab.title()).clicked() {
                    desk.tab = tab;
                }
            }
        });
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
            .show(ui, |ui| match desk.tab {
                Tab::Camera => camera_panel(ui, hud, camera, &mut desk.movement, &mut request),
                Tab::Rig => rig_panel(ui, hud, world, &mut request),
                Tab::Frames => frames_panel(ui, hud),
                Tab::World => world_panel(ui, hud, world, &mut request),
                Tab::Tile => tile_tab(ui, hud, world, &mut request),
                Tab::Light => light_panel(ui, hud, &mut desk.light, &mut request),
                Tab::Chat => chat_panel(ui, &mut desk.chat, &mut desk.fonts, hud.ttf_active),
                Tab::Audio => audio_panel(ui, &mut desk.audio, &mut request),
                Tab::Windows => windows_panel(ui, &mut desk.window_scale),
            });
    });
    desk.open = open;
    // What egui made of it, read back after the frame it was laid out in: this
    // is the rect that goes in the file, and it is the one the window is
    // actually at rather than the one it was asked for.
    if let Some(placed) = placed {
        let rect = placed.response.rect;
        desk.panel = Some(crate::desk::Panel {
            x: rect.min.x,
            y: rect.min.y,
            width: rect.width(),
            height: rect.height(),
        });
    }

    request
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
/// lies. What is *not* here is a switch for night, the sun, the lantern or the
/// sky field: those are F10, F8, F7 and F6 and have been since before this tab,
/// and two ways to spell one state is how the two come to disagree.
fn light_panel(ui: &mut egui::Ui, hud: &Hud, light: &mut crate::desk::Light, request: &mut Request) {
    let most = light::Tuning::MOST;
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
/// `ttf_active` — `App::ttf_font.is_some()` — picks which kind of size control
/// is shown, because the two faces are sized by two different kinds of number
/// and only one face draws in a given run. A TrueType face has a **real size
/// in pixels** per kind of text ([`crate::desk::FontSizes`]); `fonts.mul` has
/// a fixed height per face and an integer upscale on top of it
/// ([`crate::desk::ChatScale`]). Showing both would leave one of them a
/// control for nothing on screen. See `docs/text_sizes.md`.
fn chat_panel(
    ui: &mut egui::Ui,
    chat: &mut crate::desk::Chat,
    fonts: &mut crate::desk::FontSizes,
    ttf_active: bool,
) {
    use crate::desk::ChatScale;
    use openshard_client_render::atlas::TextSize;

    ui.label("Size");
    if ttf_active {
        // One row per role, each a real pixel size — see `FontSizes`. A tenth
        // of a pixel a step, because the whole point of a rasterized size is
        // that 13.5 is a size a person can actually have.
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
        row(ui, "tooltip", &mut fonts.tooltip);
        row(ui, "count", &mut fonts.stack_count);
        ui.label(
            egui::RichText::new(
                "Real sizes, in pixels, rasterized at that size rather than \
                 scaled up from one — so a fraction is a size and not a \
                 stretch. `speech` is a line over a head and the box below; \
                 `window` is this client's own window captions; `count` is the \
                 number written on a pile. A dense display multiplies these \
                 before the glyph is drawn, never after.",
            )
            .small()
            .weak(),
        );
    } else {
        let mut scale = chat.scale.glyph_scale_factor();
        if ui
            .add(egui::Slider::new(&mut scale, ChatScale::MIN..=ChatScale::MAX).text("scale"))
            .changed()
        {
            chat.scale = ChatScale::new(scale);
        }
        ui.label(
            egui::RichText::new(
                "An integer upscale on `fonts.mul`'s own pixels — a bitmap face has \
                 no continuous size to ask for instead. Only the journal and the \
                 compose line below it; a shard's own dialogs draw at the size it \
                 sent them.",
            )
            .small()
            .weak(),
        );
    }

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
    }
}

/// How big the client's own windows draw — a bag, a doll, a shop, a sheet.
///
/// One knob for all of them rather than one per kind: see
/// [`crate::desk::WindowScale`], whose doc says why an item that changed size
/// on its way between two windows is the reason.
fn windows_panel(ui: &mut egui::Ui, scale: &mut crate::desk::WindowScale) {
    use crate::desk::WindowScale;

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
    if ui.button("back to the defaults").clicked() {
        *scale = WindowScale::default();
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
             About eleven seconds on a facet; the client keeps playing while it runs.",
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
    // the crowd off draws the same street with nobody in it.
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
        (
            &mut draw.items,
            "the server's items — what was dropped and what a pack placed",
        ),
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
                Cut::BelowFeet(_) => format!(
                    "{drawn} surfaces above your feet, {} below and not drawn",
                    total - drawn
                ),
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
            Some(picked) => format!(
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
        Selection::Static { static_, .. } => Some(Marked::Static {
            graphic: static_.graphic,
            height: Height(static_.at.z),
        }),
        Selection::Item(Some((item, _))) => Some(Marked::Item {
            graphic: item.graphic,
            height: Height(item.at.z),
        }),
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
fn draw_world_overlays(context: &egui::Context, hud: &Hud, camera: Camera, viewport: egui::Rect) {
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
    draw_health_bars(&world, &camera, &hud.health_bars, viewport.min);
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
/// `docs/camera.md`, C4. From here on every remaining decision about the camera
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
    // this is a property of the body the eye is looking at (`docs/camera.md`
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
            Some(quarantine) => format!(
                "{:?} block {:?}, key {:?}, owner {:?}",
                quarantine.reason, quarantine.block, quarantine.key, quarantine.ground
            ),
            None => "none".to_owned(),
        });
        ui.end_row();
    });
    radar_report(ui, hud, mib);
    // The counter `docs/camera.md` asks for: without it, a full atlas repack
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
                name: "ui",
                points: series(|frame| frame.ui.as_secs_f64() * 1_000.0),
                colour: egui::Color32::from_rgb(150, 180, 240),
            },
            Curve {
                name: "world",
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
                name: "gpu",
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
/// R7 of `docs/map/radar.md`. Every number here was already being written and
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
    name: &'a str,
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
    // project convention (`docs/protocol_newtypes.md`), so this is where
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
            true => (
                washed(colour, 72 + (cell.floor % 3) as u8 * 12),
                washed(colour, 180),
            ),
            false => (
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 220),
                egui::Color32::from_rgba_unmultiplied(80, 80, 80, 230),
            ),
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
/// since `docs/lighting.md`'s step 21.2. `Occlusion::at` — what `boxes()` hands
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
/// `docs/lighting.md`, step 14, and it is an instrument rather than a picture:
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
            named => face(
                wall_of(panel_edge(named)),
                match named {
                    Edges::EAST => Side::EAST_SHADE,
                    Edges::SOUTH => Side::SOUTH_SHADE,
                    Edges::NORTH => 0.42,
                    _ => 0.58,
                },
            ),
        }
    }
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
}
