//! The client, as far as it goes: a window onto Britannia's ground.
//!
//! Run it against a real client install, which this repository never contains:
//!
//! ```sh
//! OPENSHARD_CLIENT="/path/to/Ultima Online Classic" cargo run -p openshard-client-app
//! ```
//!
//! # A library with a binary on top
//!
//! Everything here is [`run`], and `main.rs` is the environment read into its
//! two arguments. The split is the one `crates/server/server` already made and
//! for the same reason: something that wants a client should call one rather
//! than build one. Today that something is `crates/e2e/playground`, which starts
//! a shard and a window in one process — and which could not exist at all while
//! the client was a binary, because nothing can depend on a `main`.
//!
//! Arrow keys walk a tile at a time, and shift runs. A right click is a move
//! order — the body walks to that tile on its own, and holding the button steers
//! it to wherever the cursor is; taking hold of the arrows cancels it. The wheel
//! zooms about the cursor, a middle-drag pans, page up and down pan vertically,
//! `Home` puts the camera back on the body and locks it there, and escape closes
//! the window.
//!
//! # The panels
//!
//! egui, over the world: a status strip, a camera window and a list of what the
//! [`WorldView`] is holding. A *dev* HUD and not this client's interface —
//! whether that is egui or the `0xB0` gump layer is M4's decision and
//! `shell.rs` is careful not to take it. What the panels leave free is the
//! world's viewport, so docking one shrinks the world rather than covering it.
//!
//! # With a shard, and without one
//!
//! Given an account it logs in and draws what the server has shown it — the
//! character, everyone else on screen, and the ground under them:
//!
//! ```sh
//! OPENSHARD_CLIENT=… OPENSHARD_ACCOUNT=admin OPENSHARD_PASSWORD=… \
//!     cargo run -p openshard-client-app
//! ```
//!
//! Then the arrows are a `0x02` each and the camera follows the body the server
//! confirms, not the keyboard. Without an account it stays what it was: a
//! window onto the map's own ground and statics, with one placeholder body
//! standing wherever the camera looks. Both are worth having — the offline one
//! needs no shard to look at a hillside, and it is the only one that runs
//! against a facet nobody is serving.

use std::collections::HashSet;
use std::path::{
    Path,
    PathBuf,
};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

mod app;
mod audio;
mod chat;
mod clutter;
mod combat_log;
mod crowd;
mod desk;
mod diagnostics;
/// The walk, held against an oracle. Tests only — see its module docs.
#[cfg(test)]
mod dst;
mod editor_mode;
mod event_loop;
mod frame_geometry;
mod frames;
mod graphics;
mod hand;
mod input;
mod jank;
mod keyboard;
mod keys;
mod link;
mod movement_trace;
mod net_command;
mod own_windows;
mod panes;
mod picking;
mod picking_query;
mod ping;
mod presentation;
mod profile;
mod render_passes;
mod replay;
mod resources;
mod shell;
mod steer;
#[cfg(test)]
mod tests;
mod tooltips;
mod ui_command;
mod window;
mod windows;
mod world;

/// The camera this client opens with: the reference one, the eye on the body to
/// the pixel.
///
/// Which rig it *ships* is undecided and is decided on a bench rather than here
/// — `docs/camera.md` D9.
pub(crate) const STARTUP_RIG: Rig = Rig::HARD;

/// And how far the drawn body may lag the walk it is doing.
///
/// **Not a default in the sense D9 refuses.** D9 is about naming a camera before
/// one has won; this one was looked at — `dst::dump_the_ramp` is the table and
/// `docs/camera.md` C3 records the sitting. It is also not a camera: the eye is
/// still `HARD` above, and what eases is where the body is drawn (D10).
///
/// Here rather than in `crowd.rs` on purpose. [`Ease::WALK`] is a *setting* — a
/// number that was found to be right — and which setting a window opens with is
/// a decision about this binary. The two being one line apart in one file is how
/// a setting quietly becomes a default.
pub(crate) const STARTUP_EASE: crowd::Ease = crowd::Ease::WALK;

/// Read a `.env` from the working directory or an ancestor of it, if there is
/// one, so that the binaries' `env =` options have something to fall back to.
///
/// Call it before parsing a command line — that is the whole of the contract —
/// and from every binary that puts a window on a client install, which is why
/// it lives here rather than in one of them: `crates/e2e/playground` starts the
/// same client from the same `.env`.
///
/// **A missing file is not a failure and a malformed one is.** The two are one
/// `Result` in `dotenvy` and collapsing them with `.ok()` is how a quoting
/// mistake becomes "set OPENSHARD_CLIENT" from a shell where it *is* set: a
/// path with a space in it needs quotes, and without them the whole file is
/// dropped without a word. The line is printed rather than returned, because
/// the caller has not built anything to fail out of yet.
pub fn load_env() {
    match dotenvy::dotenv() {
        Ok(_) => {}
        Err(error) if error.not_found() => {}
        Err(error) => eprintln!("ignoring .env: {error}"),
    }
}

/// Begin writing jank frames to `path` for this process.
///
/// The playground calls this before it opens a window, giving every manual
/// performance run one log that can be inspected after the process exits.
pub fn start_jank_log(path: &Path) -> std::io::Result<()> {
    jank::start_log(path)
}

use app::App;
use chat::Chat;
use crowd::Crowd;
use openshard_client_net::session::Plan;
use openshard_client_net::transport::Dial;
use openshard_client_render::animate::StaticAnimations;
use openshard_client_render::atlas::FontAtlas;
use openshard_client_render::bench::{
    self,
    Scope,
};
use openshard_client_render::camera::Camera;
use openshard_client_render::control::Control;
use openshard_client_render::debug::View;
use openshard_client_render::follow::{
    Gaze,
    Rig,
};
use openshard_client_render::frame::{
    self,
};
use openshard_client_render::gump::{
    GumpAtlas,
    GumpPixel,
};
use openshard_client_render::hue::HueRamp;
use openshard_client_render::mobiles::Mobile;
use openshard_client_render::occlusion;
use openshard_client_render::sprite::SpriteQuad;
use openshard_client_render::text::{
    self,
    GumpLabel,
};
use openshard_map::grid::Tile;
use openshard_movement::Leeway;
use openshard_protocol::direction::{
    Direction,
    Facing,
};
#[cfg(test)]
use openshard_protocol::serial::Serial;
use openshard_protocol::version::ClientVersion;
use openshard_protocol::wire::{
    Graphic,
    Hue,
};
use openshard_protocol::world::Point;
use openshard_uofiles::anim::{
    Anim,
    BodyDef,
};
use openshard_uofiles::animdata::AnimData;
use openshard_uofiles::art::Art;
use openshard_uofiles::cliloc::Cliloc;
use openshard_uofiles::equipconv::EquipConv;
use openshard_uofiles::font::AsciiFonts;
use openshard_uofiles::gumpart::Gumps;
use openshard_uofiles::hues::Hues;
use openshard_uofiles::radarcol::RadarColors;
use openshard_uofiles::skillgrp::SkillGroups;
use openshard_uofiles::skills::Skills as SkillNames;
use openshard_uofiles::texmaps::TexMaps;
use openshard_uofiles::ttf_font::TtfFont;
use winit::event_loop::{
    ControlFlow,
    EventLoop,
};

/// Where the camera starts: Britain, by the bank.
pub(crate) const START: Point = Point::new(1495, 1629, 0);

/// What a run opens on: the state a person would otherwise have to reach by
/// hand before the picture they came to take is on the screen.
///
/// Every field is a *diagnostic's* starting position and never a gameplay
/// setting — the plans in `docs/` name places and views ("the staircase at
/// 1493,1639, as solids"), and reaching one meant walking there and finding a
/// checkbox, which is two variables moved between the picture and the claim it
/// is about. Nothing here is remembered: this is where a window opens, not what
/// it is.
#[derive(Clone, Copy, Debug)]
pub struct Opening {
    /// The tile to open the camera on, if not [`START`]. See the field's use in
    /// [`run`] for what it does when there is a shard.
    pub at:              Option<Tile>,
    /// Whether the occlusion grid is drawn as solids from the first frame —
    /// `docs/lighting.md` step 23.0, F5 in the window, and the checkbox in the
    /// dev panel.
    pub solids:          bool,
    /// Pause the App update callback immediately after it enters the world.
    /// This is an opt-in diagnostic for exercising backpressure; it is never a
    /// gameplay setting and defaults to no pause.
    pub stall_on_update: Option<std::time::Duration>,
    /// An opt-in, deterministic presentation scenario.  It is a diagnostic
    /// input rather than a player command: the event loop drives it directly,
    /// so a renderer investigation does not depend on a desktop input injector.
    pub scenario:        Option<Scenario>,
}

/// Where this client's ground comes from.
///
/// Three arms, and deliberately not
/// [`openshard_movement::bake::WorldSource`]'s two: the third is a *client's*
/// answer and can be nobody else's. The shard reads the world it is about to
/// serve and each of the two bakes reads the world it is about to bake, so none
/// of them has a shard to ask — and an arm they can never take is an arm every
/// one of them would have to write a `match` for. What the two types share is
/// the arms that name a file, and [`WorldSource::on_disk`] is the seam.
///
/// The install and a base set are both *sources*, which is why this is not an
/// `Option<PathBuf>` with a flag beside it: E0 argued that for the first two and
/// the third does not change it — a world that arrives over the connection is a
/// third place to read one from, not the absence of the other two.
#[derive(Clone, Copy, Debug)]
pub enum WorldSource<'a> {
    /// The client install's own `map*` and `statics*`, as every run before base
    /// sets existed.
    Install,
    /// A base set of ours, and the append-only patch log beside it.
    BaseSet(&'a Path),
    /// The shard's own ground, fetched over the game connection after login.
    ///
    /// `docs/map/new_map_representation/to_the_client.md`'s E2. It needs a shard
    /// — an offline viewer with this arm has nobody to ask — and it is the one
    /// arm under which the window exists before the facet does. What that costs
    /// is stated once, on [`crate::resources::Resources::map`].
    Shard,
}

impl<'a> WorldSource<'a> {
    /// The file source this names, or `None` for the world that arrives over the
    /// connection.
    ///
    /// The conversion to the type the shard's boot and the two bakes share. It
    /// is the whole of what the two enums have to agree about, and a new file
    /// arm added to either is a compile error here rather than a client that
    /// quietly reads the install instead.
    #[must_use]
    pub const fn on_disk(self) -> Option<openshard_movement::bake::WorldSource<'a>> {
        match self {
            Self::Install => Some(openshard_movement::bake::WorldSource::Install),
            Self::BaseSet(path) => Some(openshard_movement::bake::WorldSource::BaseSet(path)),
            Self::Shard => None,
        }
    }
}

/// A repeatable presentation path injected into the client at startup.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scenario {
    /// Ask the live shard for its craft catalogue once the window opens. This
    /// is a presentation fixture for the egui capture tool, not a saved UI
    /// preference or a simulated catalogue.
    CraftCatalogue,
    /// Zoom out to the first composite tier and pan across many map blocks.
    LodSweep,
    /// Hold the maximum zoom while comparing the resident static-atlas texture
    /// against its CPU source; used to catch delayed atlas overwrites.
    AtlasSoak,
    /// Start at the default zoom, zoom directly out, then hold the camera still
    /// while late producer/cache work is audited.
    ZoomSoak,
    /// The same stationary zoom transition, but deliberately drops subsequent
    /// server world updates after counting them. This isolates packet-driven
    /// scene mutation from a texture/cache mutation.
    ZoomSoakFreezeServer,
    /// Use the real shard connection: zoom out, keep all incoming state and
    /// animation updates, and periodically compare the resulting frame with
    /// a direct LOD0 render. This is deliberately not an offline simulation.
    LiveOracle,
}

/// The facet to open. Felucca: `0x1B` carries the facet's *size* and not its
/// number, so a shard serving another one is noticed by the size test in
/// [`App::entered`] rather than followed.
pub(crate) const FACET: u8 = 0;

/// Which client this claims to be. Every `Feature` gate on the server follows
/// from it, and this is the one ClassicUO opens with — see `docs/client.md`.
pub(crate) const VERSION: ClientVersion = ClientVersion::new(7, 0, 45, 65);

/// How often to redraw while somebody is mid-step. See [`App::redraw_interval`].
///
/// Roughly a 60Hz display, and deliberately a number of our own rather than the
/// monitor's: nothing here knows the refresh rate, and asking the surface would
/// tie the animation to the present mode the adapter happened to offer.
pub(crate) const GLIDE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(16);

/// How close together two left clicks have to land to be a double-click.
///
/// ClassicUO's `Mouse.MOUSE_DELAY_DOUBLE_CLICK`
/// (`src/ClassicUO.Client/Input/Mouse.cs`), taken as it stands: 350ms is what
/// players' hands are used to on this game, and a client that picked its own
/// number would be one where doors sometimes do not open. Distance is
/// deliberately *not* part of the test — the reference does not check it
/// either, and a mouse that slips a pixel between two clicks has not stopped
/// double-clicking.
pub(crate) const DOUBLE_CLICK: std::time::Duration = std::time::Duration::from_millis(350);

/// How near the body the cursor may sit while the steering button is held
/// without asking for anything — in **world pixels**, measured from the body's
/// own drawn pixel by [`ask_between`].
///
/// A radius and not a rectangle, because the ask is a bearing and a bearing has
/// no preferred axis: a square dead zone would let the same distance count on
/// the diagonal and not on the cardinal.
///
/// What it buys is that the sector stops being noise: two pixels of hand
/// tremor over the character name no direction, but the arithmetic in
/// [`heading_between`] will happily resolve them to one of eight, and without
/// this every twitch of a mouse held still over the body re-rolls that choice
/// and the body wanders off at random. A turn would be a mild version of the
/// same nonsense — the sprite spinning under a still hand — so the innermost
/// ring asks for nothing at all rather than for a facing.
///
/// **Ten and not the 24 the geometry asks for**, and what used to be the trade
/// in that is now [`TURN_ZONE`]'s job. A step is 44 world pixels at its longest
/// (the on-screen cardinals — see [`on_screen`]) and the cursor may sit up to
/// 22.5° off the bearing of the direction that wins its sector, so a step from
/// distance `d` ends nearer the cursor than it began only for
/// `d > 22 / cos 22.5° ≈ 23.8` — pinned by
/// `a_step_stops_overshooting_further_out_than_the_dead_zone`. That whole band
/// is the turn ring now: inside it the body faces the cursor and covers no
/// ground, so the overshoot it names cannot happen, and this radius is left to
/// answer the one question it was ever good at — where the bearing stops being
/// the hand's own tremor.
///
/// Deliberately in world pixels rather than screen ones, so it stays the same
/// fraction of a tile at every zoom — the projection is what makes a step
/// overshoot, and the projection is what this is measured in.
pub(crate) const DEAD_ZONE: f64 = 10.0;

/// Where the cursor stops asking for a facing and starts asking for a walk:
/// inside this radius of the body (and outside [`DEAD_ZONE`]) a held right
/// button turns the character on the spot and sends it nowhere — see
/// [`steer::Ask`].
///
/// The ring is the classic client's, and it is the only way a mouse can say
/// "face that way" at all: every other ask a cursor makes also sets the body
/// walking, so a player who wants to face a door, or face whoever they are
/// speaking to, has nothing to ask with. ClassicUO does *not* have it —
/// `MoveCharacterByMouseInput` walks on any non-zero offset and its one radius,
/// `mouseRange >= 190`, chooses running rather than whether to move — so this is
/// the stock client's behaviour and not the reference's, and it is why the ring
/// is a zone in this file rather than a number copied out of `Constants.cs`.
///
/// The radius is the overshoot bound, which is what makes it a decision and not
/// a taste: `22 / cos 22.5° ≈ 23.8` world pixels is where a step *stops* ending
/// past the cursor it was asked for (see
/// `a_step_stops_overshooting_further_out_than_the_dead_zone`). Nearer than
/// that, walking is the wrong answer to the ask no matter how it is paced —
/// the body lands beyond the cursor and the next ask points back the way it
/// came. So the band where a step overshoots is exactly the band where the
/// body turns instead, and the two constants stop being a trade-off with a
/// hole between them.
pub(crate) const TURN_ZONE: f64 = 23.9;

/// How much of the event loop's recent past the frame panel keeps.
///
/// The same four seconds as the scope, and for the same reason: what is worth
/// looking at is the last few steps, not the session. Its own constant because
/// the two rings answer different questions and one of them is about to grow a
/// slider — see `docs/camera.md`.
pub(crate) const FRAMES_SPAN: std::time::Duration = std::time::Duration::from_secs(4);

/// How much of the eye's recent past the scope keeps.
///
/// Long enough to hold a whole reversal — a step is 400ms and `back_and_forth`
/// turns round every one of them — and short enough that the curve on screen is
/// the last thing that happened rather than a session's worth of ink. Four
/// seconds is ten steps at a walk.
pub(crate) const SCOPE_SPAN: std::time::Duration = std::time::Duration::from_secs(4);

/// How many of the journal's last lines are drawn above the speech line.
///
/// Small on purpose, the same way the egui strip this replaced was: the whole
/// journal is kept in the [`WorldView`], capped there at
/// [`openshard_client_net::view::JOURNAL_LINES`], and what is worth *showing*
/// every frame is enough to read the answer to what was just typed, not a
/// session's worth of scrollback — there is no scrollback UI yet to read more
/// of it with.
pub(crate) const CHAT_LINES: usize = 6;

/// The vertical step between one drawn line of chat text and the next, in
/// gump pixels.
///
/// A fixed approximation and not a per-glyph measurement: `fonts.mul` has no
/// exposed line-height and its faces run roughly 8 to 14 pixels tall, so 16
/// leaves a line of air above the tallest of them. `fonts.mul`'s own step, and
/// only its: a TrueType line is spaced by the size the player asked for — see
/// `chat::line_height` and `docs/text_sizes.md`.
pub(crate) const CHAT_LINE_HEIGHT: i32 = 16;

/// Where the speech line's own left edge sits, in gump pixels from the
/// surface's corners — the same margin on every side a status strip would
/// leave, so the line does not draw into the window's own frame.
pub(crate) const CHAT_MARGIN: i32 = 4;

/// [`text::collect_gump`]'s own quads, grown around each label's own top-left
/// corner by a fractional bitmap-font scale.
///
/// One label at a time and not the whole slice through `collect_gump` once,
/// because every glyph's own quad has to grow around *its* label's corner and
/// not a shared one — the same reason a batched call cannot tell two labels'
/// quads apart once they come back in one `Vec`.
///
/// `fonts.mul` bakes every glyph at whatever pixels the art shipped
/// (`openshard_uofiles::font`'s own doc), so there is no continuous size to
/// ask the atlas for the way [`TtfAtlas::pixel_height`] has. Growing the
/// finished quad instead is nearest-sampled the same way a camera zoom step
/// already grows a world sprite — [`text`]'s own module doc — which is what
/// keeps a bigger chat box in the classic client's own blocky style rather
/// than blurring it. Not applied to the TrueType path for the opposite
/// reason: [`text::collect_screen_ttf`]'s own doc is why an antialiased glyph
/// stretched by an integer factor with no filtering reads as exactly the
/// blockiness that function exists to avoid.
pub(crate) fn scaled_gump_quads(labels: &[GumpLabel<'_>], atlas: &FontAtlas, scale: f32) -> Vec<SpriteQuad> {
    let mut quads = Vec::new();
    for label in labels {
        for mut quad in text::collect_gump(std::slice::from_ref(label), atlas) {
            quad.rect.x = label.at.x as f32 + (quad.rect.x - label.at.x as f32) * scale;
            quad.rect.y = label.at.y as f32 + (quad.rect.y - label.at.y as f32) * scale;
            quad.rect.width *= scale;
            quad.rect.height *= scale;
            quads.push(quad);
        }
    }
    quads
}

/// Open a window on `dir`'s files, and log in to `shard` if one is given.
///
/// The four arguments are the whole of what this run was asked for: which client
/// install to read, **where the ground comes from**, whether there is a shard to
/// play, and which face draws overhead speech. Everything else — the facet, the
/// version claimed, where the camera starts — is a constant above, because none
/// of it is a decision a caller has ever needed to make differently.
///
/// `world` is the second of those: the install used to be the only possible
/// answer, then a base set of ours became the second, and the shard itself is
/// the third — see [`WorldSource`], which is where the three are argued. What a
/// base set replaces is `map0LegacyMUL.uop`, `staidx0.mul` and `statics0.mul`;
/// `dir` is still required under every arm, because the art, the hues, the
/// multis and — the one that decides what a tile *means* — `tiledata.mul` are
/// not in a base set, are not on the wire, and are not going to be.
///
/// **[`WorldSource::Shard`] is the arm that reorders this function.** Under the
/// other two the facet is read before the window exists and everything after it
/// — the span bake, the coarse graph, the interiors flood, the camera's opening
/// `z` — is built from a map that is already there. A world that arrives after
/// login cannot keep that order, so under this arm the client starts with a
/// [`Ground`](openshard_movement::ground::Ground) that has no base, and grows one
/// when [`link::Update::Ground`] arrives. The two artifacts are absent with it:
/// they are bakes of a world this client has no file for, exactly as they are
/// absent today for an install with nothing baked beside it.
///
/// A shard is a [`Dial`] and a [`Plan`] rather than an address and a plan: how
/// the connection is opened is the caller's, which is what lets
/// `crates/e2e/playground` hand over a shard in this same process. Nothing in
/// this crate knows what a socket is any more; `client/net` does not either.
///
/// The bundled Noto Sans face draws text through [`text::collect_ttf`] by
/// default; F1 can select the original bitmap faces instead. `ttf_font`, when
/// supplied, replaces that default with the named TrueType/OpenType face.
///
/// This is a `-> ExitCode` and not a `-> Result`, because every failure here is
/// terminal for a *window*: no client files, no window system, no GPU. There is
/// nothing a caller could do with a typed error except print it, and printing it
/// is what the reasons already do. [`StartupError`] is the exception that proves
/// it — the failures *after* a window exists are types, because that is where
/// the same failure means different things.
///
/// It must be called on the main thread: `winit` says so on macOS and iOS, and
/// the event loop it builds is what enforces it.
/// The default interface face: Noto Sans Regular, distributed under
/// Apache-2.0. The accompanying license is `assets/LICENSE-NotoSans.txt`.
const DEFAULT_TTF_FONT: &[u8] = include_bytes!("../assets/NotoSans-Regular.ttf");

pub fn run<D: Dial + Send + 'static>(
    dir: &Path,
    world: WorldSource<'_>,
    shard: Option<(D, Plan)>,
    ttf_font: Option<PathBuf>,
    opening: Opening,
) -> ExitCode {
    // Startup is deliberately synchronous: a usable first frame is better than
    // discovering a missing client file halfway through one.  Keep a small,
    // always-on progress trace here so a slow install can be identified from
    // the terminal instead of looking like a hung window.
    let startup = Instant::now();
    let mut stage = std::time::Duration::ZERO;
    let mut checkpoint = |phase: &str| {
        eprintln!(
            "startup +{:>7.3}s ({:>7.3}s): {phase}",
            startup.elapsed().as_secs_f64(),
            startup.elapsed().saturating_sub(stage).as_secs_f64(),
        );
        stage = startup.elapsed();
    };
    eprintln!("startup: loading client resources from {}", dir.display());
    let Opening {
        at,
        solids,
        stall_on_update,
        scenario,
    } = opening;
    // Reading the whole facet takes a moment and a few hundred megabytes. That
    // is the shape `uofiles` has today — see the backlog in docs/client.md — and
    // it is honest to do it up front rather than to stall on the first frame.
    //
    // Which files that reads is `world`'s: the install's own map and statics, or
    // a base set of ours and the patch log beside it. It goes through the same
    // resolution the shard's boot and the two bakes use, so a client and a shard
    // pointed at one base set cannot arrive at different revisions of it —
    // `docs/map/new_map_representation/to_the_client.md`'s E0.
    //
    // `None` is E2's arm and reads nothing: the ground is the shard's, and it
    // arrives on the connection opened further down. Everything below that used
    // to be written against a facet in hand is written against this `Option`,
    // and each of those places says what it does without one.
    let world = match world.on_disk() {
        Some(source) => {
            match openshard_movement::bake::FacetWorld::read(
                dir,
                source,
                openshard_protocol::world::Facet(FACET),
            ) {
                Ok(world) => Some(world),
                Err(error) => {
                    eprintln!("loading facet {FACET}: {error}");
                    return ExitCode::FAILURE;
                }
            }
        }
        None => {
            if shard.is_none() {
                eprintln!(
                    "the ground was asked for from the shard and no shard was dialled: \
                     a viewer has nobody to ask, so give it --base-set or the install's map files"
                );
                return ExitCode::FAILURE;
            }
            eprintln!("facet {FACET}: the ground comes from the shard");
            None
        }
    };
    if let Some(world) = &world {
        if world.patches != 0 {
            eprintln!(
                "facet {FACET}: {} patch(es) applied; it is at revision {}",
                world.patches,
                world.snapshot.revision().get()
            );
        }
    }
    checkpoint("facet loaded");
    let art = match Art::open(dir) {
        Ok(art) => art,
        Err(error) => {
            eprintln!("opening artLegacyMUL.uop: {error}");
            return ExitCode::FAILURE;
        }
    };
    checkpoint("art archive opened");
    // What was measured off that art before this run: which edge of its tile
    // each wall stands on, and the hole in each window.
    // `docs/lighting.md`'s decision 31: the measurement is a tool's, and this is
    // the client reading what it wrote.
    //
    // **A missing table is a log line and not a failure** (decision 31.6). The
    // atlas measures as it packs, exactly as it did before the tool existed, so
    // what is lost is a slow first frame after a scroll rather than a client that
    // will not start. Saying which of the reasons it was matters: "no file" is a
    // tool nobody ran, and "stale" is a tool somebody must run *again*, and those
    // want different things done about them.
    let surfaces = match openshard_client_artscan::load(dir) {
        Ok(table) => {
            eprintln!(
                "art table: {} of {} pictures read, {} with a window, {} written by hand",
                table.decided(),
                table.examined(),
                table.holed(),
                table.authored(),
            );
            Some(table)
        }
        Err(error) => {
            eprintln!("art table: measuring as we pack — {error}");
            None
        }
    };
    checkpoint("art table loaded");
    // The two files a slope needs: the square textures, and the table that says
    // which of them a land graphic uses.
    let texmaps = match TexMaps::open(dir) {
        Ok(texmaps) => texmaps,
        Err(error) => {
            eprintln!("opening texmaps.mul: {error}");
            return ExitCode::FAILURE;
        }
    };
    checkpoint("terrain textures opened");
    let tiledata = match openshard_uofiles::tiledata::load_tiles(dir.join("tiledata.mul")) {
        Ok(tiles) => tiles,
        Err(error) => {
            eprintln!("opening tiledata.mul: {error}");
            return ExitCode::FAILURE;
        }
    };
    checkpoint("tile data loaded");
    // Every multi the client knows: the houses and the ships. A house arrives as
    // *one* item whose graphic is `0x4000 | id`, and without this table that
    // graphic is looked up in the static art — where it is a perfectly valid id
    // for something else entirely, so a villa draws as an unrelated sprite with
    // no error anywhere. See `crate::net_command`'s expansion.
    //
    // A client that cannot read them draws no houses rather than failing to
    // start, which is the bargain `animdata` above already makes.
    let multis = match openshard_uofiles::multi::Multis::load(dir) {
        Ok(multis) => Some(std::sync::Arc::new(multis)),
        Err(error) => {
            eprintln!("opening the multis: {error} — houses will not draw");
            None
        }
    };
    checkpoint("multis loaded");
    // What the animated statics cycle through. Read here and folded into
    // `tile_animations` below, because it takes both files to know which
    // graphics animate: the flag is `tiledata.mul`'s and the cycle is this one's.
    // A client without the file animates nothing rather than failing to start —
    // see `AnimData::load`.
    let animdata = match AnimData::load(dir) {
        Ok(animdata) => animdata,
        Err(error) => {
            eprintln!("opening animdata.mul: {error}");
            return ExitCode::FAILURE;
        }
    };
    checkpoint("animation metadata loaded");
    let hues = match Hues::load(dir.join("hues.mul")) {
        Ok(hues) => hues,
        Err(error) => {
            eprintln!("opening hues.mul: {error}");
            return ExitCode::FAILURE;
        }
    };
    // Built once: `hues.mul` does not change while the camera walks, unlike
    // the sprite atlases it is bound alongside.
    let hue_ramp = HueRamp::build(&hues);
    checkpoint("hues loaded and ramp packed");
    // The table itself is not kept past this point. It used to be, so that
    // `gump.rs` could pick one solid `egui::Color32` per hue for a `{ text }`
    // element; a caption is drawn through the gump pass now and is tinted the
    // way every other picture is — by the ramp, on the GPU, per pixel.
    let fonts = match AsciiFonts::open(dir) {
        Ok(fonts) => fonts,
        Err(error) => {
            eprintln!("opening fonts.mul: {error}");
            return ExitCode::FAILURE;
        }
    };
    // Built once, the same as the hue ramp: `fonts.mul` is ten faces of 224
    // glyphs, all of them a few pixels, and there is no "visible set" of
    // characters the way there is a visible set of graphics — any speech line
    // can hold any of them.
    let font_atlas = match FontAtlas::build(&fonts) {
        Ok(atlas) => atlas,
        Err(error) => {
            eprintln!("packing fonts.mul: {error}");
            return ExitCode::FAILURE;
        }
    };
    checkpoint("fonts loaded and atlas packed");
    // The interface's own pictures. Absent is not fatal and not even unusual:
    // a client directory without `gumpartLegacyMUL.uop` is a map viewer, and
    // the windows a shard opens are worth losing before the world is. What is
    // lost is said once, here, rather than per window.
    let gumps = match Gumps::open(dir) {
        Ok(gumps) => Some(gumps),
        Err(error) => {
            eprintln!("opening gumpartLegacyMUL.uop: {error} — dialogs will draw no art");
            None
        }
    };
    // The dialog text nothing sent — a `Cliloc.enu`-numbered line, not a wire
    // one. Absent the same way `gumps` is: a shard's quest and craft windows
    // (and this one's `Element::Localized` lines generally) draw those rows
    // blank rather than the client refusing to start over a table that is
    // only ever asked for, never required.
    let cliloc = match Cliloc::load(dir.join("Cliloc.enu")) {
        Ok(cliloc) => Some(cliloc),
        Err(error) => {
            eprintln!("opening Cliloc.enu: {error} — dialogs will draw no cliloc text");
            None
        }
    };
    // The minimap's one colour per graphic. Absent the same way `gumps` and
    // `cliloc` are: a client directory without `radarcol.mul` still runs, it
    // simply has no terrain to draw when the minimap window opens.
    let radar_colors = match RadarColors::load(dir.join("radarcol.mul")) {
        Ok(colors) => Some(colors),
        Err(error) => {
            eprintln!("opening radarcol.mul: {error} — the minimap will draw no terrain");
            None
        }
    };
    checkpoint("interface resources opened");
    // Parsed once: an operator may replace the bundled, Apache-2.0 Noto Sans
    // face with a shard-specific file, but every ordinary run has legible
    // Unicode text without a launch flag.
    let ttf_font = match ttf_font {
        Some(path) => {
            match TtfFont::open(&path) {
                Ok(font) => Some(font),
                Err(error) => {
                    eprintln!("opening {}: {error}", path.display());
                    return ExitCode::FAILURE;
                }
            }
        }
        None => {
            Some(
                TtfFont::from_bytes(DEFAULT_TTF_FONT)
                    .expect("the checked-in Noto Sans face is a valid TrueType font"),
            )
        }
    };
    if let Some(world) = &world {
        eprintln!(
            "{} loaded: {}x{} tiles",
            world.snapshot.map().facet_name(),
            world.snapshot.map().width(),
            world.snapshot.map().height()
        );
    }

    // User events are wake-ups only. The shard thread puts updates in the
    // staged mailbox below, so a busy platform loop cannot accumulate a second
    // user event for every update it has not yet drained.
    let event_loop = match EventLoop::<()>::with_user_event().build() {
        Ok(event_loop) => event_loop,
        Err(error) => {
            eprintln!("no window system: {error}");
            return ExitCode::FAILURE;
        }
    };
    event_loop.set_control_flow(ControlFlow::Wait);
    checkpoint("window event loop created");

    let anim = match Anim::open(dir) {
        Ok(anim) => anim,
        Err(error) => {
            eprintln!("opening anim.idx and anim.mul: {error}");
            return ExitCode::FAILURE;
        }
    };

    let body_def = match BodyDef::open(dir) {
        Ok(definition) => definition,
        Err(error) => {
            eprintln!("opening Body.def: {error}");
            return ExitCode::FAILURE;
        }
    };

    // What a worn item draws as. Read alongside `anim`, which is what its
    // entries resolve into.
    let equip_conv = match EquipConv::load(dir.join("Equipconv.def")) {
        Ok(equip_conv) => equip_conv,
        Err(error) => {
            eprintln!("opening Equipconv.def: {error}");
            return ExitCode::FAILURE;
        }
    };

    // What the skills are called, and which heading each is filed under: the
    // skill window's two tables. Fatal like every other client file — a window
    // that drew rows with no names would be a list of numbers — and read once,
    // since together they are under a kilobyte.
    let skill_names = match SkillNames::open(dir) {
        Ok(names) => names,
        Err(error) => {
            eprintln!("opening Skills.idx and skills.mul: {error}");
            return ExitCode::FAILURE;
        }
    };
    let skill_groups = match SkillGroups::open(dir) {
        Ok(groups) => groups,
        Err(error) => {
            eprintln!("opening skillgrp.mul: {error}");
            return ExitCode::FAILURE;
        }
    };
    checkpoint("animation and skill resources opened");

    // Where the character stands at boot: the camera's tile, at the height the
    // ground there actually is.
    //
    // `at` overrides [`START`] and is for looking at a *place* — the plans in
    // `docs/` name coordinates ("the staircase at 1493,1639"), and until this
    // existed the only way to reach one was to walk there, which needs a shard
    // and puts a body in front of the thing being looked at. It moves the camera
    // and nothing else: logged in, the shard still says where the character is
    // and the eye returns to them the moment anything relocks it (Home).
    //
    // With no facet in hand there is no height to look up, and [`START`]'s own
    // `z` stands in — a placeholder nothing draws, because the arm with no facet
    // is also the arm with a shard, and the shard's `0x1B` says where the body
    // actually is before the first frame is gated in.
    let start_tile = at.unwrap_or(Tile::new(START.x, START.y));
    let start = Point::new(
        start_tile.x,
        start_tile.y,
        world
            .as_ref()
            .and_then(|world| world.snapshot.map().land(start_tile.x, start_tile.y))
            .map_or(START.z, |cell| cell.z),
    );

    // The connection, if this run was asked for one. Started before the window
    // exists: the login is several round trips. A connected window remains
    // blank until its first complete world view arrives, rather than showing
    // the offline placeholder at `START` while those round trips happen.
    //
    // Neither the map nor the tile definitions go with it: the shard thread is
    // transport, and the step it sends is predicted on this side, beside the
    // snapshot that owns the terrain.
    let tiledata = Arc::new(tiledata);
    let update_proxy = event_loop.create_proxy();
    let updates = link::Updates::new();
    let connected = shard.is_some();
    // What the connection is asked to bring back, and it is the same question
    // `world` above answered: a client holding a facet asks the shard for none
    // of it, and a client holding none asks for all of it. There is no third
    // answer, which is why this is derived here rather than passed in.
    //
    // Under the second one the connection is also told where this client keeps
    // the ground it is given — the working directory, which is where
    // `client_ui.ron` already lives and for that file's reason: per-checkout,
    // visible, and deleting it is how a person asks for the whole facet again.
    // See `openshard_client_net::cache`.
    let ground_source = match world {
        Some(_) => link::GroundSource::OwnDisk,
        None => {
            link::GroundSource::Fetched {
                cache: PathBuf::from("."),
            }
        }
    };
    // The mailbox and the wake-up, as one value: the shard thread takes a copy
    // and so does every worker this side starts later — the navigation bake over
    // a world that arrives on the connection is the first of those. See
    // [`app::Post`].
    let post = app::Post::new(updates.clone(), update_proxy);
    let shard = shard.map(|(dial, plan)| {
        eprintln!("logging in as {}", plan.account.0);
        let reports = post.clone();
        link::connect(dial, plan, VERSION, ground_source, move |update| {
            reports.publish(update);
        })
    });

    // The ground, and the two artifacts baked over it.
    //
    // All three at once, because all three are the same question — *which world
    // is this* — and under [`WorldSource::Shard`] all three answers are the same
    // one **at this point in the run**: there is no world yet, so there is
    // nothing to have baked anything beside.
    //
    // The coarse graph does not stay absent. A world off the wire is kept as a
    // base set of ours, and that file is a world a bake can be stamped against:
    // `App::take_up_navigation` looks for the artifact beside it and builds one
    // when it is not there. What is still missing under this arm is the
    // interiors flood, which is a diagnostic and has an off state.
    let facet = world
        .as_ref()
        .map_or(openshard_protocol::world::Facet(FACET), |world| {
            // The facet comes from the snapshot rather than from `FACET` again:
            // one answer per process is the whole point of the snapshot owning
            // it. With no snapshot there is only the constant.
            world.snapshot.facet()
        });
    // The two things about this world that outlive reading it: which file it is
    // (a rebake is stamped against that file, and can be asked for at any time)
    // and where a graph over it belongs. Both taken before the world moves into
    // the match below, and both `None` under `WorldSource::Shard` — where there
    // is no world yet and `Update::Ground` fills them in.
    let world_file = world.as_ref().and_then(|world| world.base_set.clone());
    let navigation_path = world.as_ref().map(|world| world.navigation_path(dir));
    let (ground, coarse, interiors) = match world {
        None => {
            eprintln!(
                "no interiors flood: it is a bake of a world this client has no file for yet. \
                 The navigation graph is taken up when the ground arrives"
            );
            // Where a body may stand cannot be baked over a facet nobody has
            // handed over yet. `Ground::set_base` is the seam it arrives
            // through, and it takes the bake in the same statement — see
            // `link::Update::Ground`.
            (
                openshard_movement::ground::Ground::new(None, &tiledata),
                None,
                None,
            )
        }
        Some(world) => {
            // Both stamps are taken here, before the snapshot moves into
            // `Ground`, and both are taken *from the world* rather than from
            // `dir`: a facet read from a base set is not derived from
            // `map0LegacyMUL.uop` any more, and that file still sits there with
            // its old mtime — so a stamp naming it would pass and hand this
            // client a graph of a world it has never seen.
            //
            // The artifacts go with the world for the same reason, which is why
            // the two paths below are `world.artifacts(dir)` and not `dir`.
            let artifacts = world.artifacts(dir).to_owned();
            let navigation_stamp = world.stamp(dir, facet);
            // Taken before the snapshot moves into `Ground` below, because the
            // path is the world's own: the directory it lives in and the name
            // that says which world it is a bake of.
            let navigation_path = world.navigation_path(dir);
            let interior_stamp = openshard_client_artscan::interiors::stamp_of(dir, &world, facet);
            // What a rebake command has to be told, so the two hints below name
            // the world this client is actually running on.
            let rebake_world = match &world.base_set {
                Some(base_set) => format!(" --base-set {:?}", base_set.display()),
                None => String::new(),
            };
            // Where a body may stand, baked before anything can ask: this end
            // predicts every step it draws, and since `navigation_spans.md`'s N3
            // a step reads this rather than re-deriving each column from
            // `tiledata`. One pass over the facet, and the one thing a
            // `MapTerrain` cannot be built without.
            //
            // The snapshot goes in here rather than at `Resources` because the
            // bake is taken with it: a `Ground` is the facet *and* the bake over
            // it, so there is no window in which this end holds one without the
            // other.
            let ground = openshard_movement::ground::Ground::new(Some(world.snapshot), &tiledata);
            let (width, height) = {
                let map = ground.snapshot().expect("it was given a facet a line ago");
                (map.map().width(), map.map().height())
            };
            checkpoint("span index baked");

            let coarse = navigation_stamp
                .and_then(|stamp| openshard_movement::bake::load(&navigation_path, &stamp))
                .and_then(|graph| {
                    if graph.dimensions() == (width, height) {
                        Ok(graph)
                    } else {
                        Err(openshard_movement::bake::Error::Incompatible {
                            path:   navigation_path.clone(),
                            reason: format!(
                                "dimensions {}x{}, expected {width}x{height}",
                                graph.dimensions().0,
                                graph.dimensions().1,
                            ),
                        })
                    }
                })
                .map_err(|error| {
                    eprintln!(
                        "{error}; long-distance routing disabled\ncreate it with: \
                         OPENSHARD_CLIENT={:?} cargo run --release -p openshard-movement --bin \
                         openshard-navigation-bake -- --facet {FACET}{rebake_world}",
                        dir
                    );
                })
                .ok();
            checkpoint("navigation graph loaded");

            let interior_path = openshard_client_artscan::interiors::artifact_path(&artifacts, facet);
            let interiors = interior_stamp
                .and_then(|stamp| openshard_client_artscan::interiors::load_baked(&interior_path, &stamp))
                .and_then(|graph| {
                    if graph.dimensions() == (width, height) {
                        Ok(graph)
                    } else {
                        Err(openshard_client_artscan::interiors::Error::Incompatible {
                            path:   interior_path.clone(),
                            reason: format!(
                                "dimensions {}x{}, expected {width}x{height}",
                                graph.dimensions().0,
                                graph.dimensions().1,
                            ),
                        })
                    }
                })
                .map_err(|error| {
                    eprintln!(
                        "{error}; interior diagnostic disabled\ncreate it with: \\
                         OPENSHARD_CLIENT={:?} cargo run --release -p openshard-client-artscan --bin \\
                         openshard-interiors-bake -- --facet {FACET}{rebake_world}",
                        dir
                    );
                })
                .ok();
            (ground, coarse, interiors)
        }
    };
    checkpoint("interior graph loaded");

    // What the strip says about the graph before anything has happened: the one
    // that was just loaded, or nothing. "Building" is never a startup state —
    // the only bake that runs on this side is the one a world arriving asks for,
    // and that is a whole login away.
    let navigation = match (&coarse, navigation_path) {
        (Some(graph), Some(path)) => {
            let (regions, nodes, edges) = graph.counts();
            diagnostics::Navigation::Ready {
                regions,
                nodes,
                edges,
                path,
            }
        }
        _ => diagnostics::Navigation::Absent,
    };

    // The sound mixer opens before a window exists, but its values belong to
    // the HUD's persisted settings just like light tuning does.
    let mut desk = match desk::Desk::load(std::path::Path::new(desk::PATH)) {
        Ok(desk) => desk,
        Err(error) => {
            eprintln!("{error} — starting with the default HUD layout");
            desk::Desk::default()
        }
    };
    desk.audio = desk.audio.clamped();
    let movement = desk.movement;
    // `None` is an older UI file. Its first launch keeps the command-line
    // diagnostics exactly as before; once this client closes it writes a full
    // F1 snapshot and subsequent launches restore it.
    let f1 = desk.f1;
    let mut app = App {
        audio: audio::Audio::open(dir, desk.audio.effects, desk.audio.music),
        // Built before `resources` moves `tiledata` and `map` into it: this
        // borrows both.
        world: world::WorldState {
            authoritative: world::AuthoritativeWorld {
                designs:       std::collections::HashMap::new(),
                view:          None,
                walk:          None,
                facet_checked: false,
            },
            motion:        world::PlayerMotion::new(start, Facing::walking(Direction::SouthEast)),
            presentation:  world::PresentationWorld {
                tile_animations:       StaticAnimations::build(&animdata, &tiledata),
                flame_clock:           std::time::Duration::ZERO,
                // 400 is the male human body. Its group and frame come from the
                // crowd on the first redraw, which is also what decides that a
                // placeholder nobody is walking stands.
                player:                Mobile {
                    at:        start,
                    body:      Graphic(400),
                    group:     openshard_uofiles::anim::BodyKind::of(Graphic(400)).standing(),
                    facing:    Direction::SouthEast,
                    frame:     openshard_uofiles::anim::AnimationFrameIndex(0),
                    from:      None,
                    corpse:    false,
                    hue:       Hue::NONE,
                    drawn:     Gaze::on(start),
                    equipment: Vec::new().into(),
                },
                cutaway_at:            start,
                cutaway_fades:         openshard_client_render::cutaway::Fades::default(),
                static_geometry_cache: None,
                interior_cache:        std::cell::RefCell::default(),
                others:                Vec::new(),
                corpses:               Vec::new(),
                items:                 Vec::new(),
                item_serials:          Vec::new(),
                item_houses:           Vec::new(),
                multi_preview:         Vec::new(),
                damage_numbers:        Vec::new(),
                effects:               Vec::new(),
                health_estimates:      std::collections::BTreeMap::new(),
                crowd:                 {
                    // The body's ease, which is not the camera's — see `STARTUP_EASE`.
                    let mut crowd = Crowd::default();
                    crowd.set_ease(f1.map_or(STARTUP_EASE, desk::F1Settings::ease));
                    crowd
                },
                combat_log:            combat_log::CombatLog::default(),
            },
            // Nobody is in the way of a world nobody has described yet. The
            // first `entered` fills this, beside the clutter — see
            // `clutter::project`.
            bodies:        Vec::new(),
            render_ready:  !connected,
            connection:    String::from("offline"),
            // No dial, no shard, and that is the *viewer* — the one state in
            // which this end moves the body itself. See `world::Shard`.
            shard:         match shard {
                Some(link) => world::Shard::Live(link),
                None => world::Shard::Viewer,
            },
        },
        updates,
        stall_on_update,
        post,
        navigation,
        resources: resources::Resources {
            dir: dir.to_owned(),
            world_file,
            ground,
            coarse,
            interiors,
            art,
            surfaces,
            repack_forced: false,
            texmaps,
            tiledata,
            multis: multis.clone(),
            hue_ramp,
            font_atlas,
            gumps,
            gump_atlas: GumpAtlas::empty(),
            cliloc,
            ttf_font,
            anim,
            body_def,
            equip_conv,
            skill_names,
            skill_groups,
            radar_colors,
        },
        graphics: {
            let mut graphics = graphics::GraphicsSettings {
                cutaway_disabled: std::env::var_os("OPENSHARD_DISABLE_CUTAWAY").is_some(),
                body_overlap_transparency_disabled: std::env::var_os(
                    "OPENSHARD_DISABLE_BODY_OVERLAP_TRANSPARENCY",
                )
                .is_some(),
                // The ordinary local lighting switch is still F10.
                night: false,
                // The shard's light level is used unless F1 asks for a fixed day.
                time_of_day: true,
                daylight: graphics::Daylight::new(),
                // The fringe as the environment asks for it, which is
                // `Fringe::Clamp` when it asks for nothing — F2 cycles from
                // wherever that leaves it.
                fringe: openshard_client_render::impostor::Fringe::from_env(),
                sunlit: false,
                // And the sky field off with it: while the point lights are the
                // subject, the ambient holds still. See `GraphicsSettings::sky_field`.
                sky_field: false,
                // And a torch in hand for when it is not daylight: see
                // `GraphicsSettings::lantern`.
                lantern: true,
                light_view: View::Lit,
                // The whole world. A client that started with anything ticked off
                // would be a client showing a picture nobody asked for.
                drawing: frame::Draw::EVERYTHING,
                frame_dump: None,
                frame_dumps: 0,
                show_terrain: false,
                // The sight overlay is a debugging picture, and off until somebody
                // asks for it: see `GraphicsSettings::show_sight`.
                show_sight: false,
                // Arm's length, which is the shard's `MELEE_REACH` and the reach of
                // every fighter holding no bow. A person shooting one turns it up:
                // see `GraphicsSettings::sight_reach` for why this is a knob at all.
                sight_reach: graphics::MELEE_SIGHT_REACH,
                show_interiors: false,
                buildings: false,
                z_slice: false,
                z_slice_view: openshard_client_render::interiors::ZSliceView::Auto,
                floor_view: openshard_client_render::interiors::FloorView::Auto,
                show_occluders: false,
                show_solids: solids,
                solids_only: false,
                solids_opaque: false,
                solids_everything: false,
                solids_held: 0,
                solids_drawn: 0,
                occlusion_bake: occlusion::bake::Bake::new(),
                // The item under the cursor, ringed and lit, and the ground
                // otherwise: see `graphics::HighlightTarget` and `graphics::HighlightStyle`.
                highlight: graphics::HighlightTarget::default(),
                highlight_style: graphics::HighlightStyle::default(),
                covered: None,
            };
            if let Some(f1) = f1 {
                f1.apply_to_graphics(&mut graphics);
            }
            graphics
        },
        // The device's own limit replaces WebGL2's floor once there is a device
        // to ask; the floor is the smallest thing this has to run on.
        control: Control::new(
            Camera::new(start, 1024, 768),
            2048,
            f1.map_or(STARTUP_RIG, desk::F1Settings::rig),
        ),
        zoom_limit_reported: false,
        shell: None,
        // What the last run left, already read above so audio starts with the
        // same saved settings its panel shows.
        desk,
        steer: {
            // The one decision about walking that is a player's taste rather
            // than a rule: whether a body that has walked into something
            // slides past it or stops against it. Stated here, at the top,
            // because this is the line a client config replaces when there is
            // one — nothing further down the walk has to learn about it.
            //
            // Stopping is the default and is written out rather than left
            // implicit: it is the classic client's own behaviour, and a body
            // that only ever goes where it was pointed is the one that
            // surprises nobody. Sliding is what a player opts into.
            let mut steer = steer::Steering::default();
            steer.set_always_running(movement.always_run);
            steer.set_leeway(Leeway::Eighth);
            // The other one, and here for the same reason: what a turn costs
            // the step behind it. The reference client charges its own
            // `TurnDelay` for one, so a click sideways squares the body up and
            // sets off a beat later rather than pivoting and leaving in the
            // same frame — see `steer::Turning`, which is also the default.
            //
            // Read from the environment until there is a client config to read
            // it from, because this one is only judged by feel: the three
            // answers have to be swapped between on a running shard, by the
            // person whose hand is on the mouse, or nobody can say which is
            // right.
            steer.set_turning(match std::env::var("OPENSHARD_TURN").as_deref() {
                Ok("immediate") => steer::Turning::Immediate,
                Ok("fast") => steer::Turning::Fast,
                Ok("deliberate") => steer::Turning::Deliberate,
                Ok(other) => {
                    eprintln!(
                        "OPENSHARD_TURN={other}: expected deliberate, fast or immediate — \
                         using the reference client's, deliberate"
                    );
                    steer::Turning::Deliberate
                }
                Err(_) => steer::Turning::Deliberate,
            });
            steer
        },
        auto_open_doors: movement.auto_open_doors,
        auto_opened_doors: Vec::new(),
        last_used_item: None,
        route_cache: None,
        sight_cache: None,
        terrain_cache: None,
        occluder_cache: None,
        radar_cache: openshard_client_render::radar::RadarCache::default(),
        radar_queue: openshard_client_render::radar::RadarWorkQueue::default(),
        minimap_radar_lod: openshard_client_render::radar::RadarLodSelector::default(),
        world_map_radar_lod: openshard_client_render::radar::RadarLodSelector::default(),
        radar_frame: crate::diagnostics::RadarFrame::default(),
        composite_work: openshard_client_render::composite::CompositeWorkQueue::default(),
        composite_lod: openshard_client_render::lod::BlockLodSelector::new(
            openshard_client_render::lod::LodThresholds::DEFAULT,
        ),
        input: input::Input {
            aiming:         false,
            ctrl_held:      false,
            shift_held:     false,
            war_mode_held:  false,
            // No click has landed, so the next one cannot be the second of a
            // pair.
            last_click:     None,
            // Nobody has pointed at anything yet, and a window that opens
            // under a resting cursor hears `CursorEntered` on the first move.
            pointer_inside: false,
            pointer_gump:   GumpPixel::new(0, 0),
            // No button is down on a client that has not been clicked.
            left_press:     None,
            focused:        true,
            occluded:       false,
        },
        next_tick: Instant::now(),
        last_advance: Instant::now(),
        last_view_advance: Instant::now(),
        last_frame: Instant::now(),
        window: None,
        pending: shell::Request::default(),
        map_editor: editor_mode::MapEditor::open(dir, multis),
        picking: picking::Picking::default(),
        windows: windows::Windows {
            own_windows:    Vec::new(),
            locally_closed: HashSet::new(),
            drawn_windows:  Vec::new(),
            grip:           windows::WindowGrip::Idle,
            hand:           None,
            world_press:    None,
            prompt:         None,
            stack_pass:     None,
            keyboard:       None,
        },
        tooltips: tooltips::Tooltips::default(),
        chat: Chat::default(),
        scope: Scope::new(f1.map_or(SCOPE_SPAN, desk::F1Settings::scope_span)),
        frames: frames::Frames::new(FRAMES_SPAN),
        repacks: 0,
        // Nothing unless the environment asked, and nothing this reads again:
        // the handle is the subscription. See `profile::serve`.
        #[cfg(not(target_arch = "wasm32"))]
        _puffin: profile::serve(),
        scripts: bench::scripts(),
        replay: None,
        scenario,
        lod_sweep: None,
        movement_trace: movement_trace::MovementTrace::open(),
        ping: ping::Ping::default(),
    };
    match event_loop.run_app(&mut app) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("event loop: {error}");
            ExitCode::FAILURE
        }
    }
}

/// How far page up and page down move the eye, in viewport pixels.
///
/// Half a tile's height per press, which is what the old "camera height" keys
/// moved when a step of `z` was five units: `5 * Z_STEP` is 20 pixels.
pub(crate) const PAGE_PIXELS: i32 = 20;
