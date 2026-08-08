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

use openshard_client_render::bench::{Metrics, Reading};
use openshard_client_render::blit::ViewportRect;
use openshard_client_render::camera::Camera;
use openshard_client_render::follow::Rig;
use openshard_client_render::solid::Cut;
use openshard_uofiles::hues::Hues;
use winit::window::Window;

use crate::desk::{Desk, Tab};

/// What the panels are asked to display.
///
/// A snapshot built by the caller each frame rather than a borrow of the app:
/// the HUD is a projection of state it does not own, and this is the list of
/// what it is allowed to know.
pub struct Hud {
    /// The shard, if there is one, and what it is doing.
    pub connection: String,
    /// Our own serial, once a shard has given us one.
    pub serial: Option<u32>,
    /// Where our body stands, as the server last said.
    pub position: openshard_protocol::world::Point,
    /// The camera, read for its zoom, eye and viewport.
    pub camera: Camera,
    /// Whether the camera is locked to the body.
    pub locked: bool,
    /// What the eye is following with — every number a camera is made of.
    pub rig: Rig,
    /// How far the drawn bodies lag the walk they are doing. Beside the rig and
    /// not inside it — see the slider.
    pub ease: crate::crowd::Ease,
    /// The last few seconds of the eye, one entry per frame.
    ///
    /// Owned rather than borrowed because the HUD is a snapshot and not a view
    /// of the app; a few hundred `f64`s a frame is what that costs, and it is
    /// what keeps the panels unable to reach back into the camera.
    pub readings: Vec<Reading>,
    /// What those frames come to, and `None` before there are enough of them to
    /// difference. Absent rather than zeroed: a metric over one frame is not a
    /// small number, it is not a number.
    pub metrics: Option<Metrics>,
    /// How long a window the scope keeps, for the chart's own axis.
    pub scope_span: Duration,
    /// The last few seconds of the event loop, one entry per drawn frame.
    pub frames: Vec<crate::frames::Frame>,
    /// How long a window those cover, for that chart's own axis.
    pub frames_span: Duration,
    /// The worst frame rate in that window, and `None` before there is a frame
    /// to have a rate.
    pub worst_fps: Option<f64>,
    /// How many full atlas repacks this session has paid for. See
    /// [`crate::frames::Frame::repacked`] for which frame in the window below
    /// was one of them.
    pub repacks: u64,
    /// What is currently asking for frames.
    ///
    /// Shown beside the rate because it is the *reason* for it: a client paced
    /// by the display and one paced by the animation clock report the same kind
    /// of number and mean opposite things by it, and a panel that only showed
    /// the rate would read the second as a fault.
    pub pacing: crate::frames::Pacing,
    /// The bench's scenarios, by name, in the order it ships them.
    pub scripts: Vec<&'static str>,
    /// The one being replayed, and how far through it is from zero to one.
    pub replay: Option<(&'static str, f32)>,
    /// Whether there is no shard, which is the only state a replay may run in:
    /// connected, the body goes where the `0x22` says, and a second writer is
    /// two clients fighting over one character.
    pub offline: bool,
    /// Everyone else on screen: serial, body, position.
    pub mobiles: Vec<(u32, u16, openshard_protocol::world::Point)>,
    /// The ground items the view is holding: serial, graphic, position.
    pub items: Vec<(u32, u16, openshard_protocol::world::Point)>,
    /// What tile the cursor is over right now, if it is over the world and on
    /// the map. Live, and gone the instant the cursor leaves — see `selected`
    /// for what a click keeps.
    ///
    /// The *fact*, always answered: the panel reads it whatever is highlighted,
    /// and so does the route the terrain overlay draws. Whether the marker is
    /// drawn on it is [`Hud::hover_lit`].
    pub hover: Option<PickedTile>,
    /// The eight tiles around [`Hud::hover`], drawn as bare wireframes beside
    /// its box so the ground's *slope* is visible and not just its height.
    ///
    /// Empty when nothing is hovered, and short by however many of the eight
    /// fell off the map.
    pub neighbours: Vec<PickedTile>,
    /// Whether the tile marker is this frame's highlight.
    ///
    /// False when an item took it — see [`HighlightTarget`]. Decided by the app
    /// and not here, because it is the answer to "is there an item under the
    /// cursor", which is a question about the world rather than about the HUD.
    pub hover_lit: bool,
    /// What the two object picks answered this frame: the creature under the
    /// cursor and the item under it, as indices into the lists the passes draw
    /// from.
    ///
    /// An instrument, and one worth its line: "nothing is highlighted" has two
    /// completely different causes — a pick that found nothing, and a pick that
    /// found something the ring pass then failed to draw — and from the picture
    /// alone they are the same blank screen.
    pub lit_mobile: Option<usize>,
    /// The item half of the same. Never `Some` at the same time as
    /// [`Hud::lit_mobile`]: one highlight a frame, and creatures win.
    pub lit_item: Option<usize>,
    /// And the map's own furniture, which is the third and last link of that
    /// chain: answered only where neither of the two above found anything, since
    /// a wall loses to everything that has a serial.
    ///
    /// It is not drawn as a highlight today — what a *click* on it does is the
    /// wash, and that is held rather than hovered — but it is what puts the tile
    /// marker out: pointing at a wall and diamonding the ground behind it is one
    /// question answered twice. Shown here because that absence otherwise has no
    /// visible cause.
    pub lit_static: Option<openshard_client_render::statics::PickedStatic>,
    /// Which of the two the cursor may light, for the picker that says so.
    pub highlight: HighlightTarget,
    /// And how an item says it, when it is the one lit.
    pub highlight_style: HighlightStyle,
    /// The static a left click last landed on — a wall, a stair, a door frame —
    /// kept until the next click and washed in the world by
    /// `openshard_client_render::select`.
    ///
    /// Named here as well as washed there, because the wash says *which* and this
    /// says *what*: a graphic id and the height it stands at are what a person
    /// looking at a piece of the map is after, and they are exactly what the
    /// picture cannot show. It is also the companion the wash needs — a selection
    /// that drew nothing and a selection that was never made are one blank screen
    /// otherwise.
    pub selected_static: Option<openshard_client_render::statics::PickedStatic>,
    /// The tile a left click last landed on. Kept until the next click, which
    /// is what makes its numbers holdable still long enough to copy — the
    /// live hover moves out from under the cursor the moment it does.
    pub selected: Option<PickedTile>,
    /// Whether the terrain overlay is switched on, for the checkbox that says so.
    pub show_terrain: bool,
    /// What that overlay draws, gathered only while it is on: see
    /// [`TerrainOverlay`].
    pub terrain: Option<TerrainOverlay>,
    /// Whether the occluder boxes are switched on, for the checkbox that says so.
    pub show_occluders: bool,
    /// And whether the same grid is being drawn as solids — step 23.0.
    pub show_solids: bool,
    /// Whether the world image is skipped while solids are drawn: boxes alone
    /// over a blank frame, with nothing of the sprite underneath to compare
    /// them against. F5 draws the box *and* the art it claims to contain, on
    /// purpose (decision 39.2) — this is for the opposite question, "is the
    /// box itself the shape I think it is", which a sprite drawn between its
    /// faces makes harder to read rather than easier.
    pub solids_only: bool,
    /// Whether the solids view's fills are a straight overwrite instead of
    /// blended in — `solids::Style::opaque`, for the checkbox that says so.
    ///
    /// Off by default, matching the translucent fill F5 always drew before
    /// this existed. On, a later, nearer face genuinely hides an earlier,
    /// farther one instead of tinting through it — the same picture
    /// `OPENSHARD_SCENE_SOLIDS_OPAQUE` gives `isolated_scene`, now reachable
    /// without the env var and the debug build it requires.
    pub solids_opaque: bool,
    /// How much of the grid either view draws — [`Cut`], the second datum.
    ///
    /// Resolved rather than chosen: [`Cut::BelowFeet`] carries the player's own
    /// `z`, which is a fact about this frame, so what the HUD is handed is the
    /// cut in force and not the person's preference. What comes back through
    /// [`Request::solid_cut`] is the variant they picked, and the `z` in it is
    /// whatever was in this one.
    pub solid_cut: Cut,
    /// What the last frame's solids pass was handed, and what it drew: the rest
    /// fell outside the viewport. Both zero while the view is off.
    pub solids: (usize, usize),
    /// The lighting's own occlusion grid, gathered only while they are — the
    /// boxes a shadow ray walks through, drawn as the boxes they are.
    ///
    /// The grid itself and not a list of shapes: what is being looked at is
    /// whether the *grid* is what the picture says it is, so anything this
    /// overlay derived for itself would be a second answer to that question.
    pub occluders: Option<openshard_client_render::occlusion::Occlusion>,
    /// The tile the body is walking to, while it still is.
    ///
    /// The one piece of feedback a move order needs: a click that named a tile
    /// the shard then refuses to walk to looks exactly like a click that was
    /// never registered, and this is what tells the two apart.
    pub goal: Option<PickedTile>,
    /// The dialogs the server has open on this client, waiting to be answered.
    pub gumps: Vec<openshard_client_net::view::OpenGump>,
    /// The last few lines the shard has said, oldest first.
    ///
    /// Not the journal M4 will build — see [`layout`]'s docs. What it is for is
    /// that a system message has no mobile behind it, so it is drawn over
    /// nobody's head and a client with only overhead speech never shows it. A
    /// refused `.admin` says "you are not a game master" and nothing else, and
    /// without this strip that answer is invisible.
    pub said: Vec<String>,
}

/// A tile, read straight from the map — for telling a rendering artifact apart
/// from a gameplay one: is the graphic under a glitch the tile the client
/// thinks is there, or something else entirely?
#[derive(Clone)]
pub struct PickedTile {
    /// The tile coordinate, resolved from the cursor via [`Camera::pick`] and
    /// [`unproject`](openshard_client_render::camera::unproject).
    pub x: u16,
    /// The tile coordinate's other half.
    pub y: u16,
    /// The land tile's graphic id, if the block loaded.
    pub land: Option<u16>,
    /// The ground's height here — what the land block stores, and nothing else.
    /// Shown in the panel as a fact about the map; it is *not* where a body
    /// stands, and nothing is drawn at it. See [`PickedTile::stand_z`].
    pub land_z: i8,
    /// The height a body would stand at here: the ground, or the deck of
    /// whatever platform is on top of it.
    ///
    /// Everything the HUD draws over a tile uses this. A pier is the case that
    /// forced it apart from [`PickedTile::land_z`]: the land under one is water
    /// at `-15` and the planks are at `-3`, so a diamond drawn at the land's
    /// height lies a tile and a half away from the boards on screen — and since
    /// the cursor is resolved against the same height, a pier tile could not be
    /// pointed at at all.
    pub stand_z: i8,
    /// The four corner heights of the surface [`PickedTile::stand_z`] names, in
    /// [`Camera::tile_facet`]'s order — top, right, bottom, left.
    ///
    /// A land tile is a sloped diamond and `stand_z` is one number over the
    /// middle of it, so a marker drawn flat at that height cuts through the
    /// hillside at two corners and floats over it at the other two. These are
    /// what the ground pass lifts its own vertices by, so the marker lies on the
    /// art rather than near it.
    ///
    /// `[stand_z; 4]` when the surface is not the land — a pier's planks are
    /// flat however the water beneath them is shaped.
    pub corners: [i8; 4],
    /// Every height a body could stand at on this tile, and whether one
    /// actually fits there — sorted, and asked of the same terrain every step
    /// decision on this end asks, clutter included.
    ///
    /// A tile is a *column*, not a height: a stair carries the floor under it
    /// and the tread above, a house carries its ground floor and the storey
    /// over that, and "why will it not let me stand here" is a question about
    /// which of those a body fits on. `stand_z` names the one the cursor
    /// resolved to; this is the whole list it was chosen from, with the verdict
    /// beside each.
    pub levels: Vec<(i8, bool)>,
    /// How high the things on this tile reach — a roof over a room, the cap of
    /// a wall — or `None` where nothing stands on it.
    ///
    /// What the marker's column is drawn up to, so a tile indoors is a box from
    /// the datum to the ceiling rather than a box that stops at the floor and
    /// says nothing about the room it is in.
    pub ceiling: Option<i8>,
    /// Everything standing on top of the ground here: graphic id, height, hue.
    pub statics: Vec<(u16, i8, u16)>,
}

/// The walkability of what is on screen, and the way through it.
///
/// A debugging picture of the *movement* crate's own answers, drawn over the
/// ground they are about: pathing bugs are the kind that can be reasoned about
/// for an hour and seen in a second — a doorway the plan will not take is a red
/// diamond in a gap that looks open, and a route that goes the long way round
/// says so on the map rather than in a log line.
///
/// Every point carries the `z` its diamond is drawn at, so a tile lies on the
/// surface a body would actually stand on rather than on the bare ground under a
/// building's floor.
pub struct TerrainOverlay {
    /// The tiles in view a body can stand on.
    pub open: Vec<openshard_protocol::world::Point>,
    /// The tiles in view it cannot — no surface, or something solid in the way.
    pub blocked: Vec<openshard_protocol::world::Point>,
    /// The route being walked, or the one that would be walked to the tile under
    /// the cursor: the body's own tile first, then one point per step.
    pub route: Vec<openshard_protocol::world::Point>,
}

/// What the panels asked for this frame.
///
/// No longer `Copy`: two of these carry what the player typed. A request is
/// built fresh each frame and spent by the caller, so cloning it is not a thing
/// that happens on any path.
#[derive(Clone, Default, Debug)]
pub struct Request {
    /// Put the eye back on the body and lock it there.
    pub relock: bool,
    /// Let go of the body.
    pub unlock: bool,
    /// A line the player pressed Enter on. Sent as speech exactly as typed —
    /// a `.`-prefixed line is a staff command *on the server*, and a client that
    /// recognised its own would be deciding what a shard's commands are.
    pub say: Option<String>,
    /// A dialog the player answered. See [`crate::gump`].
    pub gump: Option<crate::link::GumpReply>,
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
}

/// What the cursor is allowed to light up.
///
/// Two kinds of highlight exist and they say different things: a *tile* marker
/// is the ground the click would walk to, and an *object* highlight is the thing
/// the click would act on — an item on the ground or a creature standing on it.
/// Drawn together they contradict each other — the ring round a barrel and a
/// diamond on the ground under it are two answers to one question — so one of
/// them wins per frame, and this is who.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum HighlightTarget {
    /// The creature or item under the cursor when there is one, the tile under
    /// it otherwise. What a player wants without being asked: pointing at a
    /// shopkeeper lights the shopkeeper, at a barrel the barrel, and at the road
    /// beside them the road.
    #[default]
    Auto,
    /// Creatures and items only. The ground stays unmarked even where there is
    /// neither — for looking at what is *there* on a crowded floor without the
    /// diamond under it moving as well.
    Items,
    /// Tiles only. An item or a creature under the cursor is drawn like any
    /// other, which is what walking somewhere across a littered street wants.
    Tiles,
}

/// How an item or a creature says it is the one under the cursor.
///
/// Both were designed to compose — they are two passes over different pixels,
/// see `docs/outline.md` — and for a while both were simply drawn. This is the
/// switch that decision was deferred behind.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum HighlightStyle {
    /// The art redrawn in [`items::HIGHLIGHT_HUE`](openshard_client_render::items::HIGHLIGHT_HUE),
    /// which is what the reference client does and all it does.
    Hue,
    /// A ring round the silhouette and a glow behind it, leaving the art its own
    /// colours. The default, because it *adds* a statement rather than
    /// replacing the picture with one.
    #[default]
    Outline,
    /// Both at once.
    Both,
}

impl HighlightStyle {
    /// Whether the item's own art is replaced by the highlight ramp.
    pub fn hues(self) -> bool {
        matches!(self, Self::Hue | Self::Both)
    }

    /// Whether a silhouette is drawn for it to be ringed and lit from.
    pub fn rings(self) -> bool {
        matches!(self, Self::Outline | Self::Both)
    }
}

/// What the script picker asked for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ScriptRequest {
    /// Walk this scenario from its start.
    Run(&'static str),
    /// Stop wherever it got to.
    Stop,
}

/// egui, and the two crates that put it on a window and on a GPU.
pub struct Shell {
    context: egui::Context,
    state: egui_winit::State,
    renderer: egui_wgpu::Renderer,
    /// Where the world may be drawn: what [`egui::CentralPanel`] left free,
    /// converted to physical pixels. Held between frames because the camera is
    /// resized from it before the next frame's UI has run.
    viewport: ViewportRect,
    /// What the last [`Shell::run`] asked to be woken after.
    repaint_after: std::time::Duration,
    /// What is in the chat line and not yet said. Lives here rather than in the
    /// app for the reason [`Windows`](crate::gump::Windows) does: it is what a
    /// widget is holding between frames, and nothing outside the UI reads it.
    typed: String,
    /// The state of the open dialogs — which page, which switches.
    gumps: crate::gump::Windows,
    /// What the HUD remembers between runs: the tab in front, where the dev
    /// window sits, whether it is open, and the scale.
    ///
    /// Lives here for the same reason `typed` and `gumps` do — it is what the UI
    /// is holding between frames — and is read back out by [`Shell::desk`] when
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
        // On top of the monitor's own density, which `egui_winit::State` is given
        // below and which nothing here saves.
        context.set_zoom_factor(desk.zoom.raw());
        let state = egui_winit::State::new(
            context.clone(),
            egui::ViewportId::ROOT,
            window,
            Some(window.scale_factor() as f32),
            None,
            Some(device.limits().max_texture_dimension_2d as usize),
        );
        let renderer = egui_wgpu::Renderer::new(
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
            state,
            renderer,
            viewport: ViewportRect {
                x: 0,
                y: 0,
                width: size.width.max(1),
                height: size.height.max(1),
            },
            // Until the first frame has run there is nothing to wait for; the
            // animation clock is what wakes the loop.
            repaint_after: std::time::Duration::MAX,
            typed: String::new(),
            gumps: crate::gump::Windows::default(),
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

    /// Offer an event to the UI, answering whether it took it.
    ///
    /// A `true` here means the camera and the walk keys must not see the event.
    pub fn on_window_event(&mut self, window: &Window, event: &winit::event::WindowEvent) -> bool {
        self.state.on_window_event(window, event).consumed
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
    pub fn run(&mut self, window: &Window, hud: &Hud, hues: &Hues) -> (Request, egui::FullOutput) {
        let input = self.state.take_egui_input(window);
        let mut request = Request::default();
        // What the panels leave behind, taken from the root `Ui` *after* they
        // have claimed their edges. That rectangle is the world's viewport, so
        // a docked panel shrinks the world and a floating window sits over it.
        let mut free = egui::Rect::from_min_size(egui::Pos2::ZERO, self.context.content_rect().size());
        let typed = &mut self.typed;
        let gumps = &mut self.gumps;
        let desk = &mut self.desk;
        let output = self.context.run_ui(input, |ui| {
            request = layout(ui, hud, typed, gumps, hues, desk);
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
        (request, output)
    }

    /// Draw what [`Shell::run`] produced, over whatever is already on the
    /// surface.
    #[allow(clippy::too_many_arguments)]
    /// The dialog state the *art* layer needs — see
    /// [`crate::gump::Windows::state`]. Here rather than on `App` because a
    /// page a button flipped to is the UI's own answer and this is the UI.
    pub fn gumps(&self) -> &crate::gump::Windows {
        &self.gumps
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
        let pixels_per_point = self.context.pixels_per_point();
        let jobs = self.context.tessellate(output.shapes, pixels_per_point);
        for (id, delta) in &output.textures_delta.set {
            self.renderer.update_texture(device, queue, *id, delta);
        }
        let descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels,
            pixels_per_point,
        };
        self.renderer
            .update_buffers(device, queue, encoder, &jobs, &descriptor);

        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("egui"),
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
        self.renderer
            .render(&mut pass.forget_lifetime(), &jobs, &descriptor);

        for id in &output.textures_delta.free {
            self.renderer.free_texture(id);
        }
    }
}

/// The panels, the speech line, and the server's own dialogs.
///
/// Deliberately absent: the paperdoll, containers, and a journal worth the name.
/// Those are M4 — see `docs/client.md` — and building them here would decide M4
/// without arguing it. The speech line is not one of them: it is the only way to
/// reach a shard's staff commands, which are `.`-prefixed *speech*, and a client
/// that cannot say `.admin` cannot open the menu the server already draws.
fn layout(
    root: &mut egui::Ui,
    hud: &Hud,
    typed: &mut String,
    gumps: &mut crate::gump::Windows,
    hues: &Hues,
    desk: &mut Desk,
) -> Request {
    let mut request = Request::default();
    // egui 0.35 hands the frame a root `Ui`: panels are shown inside it and
    // what is left of it is the world's viewport, while windows float over the
    // context. The two are laid out here in that order for exactly that reason.
    let context = root.ctx().clone();

    egui::Panel::top("status").show(root, |ui| {
        ui.horizontal(|ui| {
            ui.label(&hud.connection);
            ui.separator();
            match hud.serial {
                Some(serial) => ui.label(format!("serial 0x{serial:08X}")),
                None => ui.label("no serial"),
            };
            ui.separator();
            ui.label(format!(
                "{}, {}, {}",
                hud.position.x, hud.position.y, hud.position.z
            ));
            ui.separator();
            // What the frame cost to *build*, and not how long it took: paced by
            // the display, every frame takes a refresh interval whatever it was
            // doing, and the strip would read 16.7ms on an idle client for ever.
            // Milliseconds with one decimal, because a frame is a millisecond or
            // two here and an integer would read as zero.
            ui.label(format!(
                "{:.1} ms",
                hud.frames
                    .last()
                    .map_or(0.0, |frame| frame.build().as_secs_f64() * 1_000.0)
            ));
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
            // The HUD's scale, shown because it is remembered: a client that
            // reopened at yesterday's zoom and does not say so reads as a client
            // that is rendering at the wrong size. Ctrl+`+` / Ctrl+`-` /
            // Ctrl+`0` are egui's own — see `Options::zoom_with_keyboard` — and
            // this is the readout, not the control.
            ui.label(format!("{}%", (ui.ctx().zoom_factor() * 100.0).round()));
        });
    });

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
                Tab::Camera => camera_panel(ui, hud, &mut request),
                Tab::Rig => rig_panel(ui, hud, &mut request),
                Tab::Frames => frames_panel(ui, hud),
                Tab::World => world_panel(ui, hud),
                Tab::Tile => tile_tab(ui, hud, &mut request),
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

    request.say = speech_line(root, hud, typed);

    overlays(root, hud);

    // Over everything, and last: a dialog the shard opened is the one thing on
    // screen that is waiting for an answer.
    request.gump = gumps.show(&context, &hud.gumps, hues);

    request
}

/// Where the eye is, what it is looking at, and whether it is following.
fn camera_panel(ui: &mut egui::Ui, hud: &Hud, request: &mut Request) {
    let eye = hud.camera.eye();
    egui::Grid::new("camera").num_columns(2).show(ui, |ui| {
        ui.label("zoom");
        ui.label(hud.camera.zoom().to_string());
        ui.end_row();
        ui.label("eye");
        ui.label(format!("{}, {} px", eye.x, eye.y));
        ui.end_row();
        ui.label("tile");
        let (x, y) = hud.camera.eye_tile();
        ui.label(format!("{x}, {y}"));
        ui.end_row();
        ui.label("viewport");
        ui.label(format!("{}x{}", hud.camera.width, hud.camera.height));
        ui.end_row();
        ui.label("drawn");
        // The offscreen image, which is the viewport only at zoom 1 and
        // is what the GPU's texture limit applies to.
        ui.label(format!(
            "{}x{}",
            hud.camera.render_width(),
            hud.camera.render_height()
        ));
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
}

/// What the view has decoded, with the serials the renderer drops.
fn world_panel(ui: &mut egui::Ui, hud: &Hud) {
    ui.label(format!(
        "{} mobiles, {} ground items",
        hud.mobiles.len(),
        hud.items.len()
    ));
    for (serial, body, at) in &hud.mobiles {
        ui.label(format!(
            "0x{serial:08X}  body {body}  {}, {}, {}",
            at.x, at.y, at.z
        ));
    }
    if !hud.items.is_empty() {
        ui.separator();
    }
    for (serial, graphic, at) in &hud.items {
        ui.label(format!(
            "0x{serial:08X}  item {graphic}  {}, {}, {}",
            at.x, at.y, at.z
        ));
    }
}

/// The overlays over the ground, what the cursor is on, and what a click holds.
///
/// Named for the tab and not for [`tile_panel`], which is the readout of *one*
/// tile that this calls twice — for what is hovered and for what is selected.
fn tile_tab(ui: &mut egui::Ui, hud: &Hud, request: &mut Request) {
    let mut show = hud.show_terrain;
    if ui
        .checkbox(&mut show, "terrain — walkable green, blocked red, route orange")
        .changed()
    {
        request.show_terrain = Some(show);
    }
    match &hud.terrain {
        Some(terrain) => {
            ui.label(format!(
                "{} open, {} blocked, route {} steps",
                terrain.open.len(),
                terrain.blocked.len(),
                terrain.route.len().saturating_sub(1),
            ));
        }
        // The counts are the overlay's own companion: an empty picture
        // is a client that found nothing and a client that asked
        // nothing, and those look identical on the ground.
        None => {
            ui.label("off");
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
            for (_, _, solid) in solids_of(occluders) {
                total += 1;
                if hud.solid_cut.shows(solid) {
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
            (Cut::BelowFeet(hud.position.z), "above your feet"),
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
        match hud.lit_mobile {
            Some(index) => index.to_string(),
            None => "—".to_string(),
        },
        match hud.lit_item {
            Some(index) => index.to_string(),
            None => "—".to_string(),
        },
        // The graphic and the tile it stands on, and *that tile* is the point of
        // printing it: it is the one a click will hold, and it is not the tile
        // under the cursor — a wall's picture stands up the screen from the cell
        // it is built on. The hover readout below names the other one, so the two
        // rows together are the whole of why they differ.
        match &hud.lit_static {
            Some(picked) => format!(
                "0x{:04X} at {}, {}, {}",
                picked.graphic.0, picked.at.x, picked.at.y, picked.at.z
            ),
            None => "—".to_string(),
        },
    ));
    // The held tile first and the live one under it. The hover readout changes
    // on every mouse move — a tile with six statics is several rows taller than
    // an empty one — and whatever is drawn below it is moved by that. Above, the
    // selection is the one thing on this tab that only changes when the player
    // clicks, so it stays under the cursor long enough to be read and copied.
    ui.label(
        "selected — glows cyan; a click on a wall holds the wall's own tile, not the ground under the cursor",
    );
    // What the same click is holding of the map itself, above the tile's own
    // rows: a click on a wall names both, and the wall is the thing that was
    // pointed at while the tile is where the ground under the cursor was.
    match &hud.selected_static {
        Some(picked) => {
            ui.label(format!(
                "static 0x{:04X} at {}, {}, {} — washed with its ground",
                picked.graphic.0, picked.at.x, picked.at.y, picked.at.z,
            ));
        }
        None => {
            ui.label("no static held — click a wall to wash it and the tile under it");
        }
    }
    tile_panel(ui, "selected", hud.selected.as_ref());
    ui.separator();
    ui.label("hover — the ground under the cursor; marked yellow only while nothing is standing on it");
    tile_panel(ui, "hover", hud.hover.as_ref());
}

/// The three things drawn *on* the world rather than beside it: the terrain
/// wash, the occluder boxes, and the tile markers.
///
/// Taken out of [`layout`] with the rest of the panels' bodies, and for the same
/// reason — what is left in `layout` is then the arrangement, one screenful of
/// it, and nothing else.
fn overlays(root: &mut egui::Ui, hud: &Hud) {
    // Every panel has claimed its edge by now, so what is left of the root `Ui`
    // is the world's own rectangle — the very rect `Shell::run` reads back a
    // moment later and hands the camera. Read *here*, at the foot of the layout
    // and not in the middle of it: taken before the speech strip took its edge,
    // this was a rectangle the world is not drawn in, and the markers clipped to
    // it were painted over the strip. Windows do not narrow it and must not: they
    // float over the world, and a marker under one is correctly hidden by it
    // rather than clipped away.
    let viewport = root.available_rect_before_wrap();
    let world = world_painter(root, viewport);
    // The terrain map goes down first: it is a wash over the ground, and the
    // three markers below are read against it.
    if let Some(terrain) = &hud.terrain {
        draw_terrain(&world, &hud.camera, terrain, viewport.min);
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
        draw_occluders(&world, &hud.camera, occluders, hud.solid_cut, viewport.min);
    }
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
        for tile in &hud.neighbours {
            draw_tile_highlight(
                &world,
                &hud.camera,
                tile,
                viewport.min,
                egui::Color32::TRANSPARENT,
                egui::Stroke::new(1.2, egui::Color32::from_rgba_unmultiplied(255, 255, 0, 170)),
            );
        }
    }
    if let Some(tile) = hud.hover.as_ref().filter(|_| hud.hover_lit) {
        draw_tile_highlight(
            &world,
            &hud.camera,
            tile,
            viewport.min,
            egui::Color32::from_rgba_unmultiplied(255, 255, 0, 40),
            egui::Stroke::new(1.5, egui::Color32::from_rgba_unmultiplied(255, 255, 0, 180)),
        );
    }
    if let Some(tile) = &hud.selected {
        draw_tile_highlight(
            &world,
            &hud.camera,
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
            &hud.camera,
            tile,
            viewport.min,
            egui::Color32::from_rgba_unmultiplied(0, 255, 120, 50),
            egui::Stroke::new(2.0, egui::Color32::from_rgb(0, 255, 120)),
        );
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
fn rig_panel(ui: &mut egui::Ui, hud: &Hud, request: &mut Request) {
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
        let mut ease = hud.ease;
        ui.add(egui::Slider::new(&mut ease.tau, 0.0..=0.5).suffix(" s"));
        if ease != hud.ease {
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
        let mut span = hud.scope_span.as_secs_f32();
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
    match hud.metrics {
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

    let span = hud.scope_span.as_secs_f32().max(0.001);
    let last = hud
        .readings
        .last()
        .map_or(0.0, |reading| reading.at.as_secs_f32());
    let series = |of: fn(&Reading) -> Option<f64>| -> Vec<(f32, f32)> {
        hud.readings
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
            ui.horizontal_wrapped(|ui| {
                for name in &hud.scripts {
                    if ui.add_enabled(hud.offline, egui::Button::new(*name)).clicked() {
                        request.script = Some(ScriptRequest::Run(name));
                    }
                }
            });
            if !hud.offline {
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
    let last = hud.frames.last();
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
        match hud.worst_fps {
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
        // The vsync sleep, named as such: it is the slack in the frame and not
        // work, and a client whose wait is most of the interval has room.
        ui.label("waited");
        ui.label(last.map_or("—".to_string(), |frame| format!("{:.1} ms", ms(frame.wait))));
        ui.end_row();
    });
    // The counter `docs/camera.md` asks for: without it, a full atlas repack
    // is indistinguishable from an ordinary heavy frame, both being a large
    // number in `world` above. `repacked` marks which frame in the window
    // paid for one; the total survives past that window.
    if last.is_some_and(|frame| frame.repacked) {
        ui.label(egui::RichText::new("this frame repacked the atlas").color(egui::Color32::YELLOW));
    }
    if hud.repacks > 0 {
        ui.label(
            egui::RichText::new(format!("atlas repacks this session: {}", hud.repacks))
                .weak()
                .small(),
        );
    }
    // The sentence that turns "the frame rate dropped" from a bug report into a
    // reading. What is asking for frames is the whole answer, and when it is the
    // animation clock that is a rule rather than a symptom — see `App::pacing`.
    ui.label(
        egui::RichText::new(match hud.pacing {
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

    let span = hud.frames_span.as_secs_f32().max(0.001);
    let end = hud.frames.last().map_or(0.0, |frame| frame.at.as_secs_f32());
    let series = |of: fn(&crate::frames::Frame) -> f64| -> Vec<(f32, f32)> {
        hud.frames
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
        ],
        span,
    );
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

/// The speech line, docked at the bottom, with what the shard last said above
/// it.
///
/// Answers with a line to say, once, on the frame Enter was pressed.
///
/// # Why the field is refocused by hand
///
/// egui drops focus on Enter, which is right for a form and wrong for a chat
/// box: a player says two things in a row. So the field asks for focus back on
/// the same frame it loses it — which also means the walk keys stay out of the
/// way while typing, since a focused text field consumes them (see
/// [`Shell::on_window_event`], and `App::window_event`, which lets go of every
/// held direction when the UI takes a key).
fn speech_line(root: &mut egui::Ui, hud: &Hud, typed: &mut String) -> Option<String> {
    let mut said = None;
    egui::Panel::bottom("speech").show(root, |ui| {
        // What the shard has said lately, newest last, so the eye ends up beside
        // the line it is about to type into.
        for line in &hud.said {
            ui.label(egui::RichText::new(line).weak());
        }
        ui.horizontal(|ui| {
            ui.label("say");
            let field = ui.add(
                egui::TextEdit::singleline(typed)
                    .desired_width(f32::INFINITY)
                    .hint_text("type, and Enter to speak — a shard's staff commands start with '.'"),
            );
            let entered = field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            if entered {
                let line = std::mem::take(typed);
                // An empty line is a stray Enter, not silence worth sending:
                // the server would draw an empty message over the player's head.
                if !line.trim().is_empty() {
                    said = Some(line);
                }
                field.request_focus();
            }
        });
    });
    said
}

/// One tile's numbers, each beside a button that puts it on the clipboard —
/// the whole point of holding a selection still is being able to paste one of
/// these into a bug report.
///
/// A fixed-height box, scrolled inside, and that is the point of it: a tile's
/// readout is as many rows as it has statics, so a panel sized to its content
/// changes height under the cursor and moves everything below it — including
/// the other tile panel — while it is being read. The height is spent whether
/// or not there is a tile to put in it; `id` is what keeps the two boxes'
/// scroll offsets apart, the same way the tabs' own salt does.
fn tile_panel(ui: &mut egui::Ui, id: &str, tile: Option<&PickedTile>) {
    /// Four rows and a little: the header, the levels, the land, and one static
    /// — past that the box scrolls rather than grows.
    const HEIGHT: f32 = 108.0;
    egui::ScrollArea::vertical()
        .id_salt(id)
        .max_height(HEIGHT)
        .auto_shrink([false; 2])
        .show(ui, |ui| tile_rows(ui, tile));
}

/// The rows themselves, inside the box [`tile_panel`] fixes the height of.
fn tile_rows(ui: &mut egui::Ui, tile: Option<&PickedTile>) {
    let Some(tile) = tile else {
        ui.label("(none)");
        return;
    };
    ui.horizontal(|ui| {
        // Both heights, because the gap between them is the thing worth seeing:
        // on a pier the land is water far below the deck a body stands on, and
        // every marker on this tile is drawn at the second one.
        ui.label(format!("tile {}, {}   stand z {}", tile.x, tile.y, tile.stand_z));
        // The whole tile in one press. The per-graphic buttons below copy a
        // number to paste into a lookup; this copies what a bug report wants,
        // which is the column and everything standing in it.
        if ui.small_button("copy all").clicked() {
            ui.ctx().copy_text(tile_text(tile));
        }
    });
    // The column in words, in the same green and red the box is drawn in: the
    // picture says *where* the levels are and this says which, so a level hidden
    // behind a wall on screen is still countable here.
    ui.horizontal_wrapped(|ui| {
        ui.label("levels");
        for &(z, standable) in &tile.levels {
            let colour = match standable {
                true => STANDABLE,
                false => BLOCKED,
            };
            ui.colored_label(colour, format!("{z}"));
        }
        if let Some(ceiling) = tile.ceiling {
            ui.label(format!("· ceiling {ceiling}"));
        }
    });
    ui.horizontal(|ui| match tile.land {
        Some(graphic) => {
            ui.label(format!("land {graphic} (0x{graphic:04X})  z {}", tile.land_z));
            if ui.small_button("copy").clicked() {
                ui.ctx().copy_text(graphic.to_string());
            }
        }
        None => {
            ui.label("land: block not loaded");
        }
    });
    for (graphic, z, hue) in &tile.statics {
        ui.horizontal(|ui| {
            ui.label(format!("static {graphic} (0x{graphic:04X})  z {z}  hue {hue}"));
            if ui.small_button("copy").clicked() {
                ui.ctx().copy_text(graphic.to_string());
            }
        });
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
        "tile {}, {}  stand z {}  land z {}\n",
        tile.x, tile.y, tile.stand_z, tile.land_z
    );
    text.push_str("levels");
    // The panel says "a body does not fit here" in red, and red does not
    // survive a paste; `!` after the height is the same fact in text.
    for &(z, standable) in &tile.levels {
        let verdict = match standable {
            true => "",
            false => "!",
        };
        // `unwrap` is not needed and `?` cannot happen: writing into a `String`
        // is infallible, which is why the result is dropped here.
        let _ = write!(text, " {z}{verdict}");
    }
    if let Some(ceiling) = tile.ceiling {
        let _ = write!(text, " · ceiling {ceiling}");
    }
    text.push('\n');
    match tile.land {
        Some(graphic) => {
            let _ = writeln!(text, "land {graphic} (0x{graphic:04X})  z {}", tile.land_z);
        }
        None => text.push_str("land: block not loaded\n"),
    }
    for &(graphic, z, hue) in &tile.statics {
        let _ = writeln!(text, "static {graphic} (0x{graphic:04X})  z {z}  hue {hue}");
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
fn world_painter(ui: &egui::Ui, viewport: egui::Rect) -> egui::Painter {
    ui.ctx()
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
/// A level a body does not fit on, drawn red — the same pair the terrain
/// overlay washes tiles with, so one vocabulary answers "can I stand there"
/// wherever the question is asked.
const BLOCKED: egui::Color32 = egui::Color32::from_rgb(255, 40, 40);

/// The same colour at a chosen alpha — a wash and an outline of one hue.
fn washed(colour: egui::Color32, alpha: u8) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(colour.r(), colour.g(), colour.b(), alpha)
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
    let at = |z: i8, corners: [i8; 4]| {
        facet_corners(
            painter,
            camera,
            openshard_protocol::world::Point {
                x: tile.x,
                y: tile.y,
                z,
            },
            corners,
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
    let base = at(0, [0; 4]);
    // A tile whose whole column lies in the datum plane has no box, and the loop
    // below would draw four zero-length segments over the diamond's own edges.
    if lid.is_some() || tile.stand_z != 0 || tile.corners != [0; 4] {
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
            painter.add(egui::Shape::closed_line(top.clone(), edge));
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

/// The walkability wash and the route over it.
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
    for (tiles, fill) in [(&terrain.open, open), (&terrain.blocked, blocked)] {
        for &point in tiles {
            let corners = tile_corners(painter, camera, point, viewport_origin);
            painter.add(egui::Shape::convex_polygon(corners, fill, egui::Stroke::NONE));
        }
    }
    // The route last, over its own ground: a line through the tile centres, and
    // a dot on each step so a diagonal can be told from a pair of orthogonals.
    let centres: Vec<egui::Pos2> = terrain
        .route
        .iter()
        .map(|&point| tile_centre(painter, camera, point, viewport_origin))
        .collect();
    if centres.len() > 1 {
        painter.add(egui::Shape::line(
            centres.clone(),
            egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 160, 0)),
        ));
    }
    for centre in centres {
        painter.circle_filled(centre, 2.5, egui::Color32::from_rgb(255, 200, 80));
    }
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
fn solids_of(
    occluders: &openshard_client_render::occlusion::Occlusion,
) -> impl Iterator<Item = (i32, i32, &openshard_client_render::occlusion::Solid)> + '_ {
    let bounds = occluders.bounds();
    (bounds.min_y..=bounds.max_y).flat_map(move |y| {
        (bounds.min_x..=bounds.max_x)
            .flat_map(move |x| occluders.solids_at(x, y).map(move |solid| (x, y, solid)))
    })
}

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
    occluders: &openshard_client_render::occlusion::Occlusion,
    cut: Cut,
    viewport_origin: egui::Pos2,
) {
    use openshard_client_render::occlusion::{EDGE_ANY, EDGE_EAST, EDGE_NORTH, EDGE_SOUTH};
    use openshard_client_render::solid::Side;

    let clip = painter.clip_rect();
    // Back to front. Collected rather than drawn as they come, because a solid
    // needs an order and `surfaces_of` walks the grid in rows: a row of wall
    // drawn left to right paints its near end behind its far one.
    let mut standing: Vec<(i32, i32, &openshard_client_render::occlusion::Solid)> =
        solids_of(occluders).filter(|(_, _, s)| cut.shows(s)).collect();
    standing.sort_by_key(|(x, y, solid)| (x + y, solid.bottom(), solid.top()));

    for (x, y, solid) in standing {
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
            0 => face(high.clone(), 1.0),
            // A **body** — a tree, a post, a graphic whose art names no edge. A
            // solid the ray travels through, so it is a box; and only the three
            // faces a camera can see are drawn, which is what makes it read as a
            // box rather than as a tangle. `Face::outward` names the two: an
            // isometric camera sees `+x` and `+y`.
            EDGE_ANY => {
                face(wall_of(panel_edge(EDGE_SOUTH)), Side::SOUTH_SHADE);
                face(wall_of(panel_edge(EDGE_EAST)), Side::EAST_SHADE);
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
                    EDGE_EAST => Side::EAST_SHADE,
                    EDGE_SOUTH => Side::SOUTH_SHADE,
                    EDGE_NORTH => 0.42,
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
fn panel_edge(named: u8) -> [usize; 2] {
    use openshard_client_render::occlusion::{EDGE_EAST, EDGE_NORTH, EDGE_SOUTH};

    match named {
        EDGE_NORTH => [0, 1],
        EDGE_EAST => [1, 2],
        EDGE_SOUTH => [3, 2],
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
        use openshard_client_render::occlusion::{EDGE_EAST, EDGE_NORTH, EDGE_SOUTH, EDGE_WEST};

        // `Camera::tile_facet`'s own order, as the corner offsets it means.
        const DIAMOND: [(f32, f32); 4] = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
        let corner = |at: (f32, f32)| {
            DIAMOND
                .iter()
                .position(|it| *it == at)
                .expect("a face's end is a corner of its tile")
        };

        for (face, named) in [
            (Face::North, EDGE_NORTH),
            (Face::East, EDGE_EAST),
            (Face::South, EDGE_SOUTH),
            (Face::West, EDGE_WEST),
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
}
