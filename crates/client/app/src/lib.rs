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

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

mod clutter;
mod crowd;
mod desk;
/// The walk, held against an oracle. Tests only — see its module docs.
#[cfg(test)]
mod dst;
mod frames;
mod gump;
mod keys;
mod link;
mod replay;
mod shell;
mod steer;

/// The camera this client opens with: the reference one, the eye on the body to
/// the pixel.
///
/// Which rig it *ships* is undecided and is decided on a bench rather than here
/// — `docs/camera.md` D9.
const STARTUP_RIG: Rig = Rig::HARD;

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
const STARTUP_EASE: crowd::Ease = crowd::Ease::WALK;

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

use crowd::{Crowd, Who};
use openshard_client_net::session::Plan;
use openshard_client_net::transport::Dial;
use openshard_client_net::view::WorldView;
use openshard_client_render::animate::StaticAnimations;
use openshard_client_render::animation::FRAME_DELAY;
use openshard_client_render::atlas::{
    AnimAtlas, AtlasError, FontAtlas, LandAtlas, StaticAtlas, TexmapAtlas, TtfAtlas,
};
use openshard_client_render::bench::{self, Metrics, Scope, Script};
use openshard_client_render::blit::{self, Blit, ViewportRect};
use openshard_client_render::camera::{self, Camera, TileBounds, ViewPixel};
use openshard_client_render::control::{Control, Follow};
use openshard_client_render::cutaway::Cutaway;
use openshard_client_render::debug::View;
use openshard_client_render::follow::{Gaze, Rig};
// `gump_art` and not `gump`: this crate has a module of that name — the egui
// half of the same window — and the two are deliberately not merged. One
// draws the art, the other answers the buttons.
use openshard_client_render::container;
use openshard_client_render::gump as gump_art;
use openshard_client_render::gump::{GumpAtlas, GumpPixel, GumpRenderer};
use openshard_client_render::hue::HueRamp;
use openshard_client_render::items::{self, GroundItem};
use openshard_client_render::light::{self, Lighting};
use openshard_client_render::mobiles::{self, Mobile};
use openshard_client_render::occlusion;
use openshard_client_render::outline::{self, Outline, Ring};
use openshard_client_render::paperdoll;
use openshard_client_render::place;
use openshard_client_render::renderer::{self, GroundRenderer, MeshFaceRenderer, SpriteRenderer, Target};
use openshard_client_render::select::{self, Select, Selection};
use openshard_client_render::solids::{self, SolidsRenderer};
use openshard_client_render::sprite::{SpriteQuad, split_corners};
use openshard_client_render::statics::PickedStatic;
use openshard_client_render::text::{self, Label};
use openshard_client_render::{ground, statics};
use openshard_movement::{Heading, Lean, Leeway, Terrain};
use openshard_protocol::containers::ContainedItem;
use openshard_protocol::direction::{Direction, Facing};
use openshard_protocol::serial::Serial;
use openshard_protocol::speech::Font;
use openshard_protocol::version::ClientVersion;
use openshard_protocol::wire::{Graphic, Hue};
use openshard_protocol::world::Point;
use openshard_uofiles::anim::Anim;
use openshard_uofiles::animdata::AnimData;
use openshard_uofiles::art::Art;
use openshard_uofiles::equipconv::EquipConv;
use openshard_uofiles::font::AsciiFonts;
use openshard_uofiles::gumpart::Gumps;
use openshard_uofiles::hues::Hues;
use openshard_uofiles::map::Map;
use openshard_uofiles::texmaps::TexMaps;
use openshard_uofiles::tiledata::TileData;
use openshard_uofiles::ttf_font::TtfFont;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

/// Where the camera starts: Britain, by the bank.
const START: Point = Point::new(1495, 1629, 0);

/// What a run opens on: the state a person would otherwise have to reach by
/// hand before the picture they came to take is on the screen.
///
/// Every field is a *diagnostic's* starting position and never a gameplay
/// setting — the plans in `docs/` name places and views ("the staircase at
/// 1493,1639, as solids"), and reaching one meant walking there and finding a
/// checkbox, which is two variables moved between the picture and the claim it
/// is about. Nothing here is remembered: this is where a window opens, not what
/// it is.
#[derive(Clone, Copy, Debug, Default)]
pub struct Opening {
    /// The tile to open the camera on, if not [`START`]. See the field's use in
    /// [`run`] for what it does when there is a shard.
    pub at: Option<(u16, u16)>,
    /// Whether the occlusion grid is drawn as solids from the first frame —
    /// `docs/lighting.md` step 23.0, F5 in the window, and the checkbox in the
    /// dev panel.
    pub solids: bool,
}

/// The facet to open. Felucca: `0x1B` carries the facet's *size* and not its
/// number, so a shard serving another one is noticed by the size test in
/// [`App::entered`] rather than followed.
const FACET: u8 = 0;

/// Which client this claims to be. Every `Feature` gate on the server follows
/// from it, and this is the one ClassicUO opens with — see `docs/client.md`.
const VERSION: ClientVersion = ClientVersion::new(7, 0, 45, 65);

/// How often to redraw while somebody is mid-step. See [`App::redraw_interval`].
///
/// Roughly a 60Hz display, and deliberately a number of our own rather than the
/// monitor's: nothing here knows the refresh rate, and asking the surface would
/// tie the animation to the present mode the adapter happened to offer.
const GLIDE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(16);

/// How close together two left clicks have to land to be a double-click.
///
/// ClassicUO's `Mouse.MOUSE_DELAY_DOUBLE_CLICK`
/// (`src/ClassicUO.Client/Input/Mouse.cs`), taken as it stands: 350ms is what
/// players' hands are used to on this game, and a client that picked its own
/// number would be one where doors sometimes do not open. Distance is
/// deliberately *not* part of the test — the reference does not check it
/// either, and a mouse that slips a pixel between two clicks has not stopped
/// double-clicking.
const DOUBLE_CLICK: std::time::Duration = std::time::Duration::from_millis(350);

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
const DEAD_ZONE: f64 = 10.0;

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
const TURN_ZONE: f64 = 23.9;

/// How much of the event loop's recent past the frame panel keeps.
///
/// The same four seconds as the scope, and for the same reason: what is worth
/// looking at is the last few steps, not the session. Its own constant because
/// the two rings answer different questions and one of them is about to grow a
/// slider — see `docs/camera.md`.
const FRAMES_SPAN: std::time::Duration = std::time::Duration::from_secs(4);

/// How much of the eye's recent past the scope keeps.
///
/// Long enough to hold a whole reversal — a step is 400ms and `back_and_forth`
/// turns round every one of them — and short enough that the curve on screen is
/// the last thing that happened rather than a session's worth of ink. Four
/// seconds is ten steps at a walk.
const SCOPE_SPAN: std::time::Duration = std::time::Duration::from_secs(4);

/// How many of the shard's last lines the speech panel shows.
///
/// Small on purpose: this is not the journal — see [`shell::Hud::said`] — it is
/// enough to read the answer to what was just typed. The journal itself is kept
/// whole in the [`WorldView`], capped there, and M4 is what displays it.
const SPEECH_LINES: usize = 6;

/// The pixel height [`TtfAtlas`] rasterizes at, before the window's own
/// [`winit::window::Window::scale_factor`] scales it up for a dense display —
/// see where [`App::create_window`] builds one. Chosen to sit near
/// `fonts.mul`'s own faces (its glyphs run roughly 8 to 14 pixels tall), not
/// measured against any one of them: see the "One face, not ten" note on
/// [`openshard_uofiles::ttf_font`] for why there is only one size to choose at
/// all.
const TTF_BASE_PIXEL_HEIGHT: f32 = 16.0;

/// Open a window on `dir`'s files, and log in to `shard` if one is given.
///
/// The three arguments are the whole of what this run was asked for: which
/// client install to read, whether there is a shard to play, and which face
/// draws overhead speech. Everything else — the facet, the version claimed,
/// where the camera starts — is a constant above, because none of it is a
/// decision a caller has ever needed to make differently.
///
/// A shard is a [`Dial`] and a [`Plan`] rather than an address and a plan: how
/// the connection is opened is the caller's, which is what lets
/// `crates/e2e/playground` hand over a shard in this same process. Nothing in
/// this crate knows what a socket is any more; `client/net` does not either.
///
/// `ttf_font`, given, switches every line drawn through [`text::collect`] to
/// [`text::collect_ttf`] instead, and `fonts.mul` off entirely — see that
/// function's doc for why it is the whole line or none of it. `None` is the
/// classic client's own bitmap faces, unchanged; `Some` names a TrueType or
/// OpenType face on disk for a shard whose players type in a script
/// `fonts.mul` never shipped, Cyrillic today — nothing is bundled with the
/// engine, see [`openshard_uofiles::ttf_font`]'s doc for why.
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
pub fn run<D: Dial + Send + 'static>(
    dir: &Path,
    shard: Option<(D, Plan)>,
    ttf_font: Option<PathBuf>,
    opening: Opening,
) -> ExitCode {
    let Opening { at, solids } = opening;
    // Reading the whole facet takes a moment and a few hundred megabytes. That
    // is the shape `uofiles` has today — see the backlog in docs/client.md — and
    // it is honest to do it up front rather than to stall on the first frame.
    let map = match Map::load_facet(dir, FACET) {
        Ok(map) => map,
        Err(error) => {
            eprintln!("loading facet {FACET}: {error}");
            return ExitCode::FAILURE;
        }
    };
    let art = match Art::open(dir) {
        Ok(art) => art,
        Err(error) => {
            eprintln!("opening artLegacyMUL.uop: {error}");
            return ExitCode::FAILURE;
        }
    };
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
    // The two files a slope needs: the square textures, and the table that says
    // which of them a land graphic uses.
    let texmaps = match TexMaps::open(dir) {
        Ok(texmaps) => texmaps,
        Err(error) => {
            eprintln!("opening texmaps.mul: {error}");
            return ExitCode::FAILURE;
        }
    };
    let tiledata = match TileData::load(dir.join("tiledata.mul")) {
        Ok(tiledata) => tiledata,
        Err(error) => {
            eprintln!("opening tiledata.mul: {error}");
            return ExitCode::FAILURE;
        }
    };
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
    // `hues` itself is kept too, alongside the ramp built from it: the ramp is
    // an RGBA8 texture for the GPU passes, and `gump.rs` wants the same table
    // read as `Color16`s to pick a *solid* colour for hued text — see
    // `gump::text_color`. Building a second reader of `hues.mul` to avoid
    // holding both would be the duplication `docs/style.md` warns against, not
    // less of it.
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
    // Read and parsed once, only when asked for: a shard that never sets
    // `ttf_font` has no reason to hold a second face in memory beside
    // `fonts.mul`'s, and one that does is naming a file on this operator's
    // machine — nothing here is bundled with the engine.
    let ttf_font = match ttf_font {
        Some(path) => match TtfFont::open(&path) {
            Ok(font) => Some(font),
            Err(error) => {
                eprintln!("opening {}: {error}", path.display());
                return ExitCode::FAILURE;
            }
        },
        None => None,
    };
    eprintln!(
        "{} loaded: {}x{} tiles",
        map.facet_name(),
        map.width(),
        map.height()
    );

    // With user events, because the shard thread wakes the loop with them.
    let event_loop = match EventLoop::<link::Update>::with_user_event().build() {
        Ok(event_loop) => event_loop,
        Err(error) => {
            eprintln!("no window system: {error}");
            return ExitCode::FAILURE;
        }
    };
    event_loop.set_control_flow(ControlFlow::Wait);

    let anim = match Anim::open(dir) {
        Ok(anim) => anim,
        Err(error) => {
            eprintln!("opening anim.idx and anim.mul: {error}");
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

    // Where the character stands at boot: the camera's tile, at the height the
    // ground there actually is.
    //
    // `at` overrides [`START`] and is for looking at a *place* — the plans in
    // `docs/` name coordinates ("the staircase at 1493,1639"), and until this
    // existed the only way to reach one was to walk there, which needs a shard
    // and puts a body in front of the thing being looked at. It moves the camera
    // and nothing else: logged in, the shard still says where the character is
    // and the eye returns to them the moment anything relocks it (Home).
    let (start_x, start_y) = at.unwrap_or((START.x, START.y));
    let start = Point::new(
        start_x,
        start_y,
        map.land(start_x, start_y).map_or(START.z, |cell| cell.z),
    );

    // The connection, if this run was asked for one. Started before the window
    // exists: the login is several round trips, and there is a map to draw
    // while it happens.
    // Shared with the shard thread, which predicts the height of every step
    // from it: plain data, read by both and written by neither.
    let map = Arc::new(map);
    let tiledata = Arc::new(tiledata);
    let link = shard.map(|(dial, plan)| {
        eprintln!("logging in as {}", plan.account.0);
        link::connect(
            dial,
            plan,
            VERSION,
            Arc::clone(&map),
            Arc::clone(&tiledata),
            event_loop.create_proxy(),
        )
    });

    let mut app = App {
        tile_animations: StaticAnimations::build(&animdata, &tiledata),
        // Daylight until asked otherwise: the lighting pass is then exactly the
        // copy the blit has always been.
        night: false,
        sunlit: false,
        // And the sky field off with it: while the point lights are the subject,
        // the ambient holds still. See `App::sky_field`.
        sky_field: false,
        // And a torch in hand for when it is not daylight: see `App::lantern`.
        lantern: true,
        light_view: View::Lit,
        flame_clock: std::time::Duration::ZERO,
        map,
        art,
        surfaces,
        texmaps,
        tiledata,
        hues,
        hue_ramp,
        font_atlas,
        gumps,
        gump_atlas: GumpAtlas::empty(),
        ttf_font,
        anim,
        equip_conv,
        // The device's own limit replaces WebGL2's floor once there is a device
        // to ask; the floor is the smallest thing this has to run on.
        control: Control::new(Camera::new(start, 1024, 768), 2048, STARTUP_RIG),
        zoom_limit_reported: false,
        // 400 is the male human body. Its group and frame come from the crowd
        // on the first redraw, which is also what decides that a placeholder
        // nobody is walking stands.
        player: Mobile {
            at: start,
            body: 400,
            group: openshard_uofiles::anim::BodyKind::of(400).standing(),
            facing: Direction::SouthEast,
            frame: 0,
            from: None,
            hue: Hue::NONE,
            drawn: Gaze::on(start),
            equipment: Vec::new(),
        },
        cutaway_at: start,
        others: Vec::new(),
        items: Vec::new(),
        item_serials: Vec::new(),
        clutter: clutter::Clutter::default(),
        view: None,
        connection: String::from("offline"),
        shell: None,
        // What the last run left. A file that cannot be read is worth saying so
        // about and not worth refusing to start over: the defaults are a working
        // HUD, and the alternative is a client that will not open because of
        // where a window used to be.
        desk: match desk::Desk::load(std::path::Path::new(desk::PATH)) {
            Ok(desk) => desk,
            Err(error) => {
                eprintln!("{error} — starting with the default HUD layout");
                desk::Desk::default()
            }
        },
        link,
        facet_checked: false,
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
        aiming: false,
        ctrl_held: false,
        crowd: {
            // The body's ease, which is not the camera's — see `STARTUP_EASE`.
            let mut crowd = Crowd::default();
            crowd.set_ease(STARTUP_EASE);
            crowd
        },
        next_tick: Instant::now(),
        last_advance: Instant::now(),
        last_frame: Instant::now(),
        window: None,
        pending: shell::Request::default(),
        selected_tile: None,
        on_static: None,
        selected_static: None,
        // No click has landed, so the next one cannot be the second of a pair.
        last_click: None,
        // Nobody has pointed at anything yet, and a window that opens under a
        // resting cursor hears `CursorEntered` on the first move.
        pointer_inside: false,
        pointer_gump: GumpPixel::new(0, 0),
        own_windows: Vec::new(),
        drawn_windows: Vec::new(),
        dragging: None,
        show_terrain: false,
        show_occluders: false,
        show_solids: solids,
        solids_only: false,
        solids_opaque: false,
        solids_everything: false,
        solids_held: 0,
        solids_drawn: 0,
        occlusion_bake: occlusion::bake::Bake::new(),
        // The item under the cursor, ringed and lit, and the ground otherwise:
        // see `shell::HighlightTarget` and `shell::HighlightStyle`.
        highlight: shell::HighlightTarget::default(),
        highlight_style: shell::HighlightStyle::default(),
        covered: None,
        scope: Scope::new(SCOPE_SPAN),
        frames: frames::Frames::new(FRAMES_SPAN),
        repacks: 0,
        focused: true,
        occluded: false,
        scripts: bench::scripts(),
        replay: None,
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
const PAGE_PIXELS: i32 = 20;

/// Why the client could not start.
///
/// A binary can afford to print and exit, but the reasons are still types: a
/// `String` error loses which of these happened the moment it is formatted, and
/// "no GPU" and "no client files" want different answers from whoever hits them.
#[derive(Debug)]
enum StartupError {
    /// No window could be created.
    Window(winit::error::OsError),
    /// The window has no surface wgpu can draw to.
    Surface(wgpu::CreateSurfaceError),
    /// No adapter, or no device from it.
    NoDevice(String),
    /// The surface offers only sRGB formats, which would change the art's
    /// colours on their way to the screen.
    OnlySrgb,
    /// The land art would not pack.
    Atlas(openshard_client_render::atlas::AtlasError),
}

impl fmt::Display for StartupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Window(source) => write!(f, "creating a window: {source}"),
            Self::Surface(source) => write!(f, "creating a surface: {source}"),
            Self::NoDevice(detail) => write!(f, "no GPU to draw with: {detail}"),
            Self::OnlySrgb => write!(
                f,
                "this surface offers only sRGB formats, which would alter the art's colours",
            ),
            Self::Atlas(source) => write!(f, "packing land art: {source}"),
        }
    }
}

/// Every picture a frame can sample, packed together.
///
/// One value rather than four fields because they are grown together and used
/// together: a frame drawn from a land atlas of one camera and a static atlas
/// of another is a frame with things standing on ground that is not there.
///
/// # They grow; they are not rebuilt
///
/// An atlas used to be thrown away and packed again the moment the camera asked
/// for a graphic it did not hold, which is a full re-read of the art plus three
/// new pipelines — during a scroll, every few tiles, because a scroll is exactly
/// what keeps introducing graphics. Now [`Atlases::grow`] adds what is new to
/// what is already there and [`Atlases::upload`] sends the rows that changed.
///
/// The rebuild survives as the answer to *full* — see [`Atlases::grow`]'s note —
/// which is the one thing growing cannot do for itself.
struct Atlases {
    land: LandAtlas,
    texmaps: TexmapAtlas,
    statics: StaticAtlas,
    mobiles: AnimAtlas,
}

/// What a frame wants packed, gathered before anything is read from disk.
///
/// Three sets rather than three arguments, because they travel together
/// everywhere and two of them are keyed by numbers that look alike: a land
/// graphic and a static graphic are both a `Graphic` and are different index
/// spaces, which is a mistake a positional argument list would accept in
/// silence.
#[derive(Default)]
struct Wanted {
    /// Land graphics, which feed the land atlas and the texture atlas both.
    land: BTreeSet<Graphic>,
    /// Static graphics: what the map has standing on the ground, and what the
    /// server has dropped on top of it.
    statics: BTreeSet<Graphic>,
    /// Body, group and stored direction for everyone on screen.
    animations: BTreeSet<(u16, u8, u8)>,
}

impl Atlases {
    /// Pack a set from nothing.
    ///
    /// The startup path, and the recovery path: an atlas that has filled up is
    /// replaced by one built for what is on screen *now*, which is where the
    /// eviction lives. Growing has no other way to reclaim a graphic the camera
    /// walked away from ten minutes ago, and rebuilding used to do it by
    /// accident on every miss.
    fn build(
        art: &Art,
        surfaces: Option<&openshard_client_render::arttable::ArtTable>,
        texmaps: &TexMaps,
        tiledata: &TileData,
        anim: &mut Anim,
        wanted: &Wanted,
    ) -> Result<Self, AtlasError> {
        Ok(Self {
            land: LandAtlas::build(art, wanted.land.iter().copied())?,
            texmaps: TexmapAtlas::build(texmaps, tiledata, wanted.land.iter().copied())?,
            // The table is cloned into the atlas rather than borrowed: an atlas
            // outlives the frame it was built in and packs more art on every
            // scroll, so it has to keep what it reads a graphic's surface out of.
            statics: StaticAtlas::build_from(art, wanted.statics.iter().copied(), surfaces.cloned())?,
            mobiles: AnimAtlas::build(anim, wanted.animations.iter().copied())?,
        })
    }

    /// Add whatever of `wanted` is not packed yet, reading only that.
    ///
    /// A graphic already offered costs a lookup in a `BTreeSet` and no file
    /// access at all — including one the client ships no art for, which is the
    /// case that used to make "is the atlas stale" answer yes for ever.
    ///
    /// [`AtlasError::Full`] leaves the atlases holding whatever fitted, and the
    /// caller is expected to throw them away and [`build`](Self::build) for the
    /// current frame. That is not a lost cause: it is the eviction, and it is
    /// the only thing that stops an atlas which only ever grows from filling up
    /// and staying full.
    fn grow(
        &mut self,
        art: &Art,
        texmaps: &TexMaps,
        tiledata: &TileData,
        anim: &mut Anim,
        wanted: &Wanted,
    ) -> Result<(), AtlasError> {
        // Both halves of a ground quad from the same set, in the same growth: a
        // land graphic in one atlas and not the other draws a slope textured
        // with the terrain next door.
        self.land.add(art, wanted.land.iter().copied())?;
        self.texmaps.add(texmaps, tiledata, wanted.land.iter().copied())?;
        self.statics.add(art, wanted.statics.iter().copied())?;
        self.mobiles.add(anim, wanted.animations.iter().copied())?;
        Ok(())
    }

    /// Send whatever grew to the textures already bound.
    ///
    /// Nothing at all on the ordinary frame, and a band of rows on the frame a
    /// camera crossed a tile — where this used to be three pipelines and 48MB.
    fn upload(
        &mut self,
        queue: &wgpu::Queue,
        ground: &GroundRenderer,
        statics: &SpriteRenderer,
        mobiles: &SpriteRenderer,
    ) {
        ground.upload_changes(queue, &mut self.land, &mut self.texmaps);
        if let Some(rows) = self.statics.take_dirty() {
            statics.upload_rows(queue, self.statics.pixels(), rows);
        }
        if let Some(rows) = self.mobiles.take_dirty() {
            mobiles.upload_rows(queue, self.mobiles.pixels(), rows);
        }
    }
}

/// What a set of tile rectangles wants packed, gathered from field references.
///
/// Free rather than a method on `App` because the frame that needs it most is
/// the one holding a `&mut` borrow of the window, where no `&self` method can be
/// called — and threading the pieces explicitly is cheaper than splitting the
/// struct to please the borrow checker.
fn wanted_in(
    map: &Map,
    bands: impl IntoIterator<Item = TileBounds>,
    items: &[GroundItem],
    drawn: &[Mobile],
    animations: &StaticAnimations,
    equip_conv: &EquipConv,
) -> Wanted {
    let mut wanted = Wanted::default();
    for band in bands {
        ground::graphics_in(map, band, &mut wanted.land);
        // Every graphic of every cycle, and not the frame on screen: an atlas
        // grown for what a fire is showing this instant is an atlas grown again
        // when it stops showing it. See `StaticAnimations::cycle`.
        statics::graphics_in(map, band, animations, &mut wanted.statics);
    }
    wanted.statics.extend(items::needed_graphics(items, animations));
    wanted
        .animations
        .extend(mobiles::needed_animations(drawn, equip_conv));
    wanted
}

/// Everything a window needs, built once the window exists.
struct Screen {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    renderer: GroundRenderer,
    /// The pass that draws what stands on the ground.
    statics: SpriteRenderer,
    /// What the world is drawn into, at 1:1 and at the camera's render size —
    /// which is the viewport only at zoom 1. [`Screen::blit`] puts it on the
    /// surface.
    world: wgpu::Texture,
    /// The pass that does that, and the only place a zoom exists.
    blit: Blit,
    /// The depth buffer the three world passes share, which is what decides
    /// whether a hillside covers the wall behind it. Recreated with
    /// [`Screen::world`]: it has to be exactly the size of the image it is
    /// tested against.
    depth: wgpu::Texture,
    /// Which tile each world pixel came from, written by the same three passes
    /// and read by the blit to light the frame in world coordinates — see
    /// `openshard_client_render::place`. Recreated with [`Screen::world`] for
    /// the reason [`Screen::depth`] is: it is an attachment of the same passes
    /// and must be exactly that image's size.
    place: wgpu::Texture,
    /// The pass that draws the mobiles, which is the statics pass again with
    /// another atlas bound: a sprite is a sprite, and the two differ only in
    /// where the quad goes.
    mobile_pass: SpriteRenderer,
    /// `docs/gbuffer.md` step 4c's mesh-face pass — depth and place only, for
    /// a climbable static's honest per-face geometry. No atlas dependency, so
    /// unlike `statics`/`mobile_pass` it is never rebuilt when the atlases
    /// are.
    mesh_pass: MeshFaceRenderer,
    /// Everything currently packed, grown as the camera walks into ground it
    /// has not seen. Beside the passes rather than inside them because the CPU
    /// side of an atlas is what builds a quad and the texture is what draws it.
    atlases: Atlases,
    /// The pass that draws overhead speech, bound to `App::font_atlas` once:
    /// unlike `statics` and `mobile_pass`, nothing ever rebuilds it — the
    /// glyph atlas it is bound to is the whole of `fonts.mul` and does not go
    /// stale the way a camera-scoped atlas does.
    text_pass: SpriteRenderer,
    /// The TrueType glyphs asked for so far, when `App::ttf_font` is set.
    /// Grown a line at a time — see [`App::draw`] — the way [`Screen::atlases`]
    /// grows as the camera walks, because a face with all of Unicode to answer
    /// for has no "whole file" to pack up front the way `fonts.mul` does.
    ttf_atlas: Option<TtfAtlas>,
    /// The pass bound to [`Screen::ttf_atlas`]'s texture, rebuilt whenever that
    /// atlas is (see `App::draw`'s handling of [`AtlasError::Full`] there).
    /// `None` exactly when `ttf_atlas` is.
    ttf_pass: Option<SpriteRenderer>,
    /// Which outlined object each world pixel belongs to, or zero for none.
    ///
    /// Filled by the statics pass drawing silhouettes into it and read by
    /// [`Screen::outline`] after the blit. Recreated with [`Screen::world`] for
    /// the reason [`Screen::depth`] is: it is a colour attachment of a pass whose
    /// depth attachment is that buffer, and the two must be the same size.
    outline_mask: wgpu::Texture,
    /// The pass that turns that mask into a ring on the surface — see
    /// `openshard_client_render::outline`.
    outline: Outline,
    /// The same, for what a click is *holding*: the selected static's own
    /// silhouette, in a texture of its own.
    ///
    /// Not [`Screen::outline_mask`], and the separation is the point: the ring
    /// pass draws an edge round every id it finds, so a selection sharing that
    /// mask would come out ringed as well as washed — and the hover ring would
    /// then be two statements in one shape. Recreated with [`Screen::world`],
    /// like its neighbour and for the same reason.
    select_mask: wgpu::Texture,
    /// The pass that washes that silhouette, and the ground under it, after the
    /// blit — see `openshard_client_render::select`.
    select: Select,
    /// The pass that draws the lighting's occlusion grid as solids over the
    /// finished picture — `openshard_client_render::solids`, and step 23.0.
    ///
    /// Always built and only ever *used* while the view is on: it is one
    /// pipeline pair and an empty buffer, and the alternative — an `Option`
    /// filled on the frame somebody ticks the box — puts a shader compile in the
    /// middle of a frame a person is looking at.
    solids: SolidsRenderer,
    /// The interface's pass, bound to [`App::gump_atlas`]'s texture and to the
    /// *surface's* format: it draws over the finished frame, not into the world
    /// image. `None` exactly when `App::gumps` is.
    gump_pass: Option<GumpRenderer>,
}

/// One of this client's own windows, and the one thing about it the shard never
/// says.
///
/// Neither packet carries a position: a `0x24` names a container and a gump, a
/// `0x88` names a mobile, and where the window goes is entirely the client's —
/// once the player has dragged one it is the player's. That is the whole of
/// this type. Everything else about the window is looked up in the
/// [`WorldView`] by serial every frame, so a window can never hold a stale copy
/// of what is in the bag or on the body.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct OwnWindow {
    /// What it is a window over.
    subject: WindowSubject,
    /// Its top-left corner on the surface.
    at: GumpPixel,
}

/// What a window is over: a bag's contents, or a body.
///
/// One list holds both, because dragging, raising, hit-testing and closing are
/// the same gesture over either — decision 5 in `docs/client.md`, and the
/// reason the container's window machinery was written in this client's own
/// gump pixels rather than as an egui window. The two differ in exactly two
/// places, and each is a `match` two arms long: what is laid out for it (see
/// [`App::drawn_windows`], which is also what the pointer is tested against),
/// and what closing one means to the [`WorldView`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum WindowSubject {
    /// A container the shard has opened, by its serial.
    Container(Serial),
    /// A mobile whose paperdoll the shard has opened, by its serial. The same
    /// serial may name a container *and* a paperdoll — a player is both — which
    /// is why this is the identity and not the serial alone.
    Paperdoll(Serial),
}

/// Where the first container window opens, and how far each one after it is
/// offset.
///
/// A cascade rather than a pile: the shard sends no position, and two windows at
/// one coordinate look like one window with the wrong contents. The reference
/// client remembers a per-container position across sessions; this does not yet,
/// and the note is in `docs/client.md`.
const CONTAINER_CASCADE: GumpPixel = GumpPixel::new(24, 24);

/// The corner the cascade starts from.
const CONTAINER_ORIGIN: GumpPixel = GumpPixel::new(120, 80);

/// How many windows the cascade steps before it starts over, so that a player
/// who opens a dozen bags does not push the last of them off the screen.
const CONTAINER_CASCADE_LENGTH: i32 = 8;

struct App {
    /// The facet, shared with the shard thread — see [`link::connect`].
    map: Arc<Map>,
    art: Art,
    /// What was measured off that art off the clock, or `None` for a run with no
    /// table beside the install — see `run`, which says which it is and carries
    /// on either way.
    ///
    /// It lives here rather than in [`Atlases`] because the atlases are thrown
    /// away and rebuilt when one fills up, and a measurement of an install does
    /// not become untrue when a texture runs out of shelf space.
    surfaces: Option<openshard_client_render::arttable::ArtTable>,
    texmaps: TexMaps,
    /// Shared with the shard thread, the same way [`App::map`] is — see
    /// [`link::connect`]: the walk prediction weighs a pier's or a bridge's
    /// deck now, not only the land, and that needs `tiledata.mul` on both ends
    /// of the channel.
    tiledata: Arc<TileData>,
    /// Every hue the client ships, read as `hues.mul` stores it — a 32-step
    /// `Color16` ramp per hue. `hue_ramp` beside it is the same table packed
    /// for the GPU; this is what `gump.rs` reads to colour a `{ text }`
    /// element, which wants one CPU-side `egui::Color32` and not a texture row.
    hues: Hues,
    /// Every hue the client ships, packed once: unlike the sprite atlases it
    /// tints, nothing about it depends on where the camera is standing.
    hue_ramp: HueRamp,
    /// Every glyph `fonts.mul` ships, packed once for the reason `hue_ramp` is:
    /// nothing about it depends on the camera, and unlike a graphic there is no
    /// "not currently visible" character to leave unpacked.
    font_atlas: FontAtlas,
    /// The client's gump art, or `None` when it could not be opened — see
    /// `run`, which says so once and carries on.
    gumps: Option<Gumps>,
    /// The gump pictures packed so far.
    ///
    /// Grown a window at a time rather than built up front, unlike
    /// [`App::font_atlas`]: `gumpartLegacyMUL.uop` is 5,556 entries and a
    /// session opens a handful of them, so "the whole file" is the one thing
    /// this must not be. It lives on `App` and not on [`Screen`] for the reason
    /// [`Screen::atlases`] documents from the other side — the CPU half of an
    /// atlas builds quads and outlives any one surface.
    gump_atlas: GumpAtlas,
    /// The operator-supplied TrueType face, when `run` was asked to draw
    /// through one instead — `None` is the ordinary, `fonts.mul`-only run. Held here
    /// rather than only in [`Screen`] because it does not depend on a window
    /// existing: it is what [`Screen::ttf_atlas`] is grown from, every frame
    /// [`App::draw`] sees new characters in what is being said.
    ttf_font: Option<TtfFont>,
    /// The animations, open but not read: `anim.mul` is 195MB and frames come
    /// out of it a body at a time. `&mut` because reading one seeks the file.
    anim: Anim,
    /// What a worn item's own graphic resolves to for drawing — see
    /// [`EquipConv`]. Read once at startup like [`App::hues`]: unlike `anim`,
    /// the whole table is small enough to hold rather than seek into.
    equip_conv: EquipConv,
    /// The statics that move on their own — fires, torches, water wheels — and
    /// how far into their cycles they are.
    ///
    /// One of the clocks this app owns, and it is advanced from the same sampled
    /// instant as the crowd and the eye. Its own module argues why it is a system
    /// rather than a flag on a quad: see [`StaticAnimations`].
    tile_animations: StaticAnimations,
    /// Whether the world is drawn as if it were night: dark ambient, and the
    /// fires on the map lighting what is around them. Toggled with F10.
    ///
    /// A local switch and not the shard's clock, because there is no time of day
    /// on the wire yet. When there is, this is the field it writes to and
    /// nothing below it changes — the ambient is already a colour per frame
    /// rather than a constant read by the shader.
    night: bool,
    /// Whether a tile's ambient depends on how much of the sky its column can
    /// see: a room under a roof darker than the road outside it, before anything
    /// burns. Toggled with F6.
    ///
    /// **Off by default**, and that is a decision rather than an oversight. The
    /// sky field is a plan of its own — `docs/lighting_world.md` — and what it
    /// does is change the ambient of every tile in the frame, which is exactly
    /// the thing that must hold still while the pools of the point lights are
    /// being judged. A torch that looks wrong indoors is otherwise two questions
    /// at once, and the flat ambient is the honest baseline to compare against:
    /// see [`light::Ambient::flattened`].
    sky_field: bool,
    /// Whether the day has a sun in it: a direction, a wall's shadow lying
    /// across the street, and a lit patch on the floor behind a window. Toggled
    /// with F8, and ignored at night.
    ///
    /// Off by default, and that is not shyness: the sun's ray is walked for
    /// *every* ground pixel of a daylit frame, where firelight is walked only
    /// inside a pool. Until there is a measurement on Britain at the widest zoom
    /// — step 6 of `docs/lighting.md`, which is still open — the sky is a key
    /// somebody turns on rather than a cost every frame pays.
    sunlit: bool,
    /// Whether the player is carrying a light: a torch in the hand, throwing a
    /// beam the way the character is facing. Toggled with F7, and it does nothing
    /// in plain daylight, where the whole lighting pass is a copy.
    ///
    /// On by default, unlike the sun, and the cost is the reason the two differ:
    /// this is one more flame in a loop that already runs sixty-four of them, and
    /// a beam leaves that loop on a dot product for every fragment it does not
    /// point at. It is here at all because it is what makes a dark room
    /// *navigable* — the ambient floor is deliberately small, and without
    /// something in the hand a windowless cellar is a black rectangle with a
    /// character somewhere in it.
    ///
    /// A client-side guess, in the way `light::flame` is: nothing on the wire
    /// says a mobile is holding a torch. When the equipment layers are read for
    /// one, this is the field that answers from them and nothing below changes.
    lantern: bool,
    /// Which of the lighting pass's own values the blit draws instead of the
    /// frame. Cycled with F11, and [`View::Lit`] is the picture.
    ///
    /// Beside `night` rather than inside the renderer because it is a property of
    /// the person looking: the world is walked identically whichever view is on,
    /// and the field is written onto the frame's `Lighting` on its way to the
    /// blit. `docs/lighting.md`, decision 8.
    light_view: View,
    /// How long the flames have been burning, in the same span every other clock
    /// in the frame is advanced by.
    ///
    /// Its own accumulator rather than an `Instant`, for the reason
    /// [`StaticAnimations`] has one: `openshard-client-render` reads no clock,
    /// so the time arrives as a number, and a number sampled once per frame is
    /// what keeps a torch's flicker on the same instant as the body walking
    /// past it.
    flame_clock: std::time::Duration,
    /// The camera, who is allowed to move it, and what a drag has not yet spent.
    ///
    /// All of it arithmetic, and all of it in `client/render` where it can be
    /// reached by a test: this crate owns a window, a GPU and a `Map`, and none
    /// of the three has anything to say about a wheel notch.
    control: Control,
    /// Whether the device's refusal to hold a zoom's image has been said out
    /// loud. A silently truncated target draws a smaller world into a larger
    /// rect, which looks exactly like a bug in the projection — so it is
    /// reported, and once.
    zoom_limit_reported: bool,
    /// This client's own body.
    ///
    /// Connected, it is what the server says: `0x1B` puts it somewhere and
    /// every ack, `0x20` and `0x21` moves it. Offline it is a placeholder
    /// standing wherever the camera looks, which is enough to hold the
    /// animation reader, the frame atlas and the placement against a real
    /// install.
    player: Mobile,
    /// The tile roof-cutaway is computed from — see `draw`'s use of it with
    /// [`openshard_client_render::cutaway::Cutaway`].
    ///
    /// Deliberately not always `player.at`: that is this end's own optimistic
    /// *prediction*, published the instant a step is sent and corrected only
    /// a round trip later (see `link::Body`), and `Steering::detour`
    /// (`steer.rs`) means a held direction pinned against an obstacle asks
    /// for the very tile it is going to be refused on, every hold, for as
    /// long as it is held. Feeding that straight to `Cutaway::at` flips which
    /// roof is drawn hidden for exactly the frame between sending the doomed
    /// step and the `0x21` undoing it — a real defect this field exists to
    /// close, not the deliberate lag-compensation `player.at` is for the
    /// body's own drawn position. This only ever advances to a tile the
    /// client's own static map agrees is reachable from the last one it
    /// held, so a refusal is never drawn from; a correction snaps it the same
    /// way it snaps `player.at`.
    cutaway_at: Point,
    /// Everyone else on screen, as `0x77` and `0x78` last described them, each
    /// beside the serial the crowd's clocks are keyed by.
    ///
    /// Empty offline, and rebuilt whole from the [`WorldView`] on every update:
    /// the view is the record of what arrived and this is a projection of it,
    /// so there is nothing here to keep in step by hand.
    others: Vec<(Who, Mobile)>,
    /// Everything lying on the ground, as `0x1A` and `0x1D` last left it.
    ///
    /// A projection of the view like [`App::others`], and drawn through the
    /// same atlas and the same pass as the map's own statics: an item's picture
    /// is a static's picture. Two lists rather than one because the map's
    /// furniture never moves and these come and go with every packet.
    items: Vec<GroundItem>,
    /// What each of those items is called on the wire, at the same index.
    ///
    /// The renderer drops the serial — it draws pictures and owns no model of
    /// the world — and a click has to put it back, because "use this" is a
    /// serial and nothing else. Built in the same pass as [`App::items`] and
    /// never separately: two loops over one map is how the lists drift, and a
    /// drifted index sends the shard a double-click on whatever was next.
    item_serials: Vec<Serial>,
    /// Which of those items a step cannot go through, indexed by tile.
    ///
    /// A third projection of the view beside [`App::items`] and [`App::others`],
    /// rebuilt with them: the map's own files hold no barrel, so without this
    /// every terrain check here looks straight through one and the shard refuses
    /// the step this end thought was open. See `clutter.rs`.
    clutter: clutter::Clutter,
    /// The last thing the server said, whole.
    ///
    /// Kept only for the HUD's world window, which lists what has been decoded
    /// with the serials the three projections above drop. The renderer reads
    /// those.
    view: Option<Box<WorldView>>,
    /// What the connection is doing, for the status strip.
    connection: String,
    /// The dev HUD, once there is a window to put it on.
    shell: Option<shell::Shell>,
    /// What the HUD looked like when the client last closed: which tab, where
    /// the dev window and the operating system's window sat, and at what scale.
    ///
    /// Read once at startup and handed to the [`shell::Shell`] when there is a
    /// window; written back in [`App::exiting`]. Held here rather than in the
    /// shell because half of it — the frame — is the *platform's* window, which
    /// the HUD does not own and cannot ask about.
    desk: desk::Desk,
    /// The shard, if this run logged in to one.
    ///
    /// `None` is the offline viewer, and it is what the keyboard asks: a step
    /// is a `0x02` when there is somebody to send it to, and a camera move when
    /// there is not.
    link: Option<link::Link>,
    /// Whether the shard's facet has been compared with the one loaded. See
    /// [`App::entered`]: once, because it cannot change without a `0xBF 0x08`
    /// nothing here reads yet.
    facet_checked: bool,
    /// Where the player is asking to walk — the arrows, and the tile the mouse
    /// last sent the body to.
    ///
    /// A step is not sent from the input event: the operating system's
    /// auto-repeat is not a walking speed, a shard refuses a flood of steps as a
    /// speedhack, and a mouse held over the ground reports a move a pixel. One
    /// clock paces all of them. See `steer.rs`.
    steer: steer::Steering,
    /// Whether the right button is down, which is what makes dragging steer: a
    /// heading (or, with Ctrl, a destination) is restated on every cursor move
    /// while it is.
    aiming: bool,
    /// Whether Ctrl is held, which is what turns the right-hold from a heading
    /// — the default "run toward the cursor" idiom, no map involved — into a
    /// move order that plans a route with `find_path`. See `steer.rs`'s
    /// module docs.
    ctrl_held: bool,
    /// What everyone on screen was doing a moment ago: which animation each is
    /// playing, and how far into it.
    ///
    /// The layer above [`WorldView`] that ages what it sees — see `crowd.rs`.
    /// Real time and not the world tick: there is no world here to tick, and a
    /// real client's body animation is a wall-clock timer too.
    crowd: Crowd,
    /// When the clock next advances a frame.
    next_tick: Instant,
    /// When it last did.
    ///
    /// The crowd is moved by *measured* time and not by the interval that was
    /// waited for: `WaitUntil` is a floor and the compositor overshoots it, so a
    /// clock fed the nominal step would run slow by however much it did — which
    /// a stepping animation hides and a glide does not.
    last_advance: Instant,
    /// When the last frame was *drawn*, for the frame panel's interval.
    ///
    /// Not [`App::last_advance`], which is the clock the world is advanced on
    /// and is moved by an arriving packet as well as by a frame. Measured
    /// against that, a frame that followed a packet by a millisecond would be
    /// reported as a thousand a second, and the one number the panel exists to
    /// show — the gap between two pictures — would be the one it does not.
    last_frame: Instant,
    window: Option<Screen>,
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
    pending: shell::Request,
    /// The tile a left click last landed on, kept until the next click — see
    /// [`App::pick_tile`]. Separate from the live hover so a diagnosis does not
    /// slide off the tile the moment the mouse does.
    selected_tile: Option<(u16, u16)>,
    /// What the last drawn frame found the cursor on, when it was the map's own
    /// furniture and nothing nearer.
    ///
    /// A frame behind, and that is what makes it right rather than what it costs:
    /// a click arrives *between* frames, so the picture it is a click on is the
    /// one already drawn. Picking again at the click would ask a camera that has
    /// moved since — see the `MouseInput` arm, where this is read.
    ///
    /// It is also the tile marker's reason for going out: a wall under the cursor
    /// is what the click would take, so the diamond on the ground behind it must
    /// not be drawn as well. See [`shell::Hud::hover_lit`].
    on_static: Option<PickedStatic>,
    /// The static a left click last landed on — a wall, a door frame, a stair —
    /// kept until the next click, and washed along with the ground it stands on.
    /// See `openshard_client_render::select`.
    ///
    /// Held rather than hovered, which is the whole difference from
    /// [`App::lit_item`]'s ring: what this is for is looking at a piece of the
    /// map — reading its graphic and its height off the panel, seeing which tile
    /// it really stands on — and a highlight that moved with the mouse would be
    /// gone by the time the eye reached the numbers.
    ///
    /// Not the same question as [`App::selected_tile`] and answered by a
    /// different pick: the tile is where the *ground* under the cursor is, and a
    /// wall's picture stands two tiles up the screen from the tile it is on. Both
    /// are kept because both are asked — see [`statics::pick`].
    selected_static: Option<PickedStatic>,
    /// When the last left click landed, or `None` when the one before it
    /// already made a pair.
    ///
    /// The whole of this client's double-click detection, and the reason it is
    /// here rather than asked of the window system: the world's clicks do not go
    /// through egui — see the `MouseInput` arm — and `winit` reports presses,
    /// not gestures. Cleared when a pair fires, which is what stops three clicks
    /// from being two double-clicks; ClassicUO's `GameController` zeroes its own
    /// `lastClickTime` in the same place and for the same reason.
    last_click: Option<Instant>,
    /// Whether the cursor is inside the window at all.
    ///
    /// The other half of "does the world own the mouse", and the half no egui
    /// state can answer: a cursor that has left the window stops sending
    /// positions, so the last one it sent stays true for ever and the highlight
    /// it picked sits on the ground with nobody pointing at it. `CursorLeft` is
    /// the only event that says so.
    pointer_inside: bool,
    /// Where the cursor is in *gump* pixels — measured from the surface's own
    /// top left, not the viewport's.
    ///
    /// A second cursor and not the one [`control`](App::control) keeps, because
    /// the two are measured from different corners: the world's is relative to
    /// the viewport, so that the camera zooms about the picture's centre and not
    /// the window's, and an interface has no viewport at all. Converting one
    /// into the other at each use is the arithmetic the two pixel types exist to
    /// stop being done wrong once.
    pointer_gump: GumpPixel,
    /// The windows this client has open of its own — containers and paperdolls
    /// alike — bottom to top.
    ///
    /// Painter's order *is* z-order here, the same as the pictures inside one:
    /// the pass has no depth, so the last window in the list is the one drawn
    /// over the others and the first one picking finds. One list and not two,
    /// because a bag dragged over a paperdoll has to stay over it.
    own_windows: Vec<OwnWindow>,
    /// Every open window as the last frame laid it out: its subject, and the
    /// pictures that were drawn for it in painter's order.
    ///
    /// **What is clicked is what was drawn**, which is why this is remembered
    /// rather than recomputed at the press. A paperdoll's layout is not a
    /// function of the window alone — it reads the view, the tiledata and the
    /// client's own `gumpart` to decide which picture a worn item is — and a
    /// second walk asking those questions again is a second answer waiting to
    /// disagree with the one on the screen. It is the same rule
    /// [`items::place`] follows in the world, one layer up.
    ///
    /// A frame behind, therefore: a window that has just opened is not pickable
    /// until it has been drawn once, which is also the frame its art is packed
    /// on and so the frame it first has any pixels to be picked by.
    drawn_windows: Vec<(WindowSubject, Vec<gump_art::Picture>)>,
    /// The window being dragged and where inside it the player grabbed it, or
    /// `None` when nothing is being dragged.
    ///
    /// Keyed by subject rather than by index: raising a window on the press
    /// reorders the list, so an index taken at the press names a different
    /// window by the time the mouse moves.
    dragging: Option<(WindowSubject, GumpPixel)>,
    /// Whether the HUD is drawing what `common/movement` thinks of the ground —
    /// see [`App::terrain_overlay`].
    ///
    /// Off by default and paid for only while it is on: the overlay asks a
    /// walkability question of every tile in view and plans a route every frame,
    /// which is a bill worth a debugging picture and not worth a frame nobody is
    /// looking at.
    show_terrain: bool,
    /// Whether the HUD is drawing the lighting's occlusion grid as boxes — see
    /// [`shell::draw_occluders`](crate::shell) and `docs/lighting.md`, step 14.
    ///
    /// Off by default and paid for only while it is on, like the terrain
    /// overlay: the grid is a second walk of the map's statics over the same
    /// bounds the frame's lighting walks a moment later.
    show_occluders: bool,
    /// Whether the HUD is drawing the same grid as *solids* — decision 39 and
    /// step 23.0 of `docs/lighting.md`, and [`shell::draw_solids`](crate::shell).
    ///
    /// Beside the wireframe rather than instead of it, and that is the design: a
    /// solid hides what stands behind it and a wireframe shows it, so the two
    /// answer different halves of "is the geometry where I think it is". Both on
    /// at once is a legitimate reading and is what the outline over a filled face
    /// is for.
    show_solids: bool,
    /// Whether the world image is skipped while solids are drawn — boxes alone
    /// over a blank frame, with no sprite between their faces to compare them
    /// against. F5's own picture is deliberately the opposite of this
    /// (decision 39.2, "the wall's sprite is visible inside the box that
    /// claims to contain it"); this is for reading the box's own shape without
    /// the art arguing with it.
    solids_only: bool,
    /// Whether the solids view's fills are a straight overwrite instead of
    /// blended in — `solids::Style::opaque`, threaded through from
    /// [`shell::Request::solids_opaque`]. Off by default, matching the
    /// translucent fill the view always drew before this existed.
    solids_opaque: bool,
    /// Whether either of those two views draws the **whole** grid rather than
    /// what stands above the player's feet — the second datum, and the enum it
    /// resolves to is [`solid::Cut`](openshard_client_render::solid::Cut).
    ///
    /// A `bool` here and an enum there, and the split is deliberate: what a
    /// person picks is one of two questions and holds across frames, while the
    /// cut in force carries the player's own `z` and is a fact about the frame
    /// it is drawn in. [`App::solid_cut`] is the one place the two are joined,
    /// so a stale `z` cannot be stored anywhere.
    ///
    /// Off, because the whole grid over a town is unreadable — a pier is a slab
    /// on every plank — and the readable answer is the one a person should get
    /// without asking. What it costs to have it be a switch at all is the
    /// backlog entry it closes: a hole in a floor and a floor below the cut are
    /// the same picture, and no count beside the checkbox can tell them apart.
    solids_everything: bool,
    /// How many solids the last frame's pass was handed, and how many of those
    /// it drew — the rest fell outside the viewport.
    ///
    /// Kept and shown because a view that quietly draws a fraction of a grid
    /// looks exactly like a grid with little in it, which is
    /// [`Surface::stands`](openshard_client_render::occlusion::Surface::stands)'
    /// argument applied to the other end of the same picture. Zero on a frame
    /// the view was off, which the checkbox beside it already says.
    solids_held: usize,
    /// The half of that pair that reached the target.
    solids_drawn: usize,
    /// The blocks of the occlusion grid built for earlier frames — see
    /// [`occlusion::bake`](openshard_client_render::occlusion::bake) and
    /// `docs/lighting.md`'s step 21.5.
    ///
    /// Owned by the app because it is the app that has more than one frame. It
    /// is one map's, and this client has one map; a second facet would want a
    /// second bake, and the field being here rather than global is what would
    /// make that a change to one line.
    occlusion_bake: occlusion::bake::Bake,
    /// What the cursor is allowed to light up, and how an item says it is the
    /// one lit. Both are the HUD's to set — see [`shell::HighlightTarget`].
    highlight: shell::HighlightTarget,
    highlight_style: shell::HighlightStyle,
    /// The tile rectangle whose land and statics have been offered to the
    /// atlases, or `None` when nothing has.
    ///
    /// The state the band walk in [`App::draw`] is built on, and the one thing
    /// here that is wrong in silence: an atlas rebuilt behind this field's back
    /// forgets graphics that this still claims were offered, and the tiles that
    /// needed them simply stop being drawn — along one edge, at one camera
    /// position. So it is set from exactly two places, both of which have just
    /// finished packing, and cleared before anything that forgets.
    covered: Option<TileBounds>,
    /// The last few seconds of the eye, for the scope in the HUD.
    ///
    /// Recorded every frame the camera is advanced, from the same three values
    /// the offline bench records, so the panel's numbers and the table's are one
    /// arithmetic. See [`Scope`].
    scope: Scope,
    /// The last few seconds of the event loop, for the frame panel.
    ///
    /// Recorded every frame that is actually drawn, locked or not: this is a
    /// number about the loop and not about the camera. See [`frames::Frames`]
    /// for why it is not the scope.
    frames: frames::Frames,
    /// How many full atlas repacks this session has paid for — the eviction
    /// `AtlasError::Full` triggers, named in `docs/camera.md`: "costly and
    /// rare" was a claim nothing counted, and each one's cost otherwise reads
    /// as an ordinary heavy frame. See [`Frame::repacked`](frames::Frame) for
    /// which frame paid it.
    repacks: u64,
    /// Whether the window has the keyboard.
    ///
    /// Half of [`App::watched`], and true at construction: a window is mapped
    /// focused and winit sends no event to say the thing it has just done.
    focused: bool,
    /// Whether the compositor says the window is entirely covered.
    ///
    /// The other half of [`App::watched`]. Its own field rather than folded into
    /// the first, because the two arrive as two events in an order nothing
    /// promises, and one `bool` written by both would read the second one's
    /// answer to the first one's question.
    occluded: bool,
    /// The bench's scenarios, built once.
    ///
    /// Held rather than rebuilt per frame because the HUD lists their names, and
    /// a scenario is a `Vec` of knots: building nine of them to print nine
    /// strings would be a small allocation storm on every frame that draws.
    scripts: Vec<Script>,
    /// The one being walked in the window, while it is.
    replay: Option<replay::Replay>,
}

impl ApplicationHandler<link::Update> for App {
    /// The shard thread had something to say.
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, update: link::Update) {
        // The crowd's clock first, and before the packet is folded in. A step is
        // timestamped with `Crowd`'s own `now` — that is what the *next* step's
        // crossing is measured against (`crowd::glide_time`) — and this handler
        // used to fold packets in between two `advance` calls, so every step was
        // recorded at the previous frame's instant: up to 16ms in the past
        // mid-walk and up to a whole `FRAME_DELAY` for a body that had stopped.
        // The measurement is a difference of two of those, so the error lands on
        // the crossing *length*: the walk oracle in `dst.rs` caught a tile after
        // a turn taking 416ms instead of 400, which is a body a frame behind
        // itself and then yanked forward.
        let now = Instant::now();
        self.crowd
            .advance(now.saturating_duration_since(self.last_advance));
        self.last_advance = now;
        match update {
            link::Update::World { view, body } => self.entered(&view, body),
            // The window stays open: whatever is on screen is still the last
            // thing the server said, and closing it would take the reason with
            // it. The map viewer is what is left, which is a fair description
            // of a client that has lost its shard.
            link::Update::Lost(reason) => {
                eprintln!("disconnected: {reason}");
                self.link = None;
                return;
            }
        }
        // A step that arrives while nobody was moving finds the animation clock
        // armed for the *standing* rate, up to a whole `FRAME_DELAY` away — so
        // the first 80ms of the glide would be drawn frozen at its start, once
        // per tile. Pulling the tick forward is what makes a walk continuous
        // from its first frame; it is a `min` rather than an assignment because
        // a clock already running at the glide rate is the earlier of the two.
        let soon = now + GLIDE_INTERVAL;
        if self.crowd.anyone_gliding() && self.next_tick > soon {
            self.next_tick = soon;
        }
        if let Some(window) = self.window.as_ref() {
            window.window.request_redraw();
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        match self.create_window(event_loop) {
            Ok(window) => self.window = Some(window),
            Err(error) => {
                eprintln!("{error}");
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        // The UI sees everything first, and what it takes reaches neither the
        // camera nor the walk keys — otherwise a drag inside a panel pans the
        // world underneath it. egui never claims a close or a resize, so
        // returning here cannot swallow one.
        let consumed = match (self.shell.as_mut(), self.window.as_ref()) {
            (Some(shell), Some(screen)) => shell.on_window_event(&screen.window, &event),
            _ => false,
        };
        if consumed {
            // A key the UI took is a key this will never hear come up, and a
            // held direction that is never released walks for ever. Typing into
            // a panel should stop the character anyway, so letting go of
            // everything is both the fix and the behaviour.
            if matches!(event, WindowEvent::KeyboardInput { .. }) {
                self.steer.clear();
                self.aiming = false;
            }
            if let Some(window) = self.window.as_ref() {
                window.window.request_redraw();
            }
            return;
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(window) = self.window.as_mut() {
                    window.config.width = size.width.max(1);
                    window.config.height = size.height.max(1);
                    window.surface.configure(&window.device, &window.config);
                    self.control.resize(window.config.width, window.config.height);
                    // The world texture and the depth buffer follow the
                    // *camera's* size and not the window's, which are the same
                    // thing only at zoom 1. `draw` resizes them together.
                    window.window.request_redraw();
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };
                // An arrow is *held*, not pressed: while it is down a step is
                // due every step's length, and that clock is ours rather than
                // the operating system's repeat rate. See `keys.rs`.
                if let Some(direction) = keys::Held::direction_of(code) {
                    let step = match event.state {
                        ElementState::Pressed => {
                            let terrain = self.clutter.over(&self.map, &self.tiledata);
                            self.steer.press(
                                direction,
                                self.player.at,
                                Instant::now(),
                                self.player.facing,
                                &terrain,
                            )
                        }
                        ElementState::Released => {
                            self.steer.release(direction);
                            None
                        }
                    };
                    if let Some(facing) = step {
                        if self.walk(facing) {
                            if let Some(window) = self.window.as_ref() {
                                window.window.request_redraw();
                            }
                        }
                    }
                    return;
                }
                if event.state != ElementState::Pressed {
                    return;
                }
                if code == KeyCode::Escape {
                    event_loop.exit();
                    return;
                }
                let changed = match code {
                    // The dev window, the same switch the status strip's `dev`
                    // toggle is. Two ways in rather than one because the state is
                    // remembered: a window closed once stays closed across
                    // launches, and the strip that reopens it is itself a thing
                    // you have to know is there. A key is what you reach for
                    // without knowing anything.
                    //
                    // F1 and not a letter: letters go to the character. There is
                    // no chat line yet that would swallow one, and when there is,
                    // a key that walked the body would be the bug.
                    KeyCode::F1 => {
                        // The shell's `Desk` and not this one: see
                        // `Shell::toggle_dev`. Before there is a shell there is no
                        // window either, so this arm cannot be reached without one.
                        if let Some(shell) = self.shell.as_mut() {
                            shell.toggle_dev();
                        }
                        true
                    }
                    KeyCode::Home => {
                        self.relock();
                        true
                    }
                    // Page up and down lift the eye rather than the body,
                    // which is a pan: the map has no vertical axis to walk
                    // along, only a projection that folds `z` into `y`.
                    KeyCode::PageUp => self.control.pan(0, PAGE_PIXELS),
                    KeyCode::PageDown => self.control.pan(0, -PAGE_PIXELS),
                    // A diagnostic, not a feature: a fixed mixed-case ASCII
                    // line, sent without ever going through the keyboard —
                    // no xkb group, no IME, no `shell`'s `TextEdit`. Whatever
                    // shows up over the head from this key is exactly what
                    // `0xAD` → `0xAE` → `text::collect` do with known-good
                    // bytes, with typing entirely ruled out as a variable.
                    KeyCode::F9 => {
                        self.say("AbCdEfGh The Quick Brown Fox 123".to_owned());
                        false
                    }
                    // Night on and off. A key and not a setting because the
                    // only honest test of firelight is the two pictures side
                    // by side, and there is no time of day on the wire yet for
                    // it to follow — see `App::night`.
                    KeyCode::F10 => {
                        self.night = !self.night;
                        true
                    }
                    // The lighting's own values, one after another — see
                    // `crate::debug::View`. A key rather than a setting for the
                    // same reason F10 is one: what is being looked for is a
                    // difference between two pictures of the same instant, and
                    // anything that needed a restart would put a different world
                    // on either side of the comparison.
                    // The sun on and off, for the same reason F10 exists: the
                    // only honest test of a shadow is the two pictures of the
                    // same instant, one with it and one without.
                    KeyCode::F8 => {
                        self.sunlit = !self.sunlit;
                        true
                    }
                    // The sky field on and off — what a roof does to the light
                    // under it, against a flat ambient. A key for the third time
                    // for the same reason: the two pictures of one instant are the
                    // only way to see which of the two terms a dark room came from.
                    KeyCode::F6 => {
                        self.sky_field = !self.sky_field;
                        true
                    }
                    // The torch in the player's own hand, on and off — the same
                    // two-pictures-of-one-instant reason F10 and F8 are keys,
                    // and here it is also the only way to see what the map's own
                    // fires are doing without a beam swinging across them.
                    KeyCode::F7 => {
                        self.lantern = !self.lantern;
                        true
                    }
                    // The occlusion grid as solids — step 23.0. A key beside the
                    // checkbox for the reason F10 and F8 are keys: what is being
                    // read is the difference between two pictures of one
                    // instant, here the world with the geometry drawn over it
                    // and the world without, and a hand that has to find a
                    // checkbox has moved the camera by the time it is back.
                    KeyCode::F5 => {
                        self.show_solids = !self.show_solids;
                        true
                    }
                    // The world image, off underneath the solids — the opposite
                    // reading from F5's own (decision 39.2 draws the sprite on
                    // purpose, so the box can be checked against it): this is for
                    // looking at the box itself, with nothing behind it arguing
                    // about what shape it is.
                    KeyCode::F3 => {
                        self.solids_only = !self.solids_only;
                        true
                    }
                    // And how much of the grid either view draws — the second
                    // datum, and a key for the same reason F5 is one, only more
                    // so: the question it answers is "is that floor missing, or
                    // is it under my feet", and the two pictures that answer it
                    // differ in nothing but this.
                    KeyCode::F4 => {
                        self.solids_everything = !self.solids_everything;
                        true
                    }
                    KeyCode::F11 => {
                        self.light_view = self.light_view.next();
                        tracing::info!(view = self.light_view.name(), "lighting view");
                        true
                    }
                    _ => false,
                };
                if changed {
                    if let Some(window) = self.window.as_ref() {
                        window.window.request_redraw();
                    }
                }
            }
            // Shift is the whole of "run", and it arrives here rather than as a
            // key: `ModifiersChanged` is what winit reports a held modifier
            // with, and a `KeyboardInput` for the shift itself would miss the
            // case of it going down between two steps.
            WindowEvent::ModifiersChanged(modifiers) => {
                self.steer.set_running(modifiers.state().shift_key());
                // Toggling Ctrl mid-drag switches the right-hold from a
                // heading to a move order (or back) on the next cursor move —
                // no special-casing needed, `walk_toward_cursor` reads this
                // fresh every call.
                self.ctrl_held = modifiers.state().control_key();
            }
            // A window that loses focus never hears the key come up, and a
            // character that keeps walking into a wall while its player is in
            // another window is not what the key meant. The destination goes
            // with it, for the same reason: nobody is watching it be walked to.
            //
            // It is also half of what paces the loop — see [`App::watched`] —
            // and regaining focus has to ask for a frame, because the redraw
            // that would have asked for the next one is the one that stopped
            // being drawn.
            WindowEvent::Focused(focused) => {
                self.focused = focused;
                if focused {
                    if let Some(window) = self.window.as_ref() {
                        window.window.request_redraw();
                    }
                } else {
                    self.steer.clear();
                    self.aiming = false;
                }
            }
            // Entirely covered by another window: the compositor will not show
            // anything drawn, so the loop stops drawing at the display's rate
            // and falls back to the animation clock. Uncovered, it restarts the
            // same way focus does.
            WindowEvent::Occluded(occluded) => {
                self.occluded = occluded;
                if !occluded {
                    if let Some(window) = self.window.as_ref() {
                        window.window.request_redraw();
                    }
                }
            }
            // A cursor that has left says so once and then goes quiet, so the
            // flag is what stands in for the positions that stop arriving. It
            // reaches here even when egui consumed the move that preceded it:
            // `on_window_event` does not claim these.
            WindowEvent::CursorEntered { .. } => {
                self.pointer_inside = true;
            }
            WindowEvent::CursorLeft { .. } => {
                self.pointer_inside = false;
                if let Some(window) = self.window.as_ref() {
                    window.window.request_redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer_inside = true;
                // Relative to the *viewport* and not the window: the camera's
                // own centre is the viewport's, so a cursor measured from the
                // window would zoom about a point half a panel away.
                let origin = self.shell.as_ref().map_or((0, 0), |shell| {
                    (shell.viewport().x as i32, shell.viewport().y as i32)
                });
                let (x, y) = (position.x as i32 - origin.0, position.y as i32 - origin.1);
                // The interface's cursor is measured from the surface's own
                // corner and in gump pixels, which is what everything drawn by
                // the gump pass is placed in.
                let scale = self.gump_scale();
                self.pointer_gump = GumpPixel::new(
                    (position.x as f32 / scale) as i32,
                    (position.y as f32 / scale) as i32,
                );
                let mut changed = self.control.cursor_moved(x, y);
                changed |= self.drag_own_window();
                // Held, the button steers: a heading toward wherever the cursor
                // is, by default, or a Ctrl-held move order — see
                // `walk_toward_cursor` and `steer.rs`'s module docs for why
                // those are two different things and not one idiom stated
                // twice.
                if self.aiming {
                    changed |= self.walk_toward_cursor();
                }
                if changed {
                    if let Some(window) = self.window.as_ref() {
                        window.window.request_redraw();
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if button == winit::event::MouseButton::Middle {
                    self.control.set_panning(state == ElementState::Pressed);
                }
                // A left click selects the tile under the cursor for the Tile
                // panel — reached here and not through egui, because `consumed`
                // above already sent every click the UI wanted to it.
                if button == winit::event::MouseButton::Left && state == ElementState::Released {
                    self.dragging = None;
                }
                // A container window takes the press before the world sees it,
                // the same way a panel does: the click that raises a bag must
                // not also select the tile behind it, and it must not start a
                // double-click pair that would use whatever is under there.
                if button == winit::event::MouseButton::Left
                    && state == ElementState::Pressed
                    && self.press_on_own_window()
                {
                    if let Some(window) = self.window.as_ref() {
                        window.window.request_redraw();
                    }
                } else if button == winit::event::MouseButton::Left && state == ElementState::Pressed {
                    // The camera as it stands, which between two frames is the
                    // one the last frame was drawn with — the picture the player
                    // is clicking on.
                    let camera = *self.control.camera();
                    // What the last frame found under the cursor, when it was a
                    // piece of the map. `None` on bare ground, which is how a
                    // selection is put out: there is nothing to select where
                    // nothing is standing.
                    self.selected_static = self.on_static;
                    // **The tile is the selected thing's own, and only the ground
                    // under a bare click is unprojected.** Those are two different
                    // arithmetics and they answer differently on purpose: a wall's
                    // picture stands up the screen from the cell it is built on,
                    // so the ground *under the cursor* is the cell behind the
                    // wall — two tiles behind it, for a wall of ordinary height.
                    // Selecting a wall and marking that other tile is the client
                    // saying "this one" about two places at once, which is what
                    // this arm used to do.
                    //
                    // So the marker, the panel's readout and the wash all come off
                    // one value now. Derived here rather than asserted anywhere:
                    // there is no second source for them to drift from.
                    self.selected_tile = match self.selected_static {
                        Some(picked) => Some((picked.at.x, picked.at.y)),
                        None => self.pick_tile(camera).map(|tile| (tile.x, tile.y)),
                    };
                    // And the second click of a pair is a *use*: a door opens, a
                    // container opens, food is eaten. Which of those it is, is
                    // the shard's answer and not this end's — see
                    // `openshard_client_net::interact`.
                    let now = Instant::now();
                    let paired = self
                        .last_click
                        .is_some_and(|last| now.duration_since(last) <= DOUBLE_CLICK);
                    // Cleared on a pair rather than restarted, so a third click
                    // starts a fresh one — ClassicUO's own reset.
                    self.last_click = (!paired).then_some(now);
                    if paired {
                        self.use_under_cursor(camera);
                    }
                    if let Some(window) = self.window.as_ref() {
                        window.window.request_redraw();
                    }
                }
                // A right hold is a heading toward the cursor by default, or a
                // Ctrl-held move order — either way it stays under way while
                // the button is, driven from `CursorMoved`. Left is spoken for
                // by the Tile panel above, and the middle button pans.
                // Right over a window closes it — the reference client's own
                // gesture — and does not steer: a press that never reached the
                // world cannot be a heading into it.
                if button == winit::event::MouseButton::Right
                    && state == ElementState::Pressed
                    && self.close_window_under_pointer()
                {
                    if let Some(window) = self.window.as_ref() {
                        window.window.request_redraw();
                    }
                } else if button == winit::event::MouseButton::Right {
                    self.aiming = state == ElementState::Pressed;
                    if self.aiming {
                        if self.walk_toward_cursor() {
                            if let Some(window) = self.window.as_ref() {
                                window.window.request_redraw();
                            }
                        }
                    } else {
                        // A heading stops the instant the button does — unlike
                        // a move order, which keeps walking itself there after
                        // the button that gave it is gone. `mouse_up` only
                        // touches the heading; a Ctrl-held destination in
                        // flight is untouched.
                        self.steer.mouse_up();
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                // A notch is a line on a wheel and a fraction of one on a
                // touchpad, and only the sign is asked for here: the ladder is
                // what decides how far a notch goes.
                let notches = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                    winit::event::MouseScrollDelta::PixelDelta(position) => position.y as f32,
                };
                if notches != 0.0 && self.zoom(notches > 0.0) {
                    if let Some(window) = self.window.as_ref() {
                        window.window.request_redraw();
                    }
                }
            }
            WindowEvent::RedrawRequested => self.draw(),
            _ => {}
        }
    }

    /// Re-arm the animation clock and ask for a redraw when it has advanced.
    ///
    /// `winit`'s idiomatic timer: `ControlFlow::WaitUntil` sleeps the event
    /// loop rather than spinning it, and returning here every
    /// [`App::redraw_interval`] is what stands in for a real client's own
    /// `Mobile.ProcessAnimation` poll.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        // A held arrow — or a tile the mouse sent the body to — asks for a step
        // every step's length. Here and not in the input event: the operating
        // system repeats a held key at a rate that is not a walking speed, a
        // mouse held over the ground reports a move a pixel, and the fast half
        // of either is refused by the shard as a speedhack — which reads as the
        // walk stuttering. See `steer.rs`.
        //
        // Twice at most, because a turn is a step that covers no ground and
        // costs no time against the shard's pace budget: the step it precedes is
        // due the same instant, and holding that back to the next wake would put
        // a frame of standing still exactly where the player asked for movement.
        // Two and not a loop — the second ask is the step the turn was for, and
        // anything past it is a rate, which is what the clock is for.
        let mut moved = false;
        for _ in 0..2 {
            let terrain = self.clutter.over(&self.map, &self.tiledata);
            let Some(facing) = self.steer.due(now, self.player.at, self.player.facing, &terrain) else {
                break;
            };
            moved |= self.walk(facing);
        }
        if moved {
            if let Some(window) = self.window.as_ref() {
                window.window.request_redraw();
            }
        }
        // The animation clock. Watched, this is a safety net rather than the
        // pacer — `draw` asks for the next frame itself and the display answers
        // — and it is kept for the paths where that ask does not happen: `draw`
        // returns early with no window, with a swapchain it had to rebuild, and
        // on a compositor that refused to hand over a texture. Without it, one
        // of those would stop the loop dead until the next input event. The
        // redraw requests coalesce, so a net that fires while the display is
        // already pacing costs a wake and no frame.
        if now >= self.next_tick {
            self.next_tick = now + self.redraw_interval();
            if let Some(window) = self.window.as_ref() {
                window.window.request_redraw();
            }
        }
        // Three reasons to come back, so three terms: the animation clock,
        // whatever the UI is animating, and the next step a held key is owed.
        // The deadline is the earliest — a loop that slept past the step would
        // walk at whatever rate it happened to wake at.
        // `checked_add`, because a still UI asks for eternity
        // (`Duration::MAX`, see `Shell::repaint_after`) and `now + MAX`
        // overflows the instant rather than meaning "never". An overflow is
        // exactly the case where the UI wants no frame of its own, so it falls
        // back to the animation clock.
        let deadline = match self.shell.as_ref().map(shell::Shell::repaint_after) {
            Some(after) => match now.checked_add(after) {
                Some(ui) => self.next_tick.min(ui),
                None => self.next_tick,
            },
            None => self.next_tick,
        };
        let deadline = match self.steer.deadline() {
            Some(step) => deadline.min(step),
            None => deadline,
        };
        event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
    }

    /// The loop is over: write down what the HUD looked like.
    ///
    /// Here and not on `CloseRequested`, because that is one of several ways out
    /// — `event_loop.exit()` is also called from a startup failure and from the
    /// link — and this is the one place all of them pass through. A client that
    /// is killed writes nothing, which is the honest behaviour: the file says
    /// where things were when the client was last *closed*.
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // The HUD's half — tab, panel, scale — and then the platform's, which
        // only the window itself can answer for.
        if let Some(shell) = self.shell.as_ref() {
            self.desk = shell.desk();
        }
        if let Some(screen) = self.window.as_ref() {
            let size = screen.window.inner_size();
            // A window whose position the platform will not report — Wayland
            // does not, by design — keeps whatever the file already said rather
            // than being moved to the origin. Half a frame restored is better
            // than a window that walks to the top-left corner every launch.
            let position = screen.window.outer_position().ok();
            let previous = self.desk.window;
            self.desk.window = Some(desk::Frame {
                x: position.map_or_else(|| previous.map_or(0, |frame| frame.x), |at| at.x),
                y: position.map_or_else(|| previous.map_or(0, |frame| frame.y), |at| at.y),
                width: size.width.max(1),
                height: size.height.max(1),
                maximized: screen.window.is_maximized(),
            });
        }
        if let Err(error) = self.desk.save(std::path::Path::new(desk::PATH)) {
            eprintln!("{error}");
        }
    }
}

impl App {
    /// Take a step, answering whether anything on screen changed.
    ///
    /// Movement is clamped to the map rather than wrapped: walking off the north
    /// edge in UO is impossible, and a camera that wrapped would draw a seam
    /// between two sides of the world.
    fn walk(&mut self, facing: Facing) -> bool {
        // A hand on the body outranks a scenario, the same way a hand on the
        // camera outranks the lock: the two would otherwise both write the
        // player's position and the picture would be neither.
        self.replay = None;

        // Connected, the keyboard moves nothing: it asks. The body goes where
        // the `0x22` says it went, which is the whole point of the walk
        // handshake — a client that stepped locally and corrected later would
        // be predicting, and the prediction lives in `Walk` where it can be
        // rolled back.
        if let Some(link) = self.link.as_ref() {
            link.step(facing);
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
        let turn = matches!(
            openshard_movement::intend(self.player.at, Facing::walking(self.player.facing), facing),
            openshard_movement::Intent::Turned { .. }
        );
        let (x, y) = match turn {
            true => (self.player.at.x, self.player.at.y),
            false => {
                let (dx, dy) = facing.direction.step();
                let x = (i32::from(self.player.at.x) + dx).clamp(0, self.map.width() as i32 - 1);
                let y = (i32::from(self.player.at.y) + dy).clamp(0, self.map.height() as i32 - 1);
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
        let terrain = openshard_movement::MapTerrain::new(self.map.as_ref(), &self.tiledata);
        let ground = i8::try_from(terrain.predict_step(self.player.at, x, y)).unwrap_or(self.player.at.z);
        // The crowd's clock first, before the step is folded in, and for the
        // same reason `App::user_event` does it for a step off the wire: a step
        // is timestamped with `Crowd`'s own `now`, and this is called from
        // `about_to_wait` — where that clock is as old as the last frame. A step
        // recorded up to a frame in the past starts its crossing there, and
        // `crowd::crossing` then measures the time it has left from the same
        // stale instant. This is the offline half of the walk and it had the
        // defect the online half was already fixed for.
        let now = Instant::now();
        self.crowd
            .advance(now.saturating_duration_since(self.last_advance));
        self.last_advance = now;
        // Through the crowd like anyone else, so the placeholder walks when it
        // walks and stands when it stops. `None` is who it is: no shard has
        // named it, so it has no serial.
        // `Crowd::see` starts a fresh `Mobile` with no equipment — nobody
        // sent this placeholder a `0x78` — so whatever it was already wearing
        // is carried across by hand, the way `WorldView` carries it across a
        // `0x77`/`0x20` that names none either.
        let equipment = std::mem::take(&mut self.player.equipment);
        self.player = self.crowd.see(
            None,
            Point::new(x, y, ground),
            Graphic(self.player.body),
            facing,
            self.player.hue,
        );
        self.player.equipment = equipment;
        // Offline there is no shard to refuse a step, so nothing here is
        // speculative the way an online prediction is — trusted outright,
        // same as a correction is.
        self.cutaway_at = self.player.at;
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
    /// `self.ctrl_held` says which. Without Ctrl this is a heading — no map
    /// touched, no route planned, the same "run toward the cursor" a strategy
    /// game's held mouse button means. With it, a move order: a route planned
    /// with `find_path` to the exact tile. See `steer.rs`'s module docs for why
    /// they are not the same thing wearing one name.
    fn walk_toward_cursor(&mut self) -> bool {
        // As above: between frames, what is on screen is what the last frame drew.
        let Some(tile) = self.pick_tile(*self.control.camera()) else {
            return false;
        };
        let facing = if self.ctrl_held {
            let terrain = self.clutter.over(&self.map, &self.tiledata);
            self.steer.go_to(
                (tile.x, tile.y),
                self.player.at,
                Instant::now(),
                self.player.facing,
                &terrain,
            )
        } else {
            let terrain = self.clutter.over(&self.map, &self.tiledata);
            self.steer.steer(
                self.ask_to_cursor(*self.control.camera()),
                self.player.at,
                Instant::now(),
                self.player.facing,
                &terrain,
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
    fn ask_to_cursor(&self, camera: Camera) -> Option<steer::Ask> {
        let (cursor_x, cursor_y) = self.control.cursor();
        // The body's *drawn* pixel, height and all: what a player aims relative
        // to is the sprite they can see, not the tile beneath it.
        ask_between(camera::project(self.player.at), camera.pick(cursor_x, cursor_y))
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
    fn use_under_cursor(&self, camera: Camera) {
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
        let cutaway = Cutaway::at(&self.map, &self.tiledata, self.cutaway_at, true);
        // A creature under the cursor takes the click, and no item is used: it
        // is what the highlight is telling the player they are pointing at, and
        // using the barrel *behind* the shopkeeper is the one answer that is
        // certainly wrong. What a mobile's double-click asks for is the
        // paperdoll — the same `0x06` an item gets, answered differently by the
        // shard (`DoubleClick::interpret`), which is why nothing here says
        // "paperdoll" on the way out.
        let drawn = self.drawn_now(&window.atlases.mobiles);
        let on_mobile = mobiles::pick(
            &drawn.iter().map(|(_, mobile)| mobile.clone()).collect::<Vec<_>>(),
            &camera,
            &window.atlases.mobiles,
            &cutaway,
            &self.equip_conv,
            self.control.cursor(),
        );
        if let Some(index) = on_mobile {
            // A body with no serial is one this client is drawing without the
            // shard having named it — the offline viewer's placeholder — and
            // there is nothing to ask about.
            if let (Some(serial), Some(link)) = (drawn[index].0, self.link.as_ref()) {
                link.use_object(serial);
            }
            return;
        }
        let Some(index) = items::pick(
            &self.items,
            &camera,
            &self.tiledata,
            &self.tile_animations,
            &window.atlases.statics,
            &cutaway,
            self.control.cursor(),
        ) else {
            return;
        };
        let serial = self.item_serials[index];
        match self.link.as_ref() {
            Some(link) => link.use_object(serial),
            None => tracing::info!(serial = serial.raw(), "nothing used: no shard is connected"),
        }
    }

    /// Real pixels per gump pixel, which is egui's own scale.
    ///
    /// Not the window's scale factor: the interface's art is placed at
    /// coordinates egui laid out in points, so any other number here slides a
    /// window's pictures off whatever egui drew beside them — and the cursor,
    /// which arrives from `winit` in real pixels, has to come back the same way
    /// or a click lands where the picture is not.
    fn gump_scale(&self) -> f32 {
        self.shell
            .as_ref()
            .map(|shell| shell.gumps().scale())
            .unwrap_or(1.0)
    }

    /// Open a window for everything the shard has opened and this client has
    /// not placed yet, and drop the windows whose subject is gone.
    ///
    /// Run once a frame rather than when the packet arrived, and idempotent for
    /// that reason: the `0x24` and the `0x88` are folded into the [`WorldView`]
    /// by `client/net`, which knows nothing about screens, so the window is
    /// this end noticing that the view has grown something it has nowhere to
    /// put.
    ///
    /// The drop is the other direction of the same idea: a container removed
    /// from the world — or a mobile destroyed — takes its entry in the view
    /// with it (see `WorldView::apply`'s `Remove` arm), and a window over
    /// nothing must not outlive it.
    fn sync_own_windows(&mut self) {
        let Some(view) = self.view.as_ref() else {
            // No world, no windows: a map viewer has no shard to have opened
            // one, and anything left over is from a session that has ended.
            self.own_windows.clear();
            return;
        };
        self.own_windows.retain(|window| match window.subject {
            WindowSubject::Container(serial) => view.containers.contains_key(&serial),
            WindowSubject::Paperdoll(serial) => view.paperdolls.contains_key(&serial),
        });
        // Containers first and paperdolls after, and both in the view's own
        // iteration order — which is a `HashMap`'s and therefore not stable.
        // That decides only where two windows opened on the *same frame*
        // cascade to, and nothing else: a window's position is its own from the
        // moment it is placed.
        let wanted = view
            .containers
            .keys()
            .map(|serial| WindowSubject::Container(*serial))
            .chain(
                view.paperdolls
                    .keys()
                    .map(|serial| WindowSubject::Paperdoll(*serial)),
            );
        for subject in wanted.collect::<Vec<_>>() {
            if self.own_windows.iter().any(|window| window.subject == subject) {
                continue;
            }
            let step = self.own_windows.len() as i32 % CONTAINER_CASCADE_LENGTH;
            self.own_windows.push(OwnWindow {
                subject,
                at: GumpPixel::new(
                    CONTAINER_ORIGIN.x + CONTAINER_CASCADE.x * step,
                    CONTAINER_ORIGIN.y + CONTAINER_CASCADE.y * step,
                ),
            });
        }
    }

    /// Which window the cursor is over, topmost first, or `None`.
    ///
    /// Against **every picture the window drew**, and each against its own
    /// opaque texels rather than a bounding box: a bag's art has transparent
    /// corners, a paperdoll's frame has a large transparent middle, and a click
    /// in either belongs to whatever is behind it — which is usually the world.
    /// A hat that the doll wears past the edge of its frame is the window's, and
    /// a hole in the frame's own corner is not: both fall out of asking the
    /// list, and neither did when this asked the background alone.
    ///
    /// The list is the last frame's — see [`App::drawn_windows`] for why it is
    /// remembered rather than laid out again here — and the z-order is
    /// [`App::own_windows`]'s, which is current: raising a window on the press
    /// must not wait for a frame.
    fn window_under_pointer(&self) -> Option<WindowSubject> {
        let cursor = self.pointer_gump;
        self.own_windows.iter().rev().find_map(|window| {
            let pictures = self
                .drawn_windows
                .iter()
                .find(|(subject, _)| *subject == window.subject)
                .map(|(_, pictures)| pictures.as_slice())?;
            gump_art::pick(pictures, cursor, &self.gump_atlas).map(|_| window.subject)
        })
    }

    /// Raise a window to the top of the pile, so that the one just clicked is
    /// the one drawn over the others.
    fn raise_window(&mut self, subject: WindowSubject) {
        if let Some(index) = self
            .own_windows
            .iter()
            .position(|window| window.subject == subject)
        {
            let window = self.own_windows.remove(index);
            self.own_windows.push(window);
        }
    }

    /// A left press over one of this client's windows: raise it and take hold
    /// of it.
    ///
    /// Answers whether the press belonged to a window, so the caller can leave
    /// the world's own click alone when it did — a press that raised a bag must
    /// not also select the tile behind it.
    fn press_on_own_window(&mut self) -> bool {
        let Some(subject) = self.window_under_pointer() else {
            return false;
        };
        self.raise_window(subject);
        let grab = self
            .own_windows
            .last()
            .map(|window| {
                GumpPixel::new(
                    self.pointer_gump.x - window.at.x,
                    self.pointer_gump.y - window.at.y,
                )
            })
            .unwrap_or_default();
        self.dragging = Some((subject, grab));
        true
    }

    /// Move the window being dragged so that the point the player grabbed stays
    /// under the cursor. Answers whether anything moved.
    fn drag_own_window(&mut self) -> bool {
        let Some((subject, grab)) = self.dragging else {
            return false;
        };
        let at = GumpPixel::new(self.pointer_gump.x - grab.x, self.pointer_gump.y - grab.y);
        let Some(window) = self
            .own_windows
            .iter_mut()
            .find(|window| window.subject == subject)
        else {
            return false;
        };
        let moved = window.at != at;
        window.at = at;
        moved
    }

    /// Close the window under the cursor, if there is one.
    ///
    /// The right button, which is what the reference client closes a gump with,
    /// and it is *not* a conflict with the right-hold that steers: a press over
    /// a window never reaches the world, the same way a press over a panel does
    /// not. Answers whether a window was closed.
    ///
    /// Nothing goes out on the wire, for either kind. There is no
    /// close-container packet and no close-paperdoll packet — the shard keeps
    /// its own list of who has what open — which is why the view is told: see
    /// `WorldView::container_closed`, which drops the contents with the window,
    /// and `WorldView::paperdoll_closed`, which drops nothing else at all
    /// because the equipment belongs to the body.
    fn close_window_under_pointer(&mut self) -> bool {
        let Some(subject) = self.window_under_pointer() else {
            return false;
        };
        let Some(view) = self.view.as_mut() else {
            return false;
        };
        match subject {
            WindowSubject::Container(serial) => {
                view.container_closed(serial);
            }
            WindowSubject::Paperdoll(serial) => {
                view.paperdoll_closed(serial);
            }
        }
        self.own_windows.retain(|window| window.subject != subject);
        self.dragging = None;
        true
    }

    /// Say a line out loud, if there is a shard to hear it.
    ///
    /// Nothing is echoed locally. A shard sends every speaker their own words
    /// back — that is what makes `0xAE` exist — so a client that also drew them
    /// itself would show everything twice, and a line that never reached the
    /// server would look exactly like one that did.
    ///
    /// Offline the line goes nowhere and says so in the log rather than
    /// silently: the map viewer has nobody to talk to, and a chat box that
    /// swallowed what was typed would read as a broken connection.
    fn say(&mut self, line: String) {
        match self.link.as_ref() {
            Some(link) => link.say(line),
            None => tracing::info!(%line, "nothing said: no shard is connected"),
        }
    }

    /// Answer an open dialog and take it off the screen.
    ///
    /// The close is this end's, and it is why the view is touched here rather
    /// than waiting for a packet: the server sends one `0xB0` and waits for one
    /// `0xB1`, and nothing ever arrives to say the window is gone. See
    /// [`WorldView::gump_closed`](openshard_client_net::view::WorldView::gump_closed).
    fn answer_gump(&mut self, reply: link::GumpReply) {
        let gump_id = openshard_protocol::gump::GumpId(reply.gump_id.0);
        if let Some(link) = self.link.as_ref() {
            link.answer_gump(reply);
        }
        if let Some(view) = self.view.as_mut() {
            view.gump_closed(gump_id);
        }
    }

    /// Put the eye back on the body and lock it there.
    ///
    /// Where the body is *drawn* this frame, not the tile it is nominally on:
    /// a relock mid-step would otherwise land up to half a tile from the sprite
    /// and be corrected on the frame after.
    fn relock(&mut self) {
        self.player.drawn = self.drawn_player();
        self.control.relock(mobiles::gaze(&self.player));
    }

    /// Where our own body is drawn this instant, off the crowd's clock.
    ///
    /// Read rather than stored, and this is the one place that reads it: the
    /// position is a function of a clock and an ease's state, so one read once a
    /// frame is what keeps the sprite, the camera and the scope on the same
    /// number. A crowd that has never heard of us — before a shard names the
    /// body, and for the frame a placeholder is created on — answers with the
    /// tile, which is where a body nobody is easing stands.
    fn drawn_player(&self) -> Gaze {
        self.crowd
            .drawn_for(self.me())
            .unwrap_or_else(|| Gaze::on(self.player.at))
    }

    /// Whether there is anybody to show a frame to: the window has the keyboard
    /// and is not covered.
    ///
    /// What the loop's pacing hangs on, and the whole of what this client does
    /// about power. A window in the background still ages its animations — the
    /// crowd has to be where it would have been when the player comes back —
    /// but it does it on the animation clock rather than at the display's rate.
    fn watched(&self) -> bool {
        self.focused && !self.occluded
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
    fn pacing(&self) -> frames::Pacing {
        if self.watched() {
            return frames::Pacing::Display;
        }
        frames::Pacing::Timer(self.redraw_interval())
    }

    /// The fallback timer's interval. See [`App::pacing`] for when it is the one
    /// that decides.
    fn redraw_interval(&self) -> std::time::Duration {
        let moving = self.crowd.anyone_gliding() || self.control.settling() || self.replay.is_some();
        if moving { GLIDE_INTERVAL } else { FRAME_DELAY }
    }

    /// Start walking one of the bench's scenarios in the window.
    ///
    /// Offline only: with a shard connected the body goes where the `0x22` says
    /// it went, and a second writer would be two clients fighting over one
    /// character. The panel does not offer the buttons in that state and this
    /// refuses anyway, because a guard that only lives in a widget is a guard
    /// until somebody adds a keybinding.
    fn start_replay(&mut self, name: &str) {
        if self.link.is_some() {
            return;
        }
        let Some(script) = self.scripts.iter().find(|script| script.name == name).cloned() else {
            return;
        };
        // The height the script's own `z = 0` means here. Read once, from the
        // tile it starts on — see `Replay`'s docs on why not per tile.
        let ground = script.knots().first().map_or(self.player.at.z, |knot| {
            Self::in_bounds(i32::from(knot.from.x), i32::from(knot.from.y), &self.map)
                .and_then(|(x, y)| self.map.land(x, y))
                .map_or(self.player.at.z, |cell| cell.z)
        });
        let replay = replay::Replay::new(script, ground);
        if let Some(start) = replay.start() {
            // Put down rather than walked, and the camera cut to it: a body
            // that strolled to the start of a scenario would be measured on the
            // way there, and an eye that eased across a facet is a second
            // motion on top of the one being looked at.
            let (body, hue) = (Graphic(self.player.body), self.player.hue);
            let equipment = std::mem::take(&mut self.player.equipment);
            self.player = self
                .crowd
                .snap(self.me(), start, body, Facing::walking(self.player.facing), hue);
            self.player.equipment = equipment;
            self.cutaway_at = self.player.at;
            self.control.relock(mobiles::gaze(&self.player));
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
    fn advance_replay(&mut self, elapsed: std::time::Duration) {
        let Some(replay) = self.replay.as_mut() else {
            return;
        };
        let moves = replay.advance(elapsed);
        let finished = replay.finished();
        for step in moves {
            let (body, hue) = (Graphic(self.player.body), self.player.hue);
            let equipment = std::mem::take(&mut self.player.equipment);
            self.player = match step.glided {
                true => self.crowd.see(self.me(), step.to, body, step.facing, hue),
                false => self.crowd.snap(self.me(), step.to, body, step.facing, hue),
            };
            self.player.equipment = equipment;
            self.cutaway_at = self.player.at;
        }
        if finished {
            self.replay = None;
        }
    }

    /// Who the crowd knows our own body as.
    ///
    /// Our serial once a shard has named us, and `None` for the offline
    /// placeholder — see [`Who`].
    fn me(&self) -> Who {
        self.view.as_ref().map(|view| view.player.serial)
    }

    /// Point the eye at our own body, wherever the glide has it this instant.
    ///
    /// Called every frame and not only when a step arrives: the glide moves the
    /// body a few pixels per frame, and an eye that moved a tile at a time would
    /// jerk the whole world under it. Reads the crowd's clock straight, so it is
    /// also what keeps the eye and the sprite from disagreeing by a frame.
    ///
    /// `elapsed` is the same span the crowd's clock was just advanced by, and
    /// deliberately the same value: a rig that filters is integrating over it,
    /// and a camera integrating a different amount of time than the body moved
    /// through lags by whatever the difference was — which varies frame to
    /// frame, and varying lag is what an eye reads as a stutter.
    fn follow_player(&mut self, elapsed: std::time::Duration) {
        self.player.drawn = self.drawn_player();
        let gaze = mobiles::gaze(&self.player);
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

    /// A viewport that grew may have taken the world texture past what the
    /// device allows, which no zoom step asked for.
    fn fit_zoom_to_device(&mut self) {
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
    fn zoom(&mut self, inwards: bool) -> bool {
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
    fn report_limit(&mut self, message: std::fmt::Arguments<'_>) {
        if !self.zoom_limit_reported {
            self.zoom_limit_reported = true;
            eprintln!("{message}");
        }
    }

    /// Redraw from what the server has shown us.
    ///
    /// A projection of the whole [`WorldView`], rebuilt each time rather than
    /// patched: the view is the record of what arrived, and anything kept in
    /// step with it by hand would be a second record that could disagree.
    fn entered(&mut self, view: &WorldView, body: link::Body) {
        // The facet is chosen at startup and `0x1B` names only its size, so a
        // shard serving a different one draws this client the wrong ground with
        // no complaint from either end. Said once, because it is a
        // misconfiguration and not an event.
        if !self.facet_checked {
            self.facet_checked = true;
            if u32::from(view.map.width) != self.map.width()
                || u32::from(view.map.height) != self.map.height()
            {
                eprintln!(
                    "the shard's facet is {}x{} and {} is {}x{}: the ground drawn is not the ground you are standing on",
                    view.map.width,
                    view.map.height,
                    self.map.facet_name(),
                    self.map.width(),
                    self.map.height(),
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
        self.crowd.commanding(me);
        // A rollback is also the one thing that makes `steer.rs`'s idea of which
        // way this body was last sent a lie — it is a step ahead of the shard on
        // purpose, and a refusal is the shard saying that step never happened.
        // Left uncorrected, the step after a `0x21` is decided against a facing
        // nobody has: it is timed as a turn when it is a step, or as a step when
        // it is a turn, and either is a beat of the walk in the wrong place.
        if body.corrected {
            self.steer.corrected(body.predicted.facing.direction);
        }
        self.player = match body.corrected {
            true => self.crowd.snap(
                me,
                body.predicted.position,
                view.player.body,
                body.predicted.facing,
                view.player.hue,
            ),
            false => self.crowd.see(
                me,
                body.predicted.position,
                view.player.body,
                body.predicted.facing,
                view.player.hue,
            ),
        };
        self.player.equipment = crowd::worn(&view.player.equipment, &self.tiledata);
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
        self.items.clear();
        self.item_serials.clear();
        for (serial, item) in items {
            self.items.push(GroundItem {
                at: item.position,
                graphic: item.graphic,
                hue: item.hue,
            });
            self.item_serials.push(*serial);
        }
        // The same list read for a second question — not what to draw, but what
        // a step cannot go through. Rebuilt here rather than per decision: one
        // click plans a route over hundreds of tiles, and each of them would
        // otherwise rescan everything on screen. See `clutter.rs`.
        self.clutter = clutter::Clutter::of(&self.items, &self.tiledata);
        // `cutaway_at` follows the same prediction `player.at` does, with one
        // guard: it only ever advances to a tile the client's own static map
        // agrees is reachable from the one it already held. A correction is
        // the server's own word and is trusted outright, same as `player.at`
        // is; an optimistic step is only trusted here when it is not one
        // `Steering::detour` is going to have offered into a wall this end
        // can already see — see the field's own doc for why.
        self.cutaway_at = match body.corrected {
            true => body.predicted.position,
            false => {
                let terrain = self.clutter.over(&self.map, &self.tiledata);
                match terrain.can_step(self.cutaway_at, body.predicted.position) {
                    Some(_) => body.predicted.position,
                    None => self.cutaway_at,
                }
            }
        };
        // Sorted by serial: a `HashMap`'s order is not one, and an atlas built
        // in a different order every frame is a rebuild every frame.
        let mut others: Vec<_> = view.mobiles.iter().collect();
        others.sort_unstable_by_key(|(serial, _)| serial.raw());
        self.others = others
            .into_iter()
            .map(|(serial, mobile)| {
                let who = Some(*serial);
                let mut drawn = self
                    .crowd
                    .see(who, mobile.position, mobile.body, mobile.facing, mobile.hue);
                drawn.equipment = crowd::worn(&mobile.equipment, &self.tiledata);
                (who, drawn)
            })
            .collect();
        // Whoever the view no longer holds walked out of range, and their clock
        // goes with them. Our own body is kept by its serial like anyone else's;
        // the placeholder's `None` is gone the moment a shard names us, which is
        // right — it was never a mobile.
        self.crowd.retain(|who| {
            who.is_some_and(|serial| serial == view.player.serial || view.mobiles.contains_key(&serial))
        });
        self.connection = format!("in world as 0x{:08X}", view.player.serial.raw());
        // The newest line in the journal, heard once and hung over its
        // speaker's head for a while — compared against the old view, still
        // in `self.view` at this point, so a redraw that changed nothing else
        // does not restart the hold on the same sentence. A system line
        // (`serial: None`) has no mobile to hang over and is left for the
        // HUD's world window instead, which is not built yet.
        if let Some(latest) = view.journal.back() {
            let already_heard = self
                .view
                .as_ref()
                .is_some_and(|previous| previous.journal.back() == Some(latest));
            if !already_heard {
                if let Some(serial) = latest.serial {
                    self.crowd
                        .hear(Some(serial), latest.text.clone(), latest.font, latest.hue);
                }
            }
        }
        // Whole, for the HUD's world window: the three projections above are
        // what the renderer wants, and none of them keeps a serial.
        self.view = Some(Box::new(view.clone()));
        // The camera follows the body, which is what `0x20` is for — unless it
        // has been unlocked, in which case the eye is the mouse's and the body
        // is free to walk off the screen. `Home` puts it back. After the view is
        // stored, because that is what says who we are, and the glide is keyed
        // by it.
        //
        // Zero, for the reason `App::walk_offline` says: a packet is not a
        // frame. The crowd's clock was brought up to date before this fold, so
        // there is no elapsed time left to hand a rig anyway.
        self.follow_player(std::time::Duration::ZERO);
    }

    /// Common code for the two lookups in [`App::pick_tile`]: `unproject` hands
    /// back a signed pair that may be off the map in any direction, and a
    /// negative one is not expressible as the `u16` [`Map::land`] wants.
    fn in_bounds(x: i32, y: i32, map: &Map) -> Option<(u16, u16)> {
        if x < 0 || y < 0 || x as u32 >= map.width() || y as u32 >= map.height() {
            return None;
        }
        Some((x as u16, y as u16))
    }

    /// Everything the Tile panel shows about one tile, read straight from the
    /// map. Shared by the live hover and a click's frozen selection, so the two
    /// can never disagree about what a tile contains.
    fn tile_info(&self, x: u16, y: u16) -> shell::PickedTile {
        let land = self.map.land(x, y);
        let statics = self
            .map
            .statics_at(x, y)
            .map(|item| (item.tile, item.z, item.hue))
            .collect();
        // The height anything drawn *on* this tile belongs at: the surface a body
        // would stand on, not the ground under it. On a pier those are thirteen
        // z-units apart — the land is water at -15 and the planks are at -3 — and
        // a marker drawn at the land's height sits a tile and a half down the
        // screen from the boards it is meant to be lying on, which is what made
        // the cursor unable to hit a pier tile at all. `predict_z` is the same
        // "which surface, coming from here" the walk itself uses, asked from the
        // body's own height so a floor overhead does not win over the street.
        let terrain = openshard_movement::MapTerrain::new(self.map.as_ref(), &self.tiledata);
        let stand = terrain.predict_z(x, y, i32::from(self.player.at.z));
        // Clamped rather than unwrapped: a `z` outside `i8` is a corrupt
        // block, and a diamond at the wrong height beats a panic in a HUD.
        let stand_z = stand.clamp(i32::from(i8::MIN), i32::from(i8::MAX)) as i8;
        // The shape of the surface being marked, and the decision belongs here
        // rather than in the painter: only the map knows whether the height a
        // body stands at is the land's own — in which case the surface is a
        // sloped quad and the marker has to be too — or the flat top of a
        // platform standing on it.
        //
        // `average_land_z` is the same number `predict_z` pushed as the land's
        // candidate, so this is a comparison of one arithmetic against itself
        // rather than a re-derivation. A platform whose deck happens to sit at
        // exactly the land's average height is drawn sloped; it is level ground
        // wherever that coincidence is not one, and a corner off by a unit or
        // two is a better wrong answer than a marker that ignores the hill.
        let corners = match self.map.average_land_z(x, y) == Some(stand_z) {
            // `land_corners` reads top, right, *left*, bottom, and the facet
            // wants top, right, bottom, left — swapping the pair is what keeps
            // the quad from being a bow tie.
            true => match self.map.land_corners(x, y) {
                Some([top, right, left, bottom]) => [top, right, bottom, left],
                None => [stand_z; 4],
            },
            false => [stand_z; 4],
        };
        // The same clamp, and the same reason: a corrupt block may name a height
        // no `i8` holds, and a level drawn at the edge of the world beats a
        // panic in a HUD.
        let drawn_z = |z: i32| z.clamp(i32::from(i8::MIN), i32::from(i8::MAX)) as i8;
        // Whether a body fits is asked of the *cluttered* terrain — the client's
        // map with the shard's items laid over it — because that is what every
        // step decision on this end asks. A private "can I stand here" written
        // for the marker would be a second policy, and the first bug it hid
        // would be one of its own. The surfaces themselves come from the map:
        // where a floor *is* is a fact about the facet, and only whether a body
        // fits on it depends on what has been put there since.
        let cluttered = self.clutter.over(&self.map, &self.tiledata);
        let mut levels: Vec<(i8, bool)> = terrain
            .surfaces(x, y)
            .into_iter()
            .map(|z| {
                let fits = openshard_movement::Terrain::can_fit(
                    &cluttered,
                    openshard_movement::Tile::new(x, y),
                    z,
                    openshard_movement::PLAYER_HEIGHT,
                );
                (drawn_z(z), fits)
            })
            .collect();
        // Sorted so the diagram reads bottom to top, and deduplicated because a
        // tile can carry two statics whose decks land on the same height — two
        // diamonds drawn on one line are one line drawn twice.
        levels.sort_unstable();
        levels.dedup();
        shell::PickedTile {
            x,
            y,
            land: land.map(|cell| cell.tile),
            land_z: land.map_or(0, |cell| cell.z),
            stand_z,
            corners,
            levels,
            ceiling: terrain.ceiling(x, y).map(drawn_z),
            statics,
        }
    }

    /// The eight tiles around one, for the wireframe the HUD draws beside the
    /// marker.
    ///
    /// A box on its own says how high its tile is; a box among its neighbours
    /// says which way the ground *runs*, which is the question actually being
    /// asked while looking for the reason a step was refused or a marker sits
    /// where it does. The ring is what makes the relief readable — a stair's
    /// tread against its riser, a cliff edge one tile from level ground.
    ///
    /// Off the map is simply absent: `checked_add`/`checked_sub` at the world's
    /// corner, and [`Map::land`](openshard_uofiles::map::Map::land) answers
    /// nothing for a block that never loaded, which `tile_info` already reports
    /// as `land: None`.
    ///
    /// Eight tiles and not a radius: each of these costs a `predict_z` and the
    /// statics list under it, per frame, and eight is what a slope needs to be
    /// legible. A wider ring is the terrain overlay's job, and it has one.
    fn tile_ring(&self, centre: &shell::PickedTile) -> Vec<shell::PickedTile> {
        let mut ring = Vec::with_capacity(8);
        for dy in [-1i32, 0, 1] {
            for dx in [-1i32, 0, 1] {
                if (dx, dy) == (0, 0) {
                    continue;
                }
                let x = i32::from(centre.x) + dx;
                let y = i32::from(centre.y) + dy;
                if let Some((x, y)) = Self::in_bounds(x, y, &self.map) {
                    ring.push(self.tile_info(x, y));
                }
            }
        }
        ring
    }

    /// What tile the cursor is over, read straight from the map.
    ///
    /// `unproject` needs the height the pixel is meant to be read at, and the
    /// ground is not flat — so this picks once at the player's height to find
    /// a candidate tile, then re-picks at *that* tile's own height, which is
    /// exact wherever the two tiles agree and wrong only at a slope's edge,
    /// same as the client's own click-to-walk.
    ///
    /// That height is the *surface*, not the land: a pier's planks stand at `-3`
    /// over water at `-15`, and reading the pixel at the water's height resolved
    /// every pier tile to one more than a tile away — the cursor could not be
    /// put on the boards at all, which is what this is written against. The
    /// same `predict_z` the walk uses, so the tile the cursor names and the tile
    /// a step lands on are one answer rather than two.
    ///
    /// `camera` is the frame's own and not `self.control`'s, for the reason
    /// [`App::hud`] takes one: what tile a pixel is over is a question about the
    /// picture being drawn, and reading it from a camera that has moved since is
    /// how the highlight ends up a frame away from the ground under it.
    fn pick_tile(&self, camera: Camera) -> Option<shell::PickedTile> {
        let (cursor_x, cursor_y) = self.control.cursor();
        let world_px = camera.pick(cursor_x, cursor_y);
        let near = i32::from(self.player.at.z);
        let (mut x, mut y) = camera::unproject(world_px, self.player.at.z);
        if let Some((ux, uy)) = Self::in_bounds(x, y, &self.map) {
            let terrain = openshard_movement::MapTerrain::new(self.map.as_ref(), &self.tiledata);
            let z = terrain.predict_z(ux, uy, near);
            let z = z.clamp(i32::from(i8::MIN), i32::from(i8::MAX)) as i8;
            (x, y) = camera::unproject(world_px, z);
        }
        let (x, y) = Self::in_bounds(x, y, &self.map)?;
        Some(self.tile_info(x, y))
    }

    /// What `common/movement` makes of the ground on screen, and the way through
    /// it — the HUD's terrain overlay, gathered only while it is switched on.
    ///
    /// **Not a second opinion about walkability.** Every answer here comes from
    /// the same [`Terrain`] every step decision on this end asks — the client's
    /// map with the shard's items laid over it — so a tile the picture calls
    /// blocked is a tile the walk will refuse. A private "is this passable"
    /// written for the overlay would be a second policy, and the first bug it hid
    /// would be one of its own.
    ///
    /// Passability is asked per *tile* and not per step: `spawn_z` finds the
    /// surface a body would stand on regardless of how far that is from the
    /// player's own height — so a building's upper floor reads open from the
    /// street rather than blocked — and `can_fit` is what says nothing solid is
    /// standing in the body's space there, the clutter included.
    ///
    /// The route is the plan being walked, if there is one. When there is not,
    /// it is the plan that *would* be walked to the tile under the cursor, which
    /// is the question actually being asked while dragging the mouse over a
    /// building looking for the way in. One [`find_path`] per frame, and only
    /// while the overlay is on.
    /// How much of the occlusion grid the two views of it draw this frame.
    ///
    /// The one place [`App::solids_everything`] — what the person picked — and
    /// the player's own `z` — what this frame is — are joined, so that no
    /// stale height can be stored anywhere and the wireframe and the solids
    /// pass cannot be cut differently. See
    /// [`solid::Cut`](openshard_client_render::solid::Cut).
    fn solid_cut(&self) -> openshard_client_render::solid::Cut {
        use openshard_client_render::solid::Cut;

        match self.solids_everything {
            true => Cut::Nothing,
            false => Cut::BelowFeet(self.player.at.z),
        }
    }

    fn terrain_overlay(&self, camera: Camera, hover: Option<&shell::PickedTile>) -> shell::TerrainOverlay {
        use openshard_movement::{PLAYER_HEIGHT, Tile, find_path, step_allowed};

        let terrain = self.clutter.over(&self.map, &self.tiledata);
        let near = i32::from(self.player.at.z);
        let mut open = Vec::new();
        let mut blocked = Vec::new();
        // The same clamp the ground pass uses, so the wash covers exactly the
        // tiles that were drawn and no strip of it hangs off the map.
        if let Some((xs, ys)) = camera
            .visible_tiles()
            .clamp_to(self.map.width(), self.map.height())
        {
            for y in ys {
                for x in xs.clone() {
                    let tile = Tile::new(x, y);
                    // The height the diamond is drawn at, and the height the
                    // question is asked about, are one number — the surface a
                    // body would stand on here. A *blocked* tile has one too:
                    // the barrels on a pier stand on the planks, and washing
                    // their tile at the land's height (water, thirteen units
                    // down) drew the refusal a tile and a half away from the
                    // barrel that caused it. `ground_z` is only the fallback for
                    // a tile with no surface at all.
                    let surface = terrain.spawn_z(tile, near);
                    // `clamp` rather than `unwrap`: a `z` outside `i8` is a
                    // corrupt block and not an invariant of ours, and a diamond
                    // drawn at the wrong height is a better answer than a panic
                    // in a debugging overlay.
                    let drawn_z = |z: i32| z.clamp(i32::from(i8::MIN), i32::from(i8::MAX)) as i8;
                    match surface.filter(|&z| terrain.can_fit(tile, z, PLAYER_HEIGHT)) {
                        Some(z) => open.push(Point { x, y, z: drawn_z(z) }),
                        None => blocked.push(Point {
                            x,
                            y,
                            z: surface.map_or_else(|| terrain.ground_z(tile).unwrap_or(0), drawn_z),
                        }),
                    }
                }
            }
        }

        // Directions, from wherever they come from, walked out into the tiles
        // they land on — `step_allowed` because it is what corrects a step's `z`
        // to the surface it lands on, which is the height the marker is drawn at.
        let mut steps: Vec<Direction> = self.steer.route().collect();
        if steps.is_empty() {
            if let Some(tile) = hover {
                // The surface, like everything else here: a route planned to the
                // water under a pier is a route to somewhere nobody is standing.
                let to = Point {
                    x: tile.x,
                    y: tile.y,
                    z: tile.stand_z,
                };
                steps = find_path(&terrain, self.player.at, to, steer::PLAN_BUDGET).unwrap_or_default();
            }
        }
        let mut at = self.player.at;
        let mut route = vec![at];
        for direction in steps {
            let Some(next) = step_allowed(&terrain, at, direction) else {
                // The plan and the ground disagree, which is a thing worth
                // seeing rather than papering over: the line stops where they
                // parted company.
                break;
            };
            at = next;
            route.push(at);
        }

        shell::TerrainOverlay { open, blocked, route }
    }

    /// Do what the HUD asked for on the frame before this one.
    ///
    /// Every writer the shell has, in one place and at one moment: the top of a
    /// frame, before anything reads. See [`App::pending`] for why it is a frame
    /// late and why that is the point rather than a compromise.
    ///
    /// The viewport is deliberately not in here. It is not something a widget
    /// *asked* for — it is what the layout left over, which `Shell` holds between
    /// frames — and it is applied beside this call rather than through it.
    fn apply(&mut self, request: shell::Request) {
        if request.relock {
            self.relock();
        } else if request.unlock {
            self.control.unlock();
        }
        if let Some(rig) = request.rig {
            // The eye does not move — that is what `set_rig` promises — but the
            // frames before the swap were flown by another camera, and measuring
            // them together would average two rigs.
            self.control.set_rig(rig);
            self.scope.clear();
        }
        // The body's ease is not the rig and does not clear the scope: the frames
        // either side of it were flown by the same camera, and what the scope
        // measures is the eye against the body it was given.
        if let Some(ease) = request.ease {
            self.crowd.set_ease(ease);
        }
        if let Some(show) = request.show_terrain {
            self.show_terrain = show;
        }
        if let Some(show) = request.show_occluders {
            self.show_occluders = show;
        }
        if let Some(show) = request.show_solids {
            self.show_solids = show;
        }
        if let Some(only) = request.solids_only {
            self.solids_only = only;
        }
        if let Some(opaque) = request.solids_opaque {
            self.solids_opaque = opaque;
        }
        // The variant and not the `z` in it: what the person picked holds across
        // frames, and the height they were standing at when they picked it is
        // this frame's business — see [`App::solid_cut`].
        if let Some(cut) = request.solid_cut {
            self.solids_everything = matches!(cut, openshard_client_render::solid::Cut::Nothing);
        }
        if let Some(target) = request.highlight {
            self.highlight = target;
        }
        if let Some(style) = request.highlight_style {
            self.highlight_style = style;
        }
        // The window the metrics are taken over, and not a clear: the frames
        // already held were flown by the same rig.
        if let Some(span) = request.scope_span {
            self.scope.set_span(span);
        }
        match request.script {
            Some(shell::ScriptRequest::Run(name)) => self.start_replay(name),
            Some(shell::ScriptRequest::Stop) => self.replay = None,
            None => {}
        }
        if let Some(line) = request.say {
            self.say(line);
        }
        if let Some(reply) = request.gump {
            self.answer_gump(reply);
        }
    }

    /// What the panels are allowed to know, gathered each frame.
    ///
    /// `camera` is the frame's own, handed in rather than read back from
    /// [`App::control`]: the overlay the shell draws from this and the world pass
    /// below it are two readers of one picture, and the only way they cannot
    /// disagree is for there to be one value. See [`App::draw`].
    /// Whether the world may read the cursor at all.
    ///
    /// Asked once and answered for the whole frame. A pointer over a panel picks
    /// no tile and lights no item, so nothing is highlighted under the panel and
    /// nothing is highlighted where the pointer *was* when it went over one; a
    /// pointer that has left the window is the other half, and the one no egui
    /// state can answer — see [`App::pointer_inside`] and
    /// [`shell::Shell::holds_pointer`].
    fn world_owns_pointer(&self) -> bool {
        self.pointer_inside && !self.shell.as_ref().is_some_and(shell::Shell::holds_pointer)
    }

    /// `lit_item` and `lit_mobile` are what [`items::pick`] and
    /// [`mobiles::pick`] answered for this frame, handed in
    /// rather than asked again: the HUD and the world passes are two readers of
    /// one picture, and the tile marker is drawn or not drawn on the strength of
    /// whether an item took the highlight. Asking twice would be two answers to
    /// "what is the cursor on", and the frame where they disagree is the frame a
    /// barrel is ringed *and* the ground under it is diamonded.
    ///
    /// `cutaway` is handed in for the third reader of that same rule: the
    /// occluder overlay draws the grid the frame's lighting is about to build,
    /// and a grid built from a second cutaway would draw boxes for the storey
    /// this frame took away.
    fn hud(
        &self,
        camera: Camera,
        lit_item: Option<usize>,
        lit_mobile: Option<usize>,
        on_static: Option<PickedStatic>,
        cutaway: &Cutaway,
    ) -> shell::Hud {
        let hover = match self.world_owns_pointer() {
            true => self.pick_tile(camera),
            false => None,
        };
        let neighbours = hover.as_ref().map_or_else(Vec::new, |tile| self.tile_ring(tile));
        let (mobiles, items) = match self.view.as_ref() {
            Some(view) => {
                let mut mobiles: Vec<_> = view
                    .mobiles
                    .iter()
                    .map(|(serial, mobile)| (serial.raw(), mobile.body.0, mobile.position))
                    .collect();
                // Sorted, so a `HashMap`'s iteration order does not reshuffle
                // the list under the reader's eyes every frame.
                mobiles.sort_unstable_by_key(|(serial, _, _)| *serial);
                let mut items: Vec<_> = view
                    .items
                    .iter()
                    .map(|(serial, item)| (serial.raw(), item.graphic.0, item.position))
                    .collect();
                items.sort_unstable_by_key(|(serial, _, _)| *serial);
                (mobiles, items)
            }
            None => (Vec::new(), Vec::new()),
        };
        shell::Hud {
            ease: self.crowd.ease(),
            connection: self.connection.clone(),
            serial: self.view.as_ref().map(|view| view.player.serial.raw()),
            position: self.player.at,
            camera,
            locked: self.control.follow() == Follow::Body,
            rig: self.control.rig(),
            readings: bench::readings(self.scope.samples()),
            // Two frames is one difference and no derivative of it. Absent
            // rather than a zero, which would read as "the eye was perfectly
            // smooth" on the frame the window opened.
            metrics: (self.scope.samples().len() > 2).then(|| Metrics::of(self.scope.samples())),
            scope_span: self.scope.span(),
            frames: self.frames.frames().to_vec(),
            frames_span: self.frames.span(),
            worst_fps: self.frames.worst_fps(),
            repacks: self.repacks,
            // What is currently *asking* for frames, which is the other half of
            // any answer about the frame rate: a picture drawn every 80ms is not
            // a slow frame if the loop is on the animation clock, it is a frame
            // nobody asked for sooner.
            pacing: self.pacing(),
            scripts: self.scripts.iter().map(|script| script.name).collect(),
            replay: self.replay.as_ref().map(|replay| {
                let length = replay.length().as_secs_f32().max(0.001);
                (replay.name(), replay.at().as_secs_f32() / length)
            }),
            offline: self.link.is_none(),
            mobiles,
            items,
            show_terrain: self.show_terrain,
            // The tile is lit when nothing else took the highlight. Under
            // `Items` nothing ever does, which is the mode's whole content; the
            // ground is still hovered and the panel still reads it.
            hover_lit: match self.highlight {
                // The map's own furniture counts here as much as an item does,
                // and it is the case this rule was missing: a wall under the
                // cursor is what a click takes, so a diamond drawn on the ground
                // *behind* it — which is where the cursor unprojects to, a wall
                // being taller than the cell it stands on — is the client
                // pointing at two tiles at once. That is the disagreement this
                // arm exists to stop, and it had one more source than it knew.
                shell::HighlightTarget::Auto => {
                    lit_item.is_none() && lit_mobile.is_none() && on_static.is_none()
                }
                shell::HighlightTarget::Items => false,
                shell::HighlightTarget::Tiles => true,
            },
            lit_mobile,
            lit_item,
            lit_static: on_static,
            highlight: self.highlight,
            highlight_style: self.highlight_style,
            terrain: self
                .show_terrain
                .then(|| self.terrain_overlay(camera, hover.as_ref())),
            show_occluders: self.show_occluders,
            show_solids: self.show_solids,
            solids_only: self.solids_only,
            solids_opaque: self.solids_opaque,
            solid_cut: self.solid_cut(),
            solids: (self.solids_held, self.solids_drawn),
            // The grid the lighting will build a few lines later in the same
            // frame, built here a second time rather than kept from the last
            // one: the HUD is drawn before the world passes, and a wireframe a
            // frame behind the picture it is a claim about slides off every wall
            // as the camera pans — which is the one artefact an instrument for
            // finding misplaced occluders must not have.
            //
            // `light::lit_tiles`, not `camera.visible_tiles`: the grid is grown
            // by the widest pool's reach, and a box drawn over a rectangle the
            // shader did not walk would be a picture of this overlay's own
            // bounds rather than of the lighting's.
            occluders: self.show_occluders.then(|| {
                occlusion::collect(
                    &self.map,
                    &self.items,
                    light::lit_tiles(&camera),
                    &self.tiledata,
                    cutaway,
                    // The same atlas the frame's own grid is built from, or the
                    // wireframe would draw boxes the shader does not have.
                    self.window.as_ref().map(|window| &window.atlases.statics),
                )
            }),
            hover,
            neighbours,
            selected: self.selected_tile.map(|(x, y)| self.tile_info(x, y)),
            selected_static: self.selected_static,
            goal: self.steer.goal().map(|(x, y)| self.tile_info(x, y)),
            gumps: self
                .view
                .as_ref()
                .map(|view| view.gumps.clone())
                .unwrap_or_default(),
            said: self
                .view
                .as_ref()
                .map(|view| {
                    view.journal
                        .iter()
                        .rev()
                        .take(SPEECH_LINES)
                        .rev()
                        .map(|line| match line.name.is_empty() {
                            true => line.text.clone(),
                            false => format!("{}: {}", line.name, line.text),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// Everyone to draw, each beside the serial their clock is keyed by.
    ///
    /// Our own body first, and `None` for it while no shard has named us.
    ///
    /// The group is refreshed from the crowd here and not in
    /// [`App::advance_to_clocks`] alone, because this list is what *packs* the
    /// atlas as well as what draws from it — see [`App::wanted_in`]. `self.player`
    /// and `self.others` hold the group as of the last packet, and
    /// [`Crowd::advance`] changes it without one: a body that walked into view
    /// and then stopped is drawn standing while the packet-time list still says
    /// walking. Pack one group and draw another and [`mobiles::place`] finds no
    /// frame, so the body simply vanishes — and stays vanished for as long as it
    /// stands still, there being no further packet to correct the list with.
    fn drawn_mobiles(&self) -> Vec<(Who, Mobile)> {
        Self::everyone_drawn(&self.crowd, self.me(), &self.player, &self.others)
    }

    /// [`App::drawn_mobiles`] over the four fields it reads, so a test can build
    /// the list the atlases are grown from without a window, a device or a
    /// shard.
    fn everyone_drawn(
        crowd: &Crowd,
        me: Who,
        player: &Mobile,
        others: &[(Who, Mobile)],
    ) -> Vec<(Who, Mobile)> {
        let mut mobiles = Vec::with_capacity(others.len() + 1);
        mobiles.push((me, player.clone()));
        mobiles.extend_from_slice(others);
        Self::advance_groups(crowd, &mut mobiles);
        mobiles
    }

    /// Refresh each body's animation group from the crowd's clock.
    ///
    /// Split out of [`App::advance_to_clocks`] because the group is the one part
    /// of a mobile that has to be right *before* the atlases are grown, and the
    /// growth happens with no atlas to ask for a frame count. Both paths go
    /// through here so there is one statement of "which group is playing".
    fn advance_groups(crowd: &Crowd, drawn: &mut [(Who, Mobile)]) {
        for (who, mobile) in drawn.iter_mut() {
            // `Crowd::advance` drops a walking body to standing on its own
            // timer, with nothing that looks like a packet to refresh
            // `mobile.group` from — a group read once and left stale plays the
            // walking sprite for ever, timed by a clock that has moved on to
            // the standing group's.
            if let Some(group) = crowd.group_for(*who) {
                mobile.group = group;
            }
        }
    }

    /// Fill in the three time-varying halves of every mobile from the crowd's
    /// clocks: which group is playing, which frame of it, where the body is
    /// drawn, and which tile the step is sorted at.
    ///
    /// An associated function taking the two fields it reads rather than a
    /// method, because both callers hold a borrow of one of `App`'s fields
    /// while they ask: the frame holds `self.window` mutably, and the pick
    /// holds it shared. A `&self` method would borrow all of `App` and neither
    /// could call it.
    ///
    /// `atlas` is asked for the frame *count*: a group's length is the
    /// animation's, and taking it from anywhere else makes "frame 7 of a
    /// 6-frame walk" expressible. Under the body the atlas packed — for a ghost
    /// the living body it borrows its pictures from — or a ghost counts zero
    /// frames, lands on frame 0 for ever and slides along standing still.
    fn advance_to_clocks(crowd: &Crowd, atlas: &AnimAtlas, drawn: &mut [(Who, Mobile)]) {
        // The group is read back first and not only the frame and the glide —
        // the frame count below is asked *under* it. Idempotent when the caller
        // is [`App::drawn_mobiles`], which is every caller today; here so this
        // function is right on its own terms rather than on its callers'.
        Self::advance_groups(crowd, drawn);
        for (who, mobile) in drawn.iter_mut() {
            let (direction, _) = openshard_uofiles::anim::facing(mobile.facing);
            let frame_count = atlas.frame_count(
                openshard_uofiles::anim::animation_body(mobile.body),
                mobile.group,
                direction,
            );
            mobile.frame = crowd.frame_for(*who, frame_count);
            if let Some(at) = crowd.drawn_for(*who) {
                mobile.drawn = at;
            }
            // And which tile it sorts at, which is a step's own clock too: the
            // crossing ends without a packet to say so, and a body still sorted
            // on the tile it left would keep drawing over the ground behind it.
            mobile.from = crowd.stepping_from(*who);
        }
    }

    /// Everyone as they are drawn *this instant*, clocks and all — the list
    /// [`mobiles::pick`] and [`mobiles::collect`] both index into.
    ///
    /// Built twice a frame, once for the pick and once for the picture, rather
    /// than threaded between them: the two happen either side of the atlas
    /// growth and of a mutable borrow of the window, and the work is a handful
    /// of map lookups over whoever is on screen. What matters is that the
    /// *order* is [`App::drawn_mobiles`]'s both times, so an index answered by
    /// the pick still names the same creature to the passes below.
    fn drawn_now(&self, atlas: &AnimAtlas) -> Vec<(Who, Mobile)> {
        let mut drawn = self.drawn_mobiles();
        Self::advance_to_clocks(&self.crowd, atlas, &mut drawn);
        drawn
    }

    fn create_window(&mut self, event_loop: &ActiveEventLoop) -> Result<Screen, StartupError> {
        // Physical pixels, not logical: a `LogicalSize` here would ask for the
        // same *point* size on every monitor and come out small on a dense
        // one, exactly backwards from what "respect the density" means. Sized
        // off the monitor rather than the `Camera` default (1024x768, meant as
        // a viewport floor, not a window request) so the window opens large on
        // whatever screen it is on.
        let attributes = Window::default_attributes().with_title("OpenShard");
        // Where the last run left it, when there was one and when it still names
        // a screen that exists. The monitors are asked *now*, from the event
        // loop, because a laptop undocked since the last run has a saved frame
        // that opens the window on a monitor nobody has — offscreen, which looks
        // exactly like a client that failed to start. See `Desk::fits`.
        let monitors: Vec<_> = event_loop
            .available_monitors()
            .map(|monitor| {
                let position = monitor.position();
                let size = monitor.size();
                (position.x, position.y, size.width, size.height)
            })
            .collect();
        let restored = self
            .desk
            .window
            .filter(|frame| desk::Desk::fits(frame, &monitors));
        let attributes = match restored {
            Some(frame) => attributes
                .with_position(winit::dpi::PhysicalPosition::new(frame.x, frame.y))
                .with_inner_size(winit::dpi::PhysicalSize::new(
                    frame.width.max(1),
                    frame.height.max(1),
                ))
                .with_maximized(frame.maximized),
            // No saved frame: the first run, or one whose screen is gone.
            None => match event_loop.primary_monitor().map(|monitor| monitor.size()) {
                Some(size) if size.width > 0 && size.height > 0 => {
                    attributes.with_inner_size(winit::dpi::PhysicalSize::new(
                        (size.width as f32 * 0.9) as u32,
                        (size.height as f32 * 0.9) as u32,
                    ))
                }
                _ => attributes.with_inner_size(winit::dpi::LogicalSize::new(
                    self.control.camera().width,
                    self.control.camera().height,
                )),
            },
        };
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .map_err(StartupError::Window)?,
        );
        // Without this, the compositor never starts an IME session for this
        // window, and on Wayland that is what feeds `egui-winit` composed
        // text: a layout that needs one (Cyrillic under a caps-lock layout
        // switch, an East Asian input method) either loses every keystroke or
        // the raw keysym instead of the composed character, silently, while a
        // plain Latin layout still works because it needs no composition —
        // the shell's "say" box looked fine to type in for exactly that
        // reason and nothing else.
        window.set_ime_allowed(true);

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance
            .create_surface(Arc::clone(&window))
            .map_err(StartupError::Surface)?;

        // Blocking here is fine on the desktop and would not be in a browser,
        // where this whole function becomes an `async` one driven by the event
        // loop. Nothing below cares which way it was awaited.
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .map_err(|error| StartupError::NoDevice(error.to_string()))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .map_err(|error| StartupError::NoDevice(error.to_string()))?;

        let capabilities = surface.get_capabilities(&adapter);
        // A non-sRGB format, deliberately: `client/render` writes the art's own
        // bytes and an sRGB surface would gamma-correct them into something
        // else. See that crate's docs.
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| !format.is_srgb())
            .ok_or(StartupError::OnlySrgb)?;

        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            // `Auto` is the only value guaranteed for every format, and it means
            // "whatever the format says" — which for a non-sRGB format is the
            // pass-through this renderer needs.
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: size.width.max(1),
            height: size.height.max(1),
            // Named, and not `present_modes[0]`. This is the loop's pacer: a
            // frame is drawn, `request_redraw` asks for the next one at once,
            // and what makes that a rate rather than a spin is `get_current_texture`
            // blocking here until the display has taken the last one. Whatever
            // the adapter happened to offer first is `Mailbox` on some drivers
            // and `Immediate` on others — neither of which blocks, so the same
            // code is a 60Hz walk on one machine and a busy loop at a thousand
            // frames a second on the next. `Fifo` is the one mode `wgpu`
            // guarantees on every backend, which is why it can be asked for
            // outright rather than searched for.
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: capabilities.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        // How far the zoom may be walked out. Asked once, because it is a
        // property of the device and not of the frame.
        self.control
            .set_max_texture(device.limits().max_texture_dimension_2d);
        self.control.resize(config.width, config.height);

        let wanted = self.wanted_now();
        let atlases = Atlases::build(
            &self.art,
            self.surfaces.as_ref(),
            &self.texmaps,
            &self.tiledata,
            &mut self.anim,
            &wanted,
        )
        .map_err(StartupError::Atlas)?;
        // What the atlases were built for, which is what the band walk in
        // `draw` subtracts from on the next frame.
        self.covered = Some(self.control.camera().visible_tiles());
        // The world passes draw into the world texture, so they take *its*
        // format and not the surface's — the two differ on an HDR display,
        // where the first non-sRGB surface format is `Rgba16Float`.
        let renderer = GroundRenderer::new(
            &device,
            &queue,
            blit::WORLD_FORMAT,
            &atlases.land,
            &atlases.texmaps,
        );
        let statics = SpriteRenderer::new(
            &device,
            &queue,
            blit::WORLD_FORMAT,
            atlases.statics.pixels(),
            &self.hue_ramp,
        );
        let mobile_pass = SpriteRenderer::new(
            &device,
            &queue,
            blit::WORLD_FORMAT,
            atlases.mobiles.pixels(),
            &self.hue_ramp,
        );
        // No atlas and no format: this pass writes only place and the shared
        // depth buffer, so it does not need rebuilding here on every atlas
        // repack the way `statics`/`mobile_pass` do.
        let mesh_pass = MeshFaceRenderer::new(&device);
        // Built once, unlike `statics` and `mobile_pass`: `font_atlas` is never
        // rebuilt, so neither is what draws it.
        let text_pass = SpriteRenderer::new(
            &device,
            &queue,
            blit::WORLD_FORMAT,
            self.font_atlas.pixels(),
            &self.hue_ramp,
        );
        // Scaled by the window's own density: a `TtfAtlas` bakes one pixel
        // size into every glyph it packs (see its doc), so the size has to be
        // picked once, here, where a real `Window` first exists to ask —
        // `run` cannot ask before one does, and rebuilding a size already
        // packed at is exactly the "ten faces" cost `ttf_font`'s doc explains
        // this engine does not pay.
        let (ttf_atlas, ttf_pass) = match &self.ttf_font {
            Some(_) => {
                let atlas = TtfAtlas::empty(TTF_BASE_PIXEL_HEIGHT * window.scale_factor() as f32);
                let pass = SpriteRenderer::new(
                    &device,
                    &queue,
                    blit::WORLD_FORMAT,
                    atlas.pixels(),
                    &self.hue_ramp,
                );
                (Some(atlas), Some(pass))
            }
            None => (None, None),
        };
        // The world is drawn at 1:1 into a texture of the camera's render size,
        // which is the viewport only at zoom 1 — see `client/render`'s `blit`.
        let world = blit::world_texture(
            &device,
            self.control.camera().render_width(),
            self.control.camera().render_height(),
        );
        let depth = renderer::depth_texture(
            &device,
            self.control.camera().render_width(),
            self.control.camera().render_height(),
        );
        let outline_mask = outline::mask_texture(
            &device,
            self.control.camera().render_width(),
            self.control.camera().render_height(),
        );
        // The selection's own, at the same size and in the same format: it is a
        // colour attachment of the same silhouette pass, sharing the same depth
        // buffer, so it can be neither larger nor smaller than the world image.
        let select_mask = outline::mask_texture(
            &device,
            self.control.camera().render_width(),
            self.control.camera().render_height(),
        );
        let place = place::texture(
            &device,
            self.control.camera().render_width(),
            self.control.camera().render_height(),
        );
        let blit = Blit::new(&device, format);
        // The surface's format and not the world's: the ring is drawn over the
        // blit's output, so that a highlight is not dimmed by the night the way
        // the picture under it is.
        let outline = Outline::new(&device, format);
        // And the selection's wash, over the same finished picture and for the
        // same reason: what is held must stay legible after dark.
        let select = Select::new(&device, format);
        // The occlusion grid as solids — `docs/lighting.md` step 23.0. Over the
        // lit picture for the third time and for the third statement of the same
        // reason: a diagnostic that dimmed at night would stop working exactly
        // when the picture is hardest to read.
        let solids = SolidsRenderer::new(&device, format);
        // And the interface's, bound to the surface's format for the same
        // reason: a gump is drawn on the finished picture, and the night that
        // dimmed the world has already been applied to it.
        let gump_pass = self
            .gumps
            .as_ref()
            .map(|_| GumpRenderer::new(&device, &queue, format, self.gump_atlas.pixels(), &self.hue_ramp));
        // The HUD, with the surface's own format: egui picks its fragment entry
        // point from whether that format is sRGB, and this one deliberately is
        // not.
        self.shell = Some(shell::Shell::new(&device, format, &window, self.desk.clone()));

        Ok(Screen {
            window,
            surface,
            device,
            queue,
            config,
            renderer,
            statics,
            world,
            blit,
            depth,
            place,
            mobile_pass,
            mesh_pass,
            atlases,
            text_pass,
            ttf_atlas,
            ttf_pass,
            outline_mask,
            outline,
            select_mask,
            select,
            solids,
            gump_pass,
        })
    }

    /// Everything on screen right now, whatever the atlases already hold.
    ///
    /// The whole-viewport walk, which is what a rebuild needs and what an
    /// ordinary frame must not do: [`App::wanted_since`] is the frame's version
    /// of the same question and walks only the band the camera crossed.
    fn wanted_now(&self) -> Wanted {
        self.wanted_in([self.control.camera().visible_tiles()])
    }

    /// What the camera has walked onto since `covered` was the visible
    /// rectangle, plus everything that is not a question about the map at all.
    ///
    /// The saving this whole arrangement is for. A frame used to walk the
    /// visible rectangle twice — once for the land graphics and once for the
    /// statics — purely to ask whether the atlases were still good for it, which
    /// is ~9,800 cells at 1080p against a camera that had moved one tile. The
    /// bands [`TileBounds::difference`] hands back are that tile's worth of
    /// cells.
    ///
    /// The invariant it rests on: every cell inside `covered` has already been
    /// offered to the atlases, and an atlas never forgets what it was offered —
    /// not even a graphic the client ships no art for. So a graphic can only be
    /// new outside `covered`, and anything that *does* make an atlas forget has
    /// to set `covered` back to `None` in the same breath.
    ///
    /// `camera` is the frame's snapshot — see [`App::hud`]. What the atlases are
    /// grown for has to be what the passes below then draw, or a band is packed
    /// for one rectangle and sampled for another.
    fn wanted_since(&self, camera: Camera, covered: Option<TileBounds>) -> Wanted {
        let bounds = camera.visible_tiles();
        let bands = match covered {
            Some(covered) => bounds.difference(covered),
            None => [Some(bounds), None, None, None],
        };
        self.wanted_in(bands.into_iter().flatten())
    }

    /// The graphics on some set of tiles, and everything that is on screen
    /// regardless of where the camera is.
    ///
    /// Items the server has dropped and the bodies walking about are short lists
    /// held in memory, so they are asked in full however small the bands are —
    /// an item that arrives while the camera stands still is on no band at all.
    /// They go into the *static* set deliberately: one atlas serves the map's
    /// statics and the server's items, because a floor tile packed twice is a
    /// floor tile twice.
    fn wanted_in(&self, bands: impl IntoIterator<Item = TileBounds>) -> Wanted {
        let drawn: Vec<Mobile> = self
            .drawn_mobiles()
            .into_iter()
            .map(|(_, mobile)| mobile)
            .collect();
        wanted_in(
            &self.map,
            bands,
            &self.items,
            &drawn,
            &self.tile_animations,
            &self.equip_conv,
        )
    }

    fn draw(&mut self) {
        let started = Instant::now();
        // What the shard has opened, and what it has taken away: the view is
        // filled by `client/net`, which knows nothing about screens, so a
        // window appearing is this end noticing.
        self.sync_own_windows();
        // The animation clock moves here, at the top of the frame that is about
        // to show its answer — not when the timer that asked for this frame
        // fired.
        //
        // A glide is a position read off a clock, so the moment that clock is
        // read has to be the moment the picture is built or the walk judders:
        // the timer fires, the loop then lays out the UI, grows an atlas and
        // waits on the swapchain, and however long that took is error in the
        // body's position — error that varies frame to frame, which is exactly
        // what an eye reads as a stutter. It also puts the sampling back in step
        // with the display: `WaitUntil` is a floor, the timer's 16ms beats
        // against a 60Hz refresh, and a frame drawn from the previous tick's
        // clock lands on the wrong side of that beat every second or so.
        //
        // Whatever really passed — see `App::last_advance`. A stall longer than
        // a frame, the window minimised or the machine asleep, moves the clock
        // the whole way rather than queuing a burst of catch-up frames for time
        // nobody watched: a body that was walking through it has long since
        // arrived.
        let elapsed = started.saturating_duration_since(self.last_advance);

        // # The frame is three steps, and this is the first of them
        //
        // Everything that writes runs here, before anything reads. What the
        // shell asked for last frame, then every clock, then the eye — and after
        // this block nothing in the frame moves the world or the camera again.
        //
        // The defect it is written against: the HUD used to be built at the top
        // of the frame and the eye moved a few lines further down, so the
        // overlay egui laid out — the tile highlight, the hover, the walk goal —
        // was drawn against the *previous* frame's camera while the world pass
        // below drew from this one's. The gap between them is one frame of camera
        // motion, which is not a constant: it is whatever the display gave this
        // frame, so the markers shivered against the ground they were meant to be
        // lying on, and every missed interval made them jump. Reordering two
        // calls would have fixed today's version of it and left the shape that
        // produced it, which is a second reader picking the camera up at a
        // different moment. So the frame is staged instead, and the snapshot
        // below is what both readers are handed.
        let asked = std::mem::take(&mut self.pending);
        self.apply(asked);
        // The viewport the last frame's layout left free — `Shell` holds it
        // between frames for exactly this. It has to be settled before the eye
        // is, because it is what decides how much world a camera can see.
        if let Some(shell) = self.shell.as_ref() {
            let viewport = shell.viewport();
            self.control.resize(viewport.width, viewport.height);
        }
        self.crowd.advance(elapsed);
        // The statics that move on their own, on the same span as everybody
        // else. Its own clock inside — a fire's cycle has nothing to do with a
        // walk's — and one *sample*, which is the whole rule: two clocks read
        // from two `Instant::now()`s a few hundred microseconds apart would put
        // a torch and the body that walks past it on two different instants.
        self.tile_animations.advance(elapsed);
        // And the flames, off the same span: a fire's animation frame and the
        // brightness of the pool it casts are two clocks describing one fire,
        // and they are advanced together or they describe two.
        self.flame_clock += elapsed;
        self.last_advance = started;
        // Whatever scenario is being walked delivers its knots for the span that
        // just passed, before the eye is asked where the body is: a step that
        // arrived this frame is one the camera has to answer this frame.
        self.advance_replay(elapsed);
        // A viewport that grew may have taken the world texture past what the
        // device allows, which no zoom step asked for.
        self.fit_zoom_to_device();
        // And the eye goes where the body is *this frame*: a step arrives once
        // and is then walked across for the next 400ms, so every frame in
        // between has a different answer.
        self.follow_player(elapsed);

        // # Step two: one snapshot, and it is a value
        //
        // The camera the whole frame is built from, copied out rather than read
        // back from `self.control` at each use. A `&Camera` handed to five
        // collectors is five reads of a field that something between them might
        // have moved — which is the defect above, expressed as a borrow. A
        // `Camera` is `Copy`, so this costs nothing and cannot be stale in one
        // place and fresh in another.
        let camera = *self.control.camera();

        // Read before the window is borrowed below, for the same reason the line
        // above is: the borrow is of `self`, and the pacing at the foot of this
        // frame is a fact about the whole app rather than about it.
        let watched = self.watched();
        // The same, for the two the item highlight needs — both are questions
        // about the whole of `self` and are asked once, here.
        let owns_pointer = self.world_owns_pointer();
        let cursor = self.control.cursor();

        // What this frame does not draw, read once from the tile the player is
        // standing on. Once, and from the *player's* tile rather than the
        // camera's: a free camera looking at a rooftop three streets away has
        // not walked indoors, and the client's rule is about where the body is.
        // See `openshard_client_render::cutaway`.
        //
        // `self.cutaway_at`, not `self.player.at`: the latter is this end's
        // own unconfirmed prediction, which for one frame can be a tile a
        // held direction was refused on — see the field's own doc.
        //
        // Here, in the snapshot, and not beside the passes that draw from it:
        // the item pick below needs it, and the pick has to be answered before
        // the HUD is built — see the next paragraph.
        let cutaway = Cutaway::at(&self.map, &self.tiledata, self.cutaway_at, true);
        // What the cursor is over, asked here rather than remembered from the
        // last click: the picture moves under a still mouse — the body walks,
        // the camera follows, a door swings — so where the cursor is pointing is
        // a question about *this* frame's picture and has to be asked against
        // this frame's camera. The same `items::pick` a double-click asks, so
        // what is lit is what would be used.
        //
        // Asked once and answered to three readers: the hue the picture is drawn
        // in, the silhouette the ring is grown from, and whether the HUD marks
        // the tile under the cursor at all. Two picks would be two chances to
        // disagree about what the cursor is on, and the visible form of that
        // disagreement is a barrel ringed with the ground under it diamonded.
        //
        // Against the atlas as it stands *before* this frame grows it, which is
        // the one thing given up by asking this early. An item that came on
        // screen this very frame has no sprite packed yet and so no rectangle to
        // be pointed at, and is pickable a frame later; the alternative was a
        // tile marker that decides whether to draw itself from the previous
        // frame's answer, which flickers along every item's edge.
        // **The picks are the frame's *facts*, and the mode decides only what is
        // drawn from them.** They used to be skipped under
        // `HighlightTarget::Tiles`, which folded two questions into one field:
        // "what is the cursor on" and "what may light up". A click reads the
        // first — see the `MouseInput` arm — so with the two folded together a
        // player who had pinned the highlight to tiles could not select a wall at
        // all, and the reason was invisible. The mode is applied to `lit_*`
        // below instead, where it is about lighting and nothing else.
        //
        // Creatures are asked first and they win: a mobile stands *on* the
        // clutter of its tile — it is sorted above whatever is lying there, and
        // it is what a player pointing at a shopkeeper standing on a rug means.
        // Then the server's items, then the map's own furniture. One chain, and
        // every later question is asked only where the earlier ones found
        // nothing — so "what is under the cursor" has exactly one answer and the
        // ring, the wash, the tile marker and the click cannot disagree about it.
        let on_mobile = match owns_pointer {
            true => self.window.as_ref().and_then(|window| {
                mobiles::pick(
                    &self
                        .drawn_now(&window.atlases.mobiles)
                        .into_iter()
                        .map(|(_, mobile)| mobile)
                        .collect::<Vec<_>>(),
                    &camera,
                    &window.atlases.mobiles,
                    &cutaway,
                    &self.equip_conv,
                    cursor,
                )
            }),
            false => None,
        };
        let on_item = match owns_pointer && on_mobile.is_none() {
            true => self.window.as_ref().and_then(|window| {
                items::pick(
                    &self.items,
                    &camera,
                    &self.tiledata,
                    &self.tile_animations,
                    &window.atlases.statics,
                    &cutaway,
                    cursor,
                )
            }),
            false => None,
        };
        // And the map's own furniture last, which is the one a wall is: it has no
        // serial and cannot be used, so it loses to anything that can. Asked
        // every frame rather than at the click, because it is what the *tile
        // marker* has to know — a wall under the cursor takes the highlight, and
        // the diamond drawn on the ground behind it was the client answering the
        // same question twice with two different tiles.
        //
        // This is the one pick that walks the map: `statics::pick` covers the
        // cells `statics::collect` is about to draw. It is a second walk of them
        // per frame with the pointer over the world, and the placement it does
        // per static is the collector's own — see the Frames tab if it ever
        // shows.
        let on_static = match owns_pointer && on_mobile.is_none() && on_item.is_none() {
            true => self.window.as_ref().and_then(|window| {
                statics::pick(
                    &self.map,
                    &camera,
                    &self.tiledata,
                    &self.tile_animations,
                    &window.atlases.statics,
                    &cutaway,
                    cursor,
                )
            }),
            false => None,
        };
        // Kept for the click, which happens between frames and therefore points
        // at the picture the *last* frame drew. Reading it back here rather than
        // picking again there is what makes the wash land on the wall the player
        // was looking at: a second pick would use a camera that has moved since,
        // and the two answers differ by however far the eye travelled in a frame.
        self.on_static = on_static;
        // What the mode allows to light up. `Tiles` lights neither, which is the
        // whole of that setting; the facts above are unchanged by it.
        let lit_mobile = on_mobile.filter(|_| self.highlight != shell::HighlightTarget::Tiles);
        let lit_item = on_item.filter(|_| self.highlight != shell::HighlightTarget::Tiles);

        // # Step three: present. Nothing below this line writes the world.
        //
        // The UI first, because it is what the surface is composited from
        // bottom-up and because its layout is what next frame's viewport comes
        // from. Its request is *held* rather than applied — see [`App::pending`].
        //
        // Timed, and separately from the world below: the two halves of a frame
        // are built by two things that grow for different reasons, and a single
        // build time cannot say which of them ate the frame. See [`frames`].
        //
        // The `Instant`s from here down are instrumentation and not a clock the
        // picture depends on: they measure what this frame cost, and no position
        // in it is a function of them. The one sampling of time that the frame is
        // built from is `started`, at the top.
        let ui_started = Instant::now();
        let hud = self.hud(camera, lit_item, lit_mobile, on_static, &cutaway);
        let painting = self.window.as_ref().map(|screen| Arc::clone(&screen.window));
        let ui = match (self.shell.as_mut(), painting.as_ref()) {
            (Some(shell), Some(window)) => {
                let (request, output) = shell.run(window, &hud, &self.hues);
                let viewport = shell.viewport();
                Some((request, output, viewport))
            }
            _ => None,
        };
        let mut ui_cost = ui_started.elapsed();
        if let Some((request, _, _)) = &ui {
            self.pending = request.clone();
        }

        // What the camera has walked onto since the atlases were last grown.
        // Gathered before the window is borrowed, and not inside the borrow: it
        // reads the whole of `self`, and the window is part of it.
        let want = camera.visible_tiles();
        let wanted = self.wanted_since(camera, self.covered);
        let mut drawn = self.drawn_mobiles();
        // Likewise: the cut the solids view is drawn under reads the player, and
        // the pass that uses it runs inside the window's borrow.
        let solid_cut = self.solid_cut();

        let Some(window) = self.window.as_mut() else {
            return;
        };
        // Grow rather than rebuild. What is new is added to the textures
        // already bound, a band of rows at a time, and a frame where the camera
        // stood still reads four `BTreeSet`s and touches no file and no GPU.
        let grown = window
            .atlases
            .grow(&self.art, &self.texmaps, &self.tiledata, &mut self.anim, &wanted);
        // Whatever was packed is uploaded, including on the way out of a failure:
        // a growth that stopped part way still wrote pixels, and pixels the
        // device has not been told about are sampled as whatever was there
        // before. Cheap to do unconditionally — the band is empty when nothing
        // grew — and it is one fewer path where an atlas and its texture can
        // disagree.
        window.atlases.upload(
            &window.queue,
            &window.renderer,
            &window.statics,
            &window.mobile_pass,
        );
        // Set only in the branch below, on a successful rebuild — this is the
        // counter `docs/camera.md` asks for, so the frame that stalled for it
        // can be told apart from one that is merely heavy. See
        // [`Frame::repacked`](frames::Frame).
        let mut repacked = false;
        match grown {
            Ok(()) => self.covered = Some(want),
            // Full, and this is the eviction: pack an atlas for what is on
            // screen now and throw away everything the camera has walked past.
            // Costly and rare — where the old arrangement paid it every few
            // tiles — and it is the *only* thing that reclaims space, so an
            // atlas that only ever grew would eventually stay full for ever.
            //
            // The passes are rebuilt with it, because the texture a bind group
            // points at is the one the old atlas was uploaded to.
            Err(AtlasError::Full { .. }) => {
                // `covered` is cleared first: a rebuild forgets, so the next
                // frame may not assume anything about what the atlases hold.
                // Set again below only if the rebuild succeeds.
                self.covered = None;
                match Atlases::build(
                    &self.art,
                    self.surfaces.as_ref(),
                    &self.texmaps,
                    &self.tiledata,
                    &mut self.anim,
                    &wanted_in(
                        &self.map,
                        [camera.visible_tiles()],
                        &self.items,
                        &drawn.iter().map(|(_, mobile)| mobile.clone()).collect::<Vec<_>>(),
                        &self.tile_animations,
                        &self.equip_conv,
                    ),
                ) {
                    Ok(atlases) => {
                        window.renderer = GroundRenderer::new(
                            &window.device,
                            &window.queue,
                            blit::WORLD_FORMAT,
                            &atlases.land,
                            &atlases.texmaps,
                        );
                        window.statics = SpriteRenderer::new(
                            &window.device,
                            &window.queue,
                            blit::WORLD_FORMAT,
                            atlases.statics.pixels(),
                            &self.hue_ramp,
                        );
                        window.mobile_pass = SpriteRenderer::new(
                            &window.device,
                            &window.queue,
                            blit::WORLD_FORMAT,
                            atlases.mobiles.pixels(),
                            &self.hue_ramp,
                        );
                        window.atlases = atlases;
                        self.covered = Some(want);
                        repacked = true;
                        self.repacks += 1;
                    }
                    // One screen does not fit one atlas, which is a different
                    // statement from "the atlas filled up": no eviction can help
                    // and the frame draws with sprites missing. Named here
                    // rather than hidden, and it is what the standing backlog
                    // item about a failed repack is about.
                    Err(error) => eprintln!("packing the art on screen: {error}"),
                }
            }
            Err(error) => eprintln!("growing the atlases: {error}"),
        }

        // Three time-varying halves of a mobile, filled in per frame rather
        // than per packet: the crowd is the only thing that knows what a
        // clock — and a group — has done since the `0x77` landed, and
        // `self.player`/`self.others` were built when it did. Against the atlas
        // as it stands *after* this frame's growth, which is the one the
        // picture below is drawn from.
        Self::advance_to_clocks(&self.crowd, &window.atlases.mobiles, &mut drawn);
        // Whoever the crowd is still holding a line for, hung above whichever
        // of `drawn`'s mobiles their serial belongs to. Read out here, before
        // `who` is dropped below: a label with no mobile to anchor to has
        // nothing to draw either way, so the two share the same "still on
        // screen" question `mobiles::head_anchor` answers.
        let speech: Vec<(ViewPixel, String, Font, Hue)> = drawn
            .iter()
            .filter_map(|(who, mobile)| {
                let (text, font, hue) = self.crowd.speaking(*who)?;
                let anchor = mobiles::head_anchor(mobile, &camera, &window.atlases.mobiles)?;
                Some((anchor, text.to_string(), font, hue))
            })
            .collect();
        let drawn: Vec<Mobile> = drawn.into_iter().map(|(_, mobile)| mobile).collect();

        // The vsync wait, and the reason it is timed on its own: under
        // `PresentMode::Fifo` this call blocks until the display has taken the
        // frame before it, which on an idle client is most of the interval.
        // Counted as build time it would report a client that is asleep as one
        // at full load, and the panel exists to tell those two apart.
        let acquire_started = Instant::now();
        let frame = match window.surface.get_current_texture() {
            // Suboptimal still draws: the surface wants reconfiguring, and the
            // next resize event will do it.
            wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                frame
            }
            // The swapchain no longer matches the window. Rebuild it and let the
            // next redraw use it; drawing into a stale one is a crash on some
            // backends and a stretched frame on others.
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                window.surface.configure(&window.device, &window.config);
                return;
            }
            // Nothing was acquired and nothing is wrong: the window is hidden,
            // or the compositor took too long. Skipping the frame is the answer.
            other => {
                if !matches!(
                    other,
                    wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded
                ) {
                    eprintln!("acquiring a frame: {other:?}");
                }
                return;
            }
        };
        let wait = acquire_started.elapsed();
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Where the world goes on the surface: the rect the panels left free, so
        // a docked panel shrinks the world rather than covering it.
        let viewport = ui.as_ref().map_or(
            ViewportRect {
                x: 0,
                y: 0,
                width: window.config.width,
                height: window.config.height,
            },
            |(_, _, viewport)| *viewport,
        );

        // The image the world is drawn into. Its size is the camera's, so a
        // resize and a zoom step are the same event here — and recreating it is
        // the only thing either of them costs.
        //
        // Magnified it is the *viewport's* size and the magnification rides in
        // the vertex transform, so the world is drawn at the display's own
        // resolution and the blit below is a copy; minified it is the world's
        // own larger extent and the blit shrinks it. `docs/camera.md` D11 is the
        // argument, and the short of it is that an image of virtual resolution
        // cannot express an offset of one real pixel — which is the whole of
        // what made a magnified scroll coarser than the screen it was on.
        let (render_width, render_height) = camera.image_size();
        if window.world.width() != render_width || window.world.height() != render_height {
            window.world = blit::world_texture(&window.device, render_width, render_height);
            // Tested pixel for pixel against that image, so it is exactly its
            // size or it is nothing.
            window.depth = renderer::depth_texture(&window.device, render_width, render_height);
            // And the mask with it: it is the colour attachment of a pass whose
            // depth attachment is that buffer, and wgpu requires the two to be
            // one size.
            window.outline_mask = outline::mask_texture(&window.device, render_width, render_height);
            window.select_mask = outline::mask_texture(&window.device, render_width, render_height);
            // And the place channel, which is an attachment of those same
            // passes and is read texel for texel against that image.
            window.place = place::texture(&window.device, render_width, render_height);
        }
        let world_view = window.world.create_view(&wgpu::TextureViewDescriptor::default());

        // **The frame's occluders are built before its pictures are collected**,
        // and that ordering is `docs/lighting_height.md` phase 3's one real cost.
        // A static's drawn row now carries the number this grid gave it
        // (`occlusion::Occlusion::owner_at`), so that a fragment of it can say
        // which occluder it is a point of instead of having that guessed from its
        // height; collecting the pictures first would stamp numbers off the grid
        // of the frame before. Nothing else about either step changed — the
        // statics used to go first for no reason anyone recorded.
        //
        // The lights come from the same camera, cutaway and item list the passes
        // below draw from, so a torch that was not drawn casts nothing and a
        // torch that was is lighting the pixels it is standing in rather than the
        // pixels it stood in last frame.
        // Three skies and not two: night, a daylight with a sun in it, and the
        // plain daylight that is the identity — the frame the blit has always
        // copied through untouched. The middle one is a key today; see
        // `App::sunlit`.
        let sky = match (self.night, self.sunlit) {
            (true, _) => Some(light::NIGHT),
            (false, true) => Some(light::SKYLIGHT),
            // Daylight, where the pass is a copy and no grid is built at all —
            // unless the solids view is on, and then the grid *is* the subject.
            // `Ambient::DAY` flattened is the identity, so the picture under the
            // boxes is the same daylight frame it was; what it buys is that the
            // list drawn is the one the shader would walk, out of the same bake,
            // rather than a second walk of the map made for the view. See
            // `docs/lighting.md` step 23.0.
            (false, false) => self.show_solids.then_some(light::Ambient::DAY),
        };
        // And whether a tile's share of it depends on what stands over the tile.
        // Off by default: see `App::sky_field`, and `light::Ambient::flattened`
        // for why the flat one is the baseline rather than a lesser version.
        let sky = match self.sky_field {
            true => sky,
            false => sky.map(light::Ambient::flattened),
        };
        let mut lighting = match sky {
            Some(ambient) => light::collect(
                &self.map,
                &self.items,
                &camera,
                &self.tiledata,
                &cutaway,
                ambient,
                self.flame_clock.as_secs_f32(),
                // The pictures, which is where an occluder's *facing* comes from:
                // a wall stops a ray only where the ray crosses the side the wall
                // stands on, and only the art says which side that is. The same
                // atlas the statics pass is about to draw from, so the grid and
                // the picture cannot be about two different sets of sprites.
                Some(&window.atlases.statics),
                // And the blocks of that grid built for earlier frames. A camera
                // that has moved a tile wants the same five hundred and fifty
                // blocks it wanted last frame bar a handful — see
                // `occlusion::bake`, and `StaticAtlas::revision` for what makes
                // this let go when the atlas learns something new about a
                // graphic.
                Some(&mut self.occlusion_bake),
            ),
            None => Lighting::NONE,
        };

        let quads = ground::collect(
            &self.map,
            &camera,
            &window.atlases.land,
            &window.atlases.texmaps,
            &cutaway,
        );
        let statics::StaticGeometry {
            quads: static_quads,
            mut mesh_vertices,
            mut mesh_rows,
        } = statics::collect(
            &self.map,
            &camera,
            &self.tiledata,
            &self.tile_animations,
            &window.atlases.statics,
            &cutaway,
            &lighting.occlusion,
        );
        // Through the same pass as the map's statics, because they are the same
        // atlas: one draw call binds one texture, and what covers what is the
        // depth these carry rather than the order they are appended in.
        // One pick (`lit_item`, at the top of the frame), two effects, and the
        // style decides which of them is asked for. `None` is how each is
        // switched off, so neither pass has a mode to branch on: the hue pass
        // draws an item that is not highlighted, and the silhouette pass is
        // handed an empty list.
        let hued = self.highlight_style.hues().then_some(lit_item).flatten();
        let ringed = self.highlight_style.rings().then_some(lit_item).flatten();
        // What a click is holding, placed exactly as the picture placed it —
        // `statics::selected` is `statics::collect`'s own arithmetic — so the
        // mask lands on the wall's pixels rather than beside them. Empty on
        // every frame with nothing selected, which is what switches the pass off.
        let select_quads = statics::selected(
            &camera,
            &self.tiledata,
            &self.tile_animations,
            &window.atlases.statics,
            &cutaway,
            self.selected_static,
        );
        // The same quads as the picture's, so the ring lands on the sprite
        // rather than beside it — see `items::outlined`.
        let outline_quads = items::outlined(
            &self.items,
            &camera,
            &self.tiledata,
            &self.tile_animations,
            &window.atlases.statics,
            &cutaway,
            ringed,
        );
        let statics::StaticGeometry {
            quads: item_quads,
            mesh_vertices: item_mesh_vertices,
            mesh_rows: item_mesh_rows,
        } = items::collect(
            &self.items,
            &camera,
            &self.tiledata,
            &self.tile_animations,
            &window.atlases.statics,
            &cutaway,
            hued,
            &lighting.occlusion,
        );
        let static_quads = {
            let mut quads = static_quads;
            quads.extend(item_quads);
            quads
        };
        // A climbable item gets the same honest mesh a climbable map static
        // does — `items::collect`'s own doc — so its faces join the map
        // statics' here, one buffer and one `mesh_pass.render` call for both.
        mesh_vertices.extend(item_mesh_vertices);
        mesh_rows.extend(item_mesh_rows);
        // A corner static's two faces get their own id past this point — see
        // `docs/gbuffer.md` step 4 and `sprite::split_corners`'s own doc.
        let static_instances = split_corners(static_quads);
        // The same two effects for a creature, off the same style switch and
        // the same one-pick-a-frame rule: `lit_mobile` and `lit_item` are never
        // both `Some` (see where they are asked), so exactly one of the four
        // lists below is ever non-empty.
        let mobile_hued = self.highlight_style.hues().then_some(lit_mobile).flatten();
        let mobile_ringed = self.highlight_style.rings().then_some(lit_mobile).flatten();
        let mobile_outline = mobiles::outlined(
            &drawn,
            &camera,
            &window.atlases.mobiles,
            &cutaway,
            &self.equip_conv,
            mobile_ringed,
        );
        let mobile_quads = mobiles::collect(
            &drawn,
            &camera,
            &window.atlases.mobiles,
            &cutaway,
            &self.equip_conv,
            mobile_hued,
        );
        let labels: Vec<Label<'_>> = speech
            .iter()
            .map(|(anchor, line, font, hue)| Label {
                anchor: *anchor,
                text: line.as_str(),
                font: *font,
                hue: *hue,
                // Nearer than anything the world draws, rather than an
                // `Order` of its own: speech reads as an overlay above
                // whoever said it in every reference client, and there is no
                // real case here of a wall in front of the speaker hiding it
                // that a viewer would want honoured. Worth revisiting with a
                // `depth::text_priority_z` alongside the mobile's own if that
                // ever stops being true.
                depth: 0.0,
            })
            .collect();
        // `fonts.mul` or the operator-supplied TrueType face, never a mix
        // within one frame — see `run`'s doc for why `ttf_font` is an all-or-nothing
        // switch. Unlike `font_atlas`, `ttf_atlas` is grown a line at a time:
        // there is no bounded "whole file" to pack up front for a face that
        // answers to all of Unicode, so this asks it to rasterize whatever of
        // this frame's speech it has not seen yet, the way `window.atlases`
        // grows for graphics newly on screen.
        let text_quads = if let Some(font) = &self.ttf_font {
            let atlas = window
                .ttf_atlas
                .as_mut()
                .expect("create_window builds ttf_atlas whenever ttf_font is set");
            if let Err(error) = atlas.add(font, labels.iter().flat_map(|label| label.text.chars())) {
                // `eprintln!` and a frame that draws anyway, the same corner
                // `AtlasError::Full` already cuts for the map's own atlases —
                // see docs/client.md. Unreachable in practice: a shard's whole
                // spoken character set is a few hundred glyphs at most, nowhere
                // near one 2048 texture.
                eprintln!("packing ttf glyphs: {error}");
            }
            if let Some(rows) = atlas.take_dirty() {
                window
                    .ttf_pass
                    .as_ref()
                    .expect("create_window builds ttf_pass whenever ttf_atlas is")
                    .upload_rows(&window.queue, atlas.pixels(), rows);
            }
            text::collect_ttf(&labels, atlas)
        } else {
            text::collect(&labels, &self.font_atlas)
        };
        let depth_view = window.depth.create_view(&wgpu::TextureViewDescriptor::default());
        let place_view = window.place.create_view(&wgpu::TextureViewDescriptor::default());
        let target = Target {
            view: &world_view,
            depth: &depth_view,
            place: &place_view,
            width: render_width,
            height: render_height,
            projection: camera.projection(),
        };
        let mut encoder = window
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        // Ground first, because it clears; statics after, into what it left.
        // Which covers which is decided by the depth they share, not by this
        // order — the order only decides who clears.
        window
            .renderer
            .render(&window.device, &window.queue, &mut encoder, target, &quads);
        window.statics.render(
            &window.device,
            &window.queue,
            &mut encoder,
            target,
            &static_instances.rows,
            Some(static_instances.drawn),
        );
        // Right after statics, into the same static's own pixels its
        // billboard sprite just drew — `docs/gbuffer.md` step 4c. Depth and
        // place only, never colour: this only gives a climbable static's
        // pixels a more honest per-face normal than one blended stance could.
        window.mesh_pass.render(
            &window.device,
            &window.queue,
            &mut encoder,
            target,
            &mesh_vertices,
            &mesh_rows,
        );
        window.mobile_pass.render(
            &window.device,
            &window.queue,
            &mut encoder,
            target,
            &mobile_quads,
            None,
        );
        // The silhouettes, here and not later: the mask is depth-tested against
        // what the three world passes have drawn, so a barrel behind a wall is
        // kept out of it — and the text pass below writes depth at the near
        // plane over everything, which would punch the mask through.
        let mask_view = window
            .outline_mask
            .create_view(&wgpu::TextureViewDescriptor::default());
        // One item is one ring; the pass numbers groups, so each quad is a group
        // of its own — see `SpriteRenderer::render_mask`.
        let item_rings: Vec<&[SpriteQuad]> = outline_quads.iter().map(std::slice::from_ref).collect();
        window.statics.render_mask(
            &window.device,
            &window.queue,
            &mut encoder,
            target,
            &mask_view,
            &item_rings,
        );
        // And a creature through its own atlas, in *one* group: a body and
        // everything it wears is one thing being pointed at, and one ring goes
        // round the lot. This pass clears the mask too, which is why it is
        // skipped when nothing is ringed — the items' pass above has already
        // written the frame's answer, and a second clear would erase it.
        if !mobile_outline.is_empty() {
            window.mobile_pass.render_mask(
                &window.device,
                &window.queue,
                &mut encoder,
                target,
                &mask_view,
                &[&mobile_outline],
            );
        }
        // And the held selection into its own mask, through the same pass and
        // the same depth buffer: what is washed is what is *visible* of the
        // selected static, so a wall the player has walked behind is not painted
        // over the thing now in front of it. One group, because a selection is
        // one thing — the pass numbers groups for the ring's sake and the wash
        // reads only "is this texel nought".
        let select_view = window
            .select_mask
            .create_view(&wgpu::TextureViewDescriptor::default());
        if !select_quads.is_empty() {
            window.statics.render_mask(
                &window.device,
                &window.queue,
                &mut encoder,
                target,
                &select_view,
                &[&select_quads],
            );
        }
        // `ttf_pass` when the run is drawing through it — bound to a
        // different texture than `text_pass`, so a mix of the two within one
        // frame would sample one atlas with quads packed for the other.
        let text_renderer = match &mut window.ttf_pass {
            Some(pass) => pass,
            None => &mut window.text_pass,
        };
        text_renderer.render(
            &window.device,
            &window.queue,
            &mut encoder,
            target,
            &text_quads,
            None,
        );
        // And the world image onto the surface, into the rect the panels left
        // free. Magnified this is a copy — the image is already the viewport's
        // size and the magnification happened in the vertex transform — and
        // minified it is where the shrinking happens, which is why the zoom is
        // still what picks the sampler.
        //
        // The lights themselves were collected at the top of the frame, before
        // the pictures — see the comment there for why the order is that way
        // round now.
        // The sun is a property of the sky and not of the tiles, so it is set
        // here rather than inside the walk — and never at night, where a second
        // source lighting every roof would undo the whole point of the dark.
        if self.sunlit && !self.night {
            lighting.sun = Some(light::midday());
        }
        // And the flame in the player's own hand, which no walk of the map could
        // have found — see `light::carried`. Only where the frame has a sky at
        // all: with no ambient the pass is the copy the blit has always been, and
        // a beam over an already-white multiplier would cost a loop to change
        // nothing. It goes in after the sort, and `hold` is what says it is never
        // the flame dropped when a tavern's candles fill the array.
        if self.lantern && sky.is_some() {
            lighting.hold(light::carried(
                self.player.at,
                self.player.facing,
                self.flame_clock.as_secs_f32(),
            ));
        }
        // The view is the looker's, not the world's: a diagnostic draws from the
        // values this frame was lit with, and in daylight those are the ambient
        // and the place attachment — which is exactly what a person checking the
        // place channel wants to see, without having to make it night first.
        lighting.view = self.light_view;
        // **Solids alone**, `App::solids_only`: the surface is cleared and the
        // world image is not drawn onto it at all, so the boxes below stand
        // over nothing that could be mistaken for their own shape. `lighting`
        // is unaffected either way — it is what the solids pass reads its grid
        // from, and it was already built above whichever branch runs here.
        if self.solids_only && self.show_solids {
            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("solids-only clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(openshard_client_render::renderer::CLEAR),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                multiview_mask: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        } else {
            window.blit.render(
                &window.device,
                &window.queue,
                &mut encoder,
                blit::Frame {
                    target: &view,
                    world: &world_view,
                    place: &place_view,
                    face_instances: window.statics.instances_buffer(),
                    mobile_instances: window.mobile_pass.instances_buffer(),
                    mesh_instances: window.mesh_pass.rows_buffer(),
                    ground_instances: window.renderer.instances_buffer(),
                    zoom: camera.zoom(),
                    rect: viewport,
                },
                &lighting,
            );
        }
        // The occlusion grid as solids, when somebody asked for it — step 23.0.
        // First of what is drawn over the lit picture, so the highlights stay on
        // top of it: a diagnostic must not hide the thing the cursor is naming.
        //
        // The grid drawn is the frame's **own** — `lighting.occlusion`, which is
        // the list the shader is walking this same frame — and not a second walk
        // of the map. A picture of a grid rebuilt beside the one in force would
        // be a claim about a grid nothing rendered.
        if self.show_solids {
            let standing = openshard_client_render::solid::standing(&lighting.occlusion, solid_cut);
            self.solids_held = standing.len();
            self.solids_drawn = window.solids.render(
                &window.device,
                &window.queue,
                &mut encoder,
                solids::Frame {
                    target: &view,
                    size: (window.config.width, window.config.height),
                    rect: viewport,
                },
                &camera,
                &standing,
                solids::Style {
                    opaque: self.solids_opaque,
                    ..solids::Style::default()
                },
            );
        }
        // The held selection's wash, first of the two things drawn over the lit
        // picture: the wall the click named and the ground it stands on. Under
        // the ring rather than over it, because they answer different questions
        // — the wash is what is *held* and the ring is what the cursor is on —
        // and the live one has to stay readable while it passes over the held
        // one.
        //
        // Skipped when nothing is selected, and the whole cost of a frame with
        // nothing selected is that comparison: the mask is not drawn either.
        if let Some(picked) = self.selected_static.filter(|_| !select_quads.is_empty()) {
            window.select.render(
                &window.device,
                &window.queue,
                &mut encoder,
                select::Frame {
                    target: &view,
                    mask: &select_view,
                    place: &place_view,
                    face_instances: window.statics.instances_buffer(),
                    ground_instances: window.renderer.instances_buffer(),
                    size: (render_width, render_height),
                    rect: viewport,
                },
                // The tile the *static* stands on, and not `selected_tile`: the
                // ground being washed is the ground under the thing that was
                // picked, which is the whole of "and the tile it stands on". The
                // two are usually different tiles — a wall's picture stands up
                // the screen from its own cell, so the ground under the cursor is
                // the cell behind it.
                Selection::DEFAULT.on((picked.at.x, picked.at.y)),
            );
        }
        // And the ring on top of that, over the same rectangle — after the blit
        // so it is drawn in screen pixels and unlit: a highlight that dimmed at
        // night would stop working exactly when the picture is hardest to read.
        // Skipped entirely on the ordinary frame, where nothing is under the
        // cursor and the mask is empty. **Both silhouette lists**, or a ringed
        // creature draws its mask into a texture no pass ever reads and the
        // highlight is simply absent — which is what an item-only test of this
        // condition looked like from the outside.
        if !outline_quads.is_empty() || !mobile_outline.is_empty() {
            window.outline.render(
                &window.device,
                &window.queue,
                &mut encoder,
                outline::Frame {
                    target: &view,
                    mask: &mask_view,
                    mask_size: (render_width, render_height),
                    rect: viewport,
                },
                // The soft ring — an edge with a glow behind it — widened when
                // the world is minified, where one mask texel is less than one
                // screen pixel and a hairline breaks into a dashed line. See
                // `Ring::for_zoom`.
                Ring::SOFT.for_zoom(camera.zoom()),
            );
        }
        // The shard's dialogs, in the client's own art, over the finished
        // picture and under egui's.
        //
        // Under egui and not over it, deliberately: the widgets that *answer* a
        // gump are still egui's, laid out at the same coordinates in the same
        // units — one gump pixel is one egui point, and the scale below is the
        // window's own scale factor, which is what makes those two spaces the
        // same one. So the art draws the window and egui's transparent widgets
        // sit exactly on it. See `client/app/src/gump.rs`.
        //
        // The atlas grows here rather than when the packet arrived: a page
        // button flips pages inside the client, so what a window needs is every
        // page's art and not the showing one's — `gump::art_of` is that list,
        // and it is asked for on the frame the window is drawn on because that
        // is the frame that knows the window is open at all.
        if let (Some(files), Some(pass)) = (self.gumps.as_ref(), window.gump_pass.as_mut()) {
            let open = self
                .view
                .as_ref()
                .map(|view| view.gumps.as_slice())
                .unwrap_or_default();
            let mut pictures = Vec::new();
            for gump in open {
                let art_files = gump_art::ArtFiles {
                    gumps: files,
                    items: &self.art,
                };
                if let Err(error) = self.gump_atlas.add(art_files, gump_art::art_of(&gump.elements)) {
                    // Said once per window and then drawn without whatever is
                    // missing: a dialog with a hole in it is still a dialog the
                    // player can read, and a client that refused to draw one
                    // would take the shard's staff commands down with it.
                    eprintln!("packing gump art for {:?}: {error}", gump.gump_id);
                }
                // Where egui put it, not where the server asked for it: the
                // player may have dragged the window since, and the art has to
                // arrive at the same rectangle the buttons did. A window egui
                // has not laid out yet — the frame its packet arrived on — has
                // nowhere to put its art and waits one frame.
                let Some(place) = self
                    .shell
                    .as_ref()
                    .and_then(|shell| shell.gumps().placement(gump.gump_id.0))
                else {
                    continue;
                };
                pictures.extend(
                    gump_art::window(
                        &gump.elements,
                        GumpPixel::new(place.at.0, place.at.1),
                        place.page,
                        &place.on,
                        // Nothing is drawn held: the button the mouse is on is
                        // egui's widget, and it draws its own press.
                        None,
                        &self.gump_atlas,
                    )
                    .pictures,
                );
            }
            // This client's own windows, over the dialogs. Not egui windows at
            // all, unlike the `0xB0`s above: a container has no widget in it to
            // answer with — no button, no field, nothing that would need
            // egui's hit test — so its position, its drag and its z-order are
            // this client's, in gump pixels, and there is nothing left for a
            // frame to be laid out by. A paperdoll is the same machinery's
            // second caller, which is decision 5 in `docs/client.md`. See
            // `own_windows`, `openshard_client_render::container` and
            // `openshard_client_render::paperdoll`.
            //
            // Bottom to top, which is the list's own order: the pass has no
            // depth, so later is over.
            //
            // The layouts are built before the loop that packs them, so that
            // nothing borrows the view while the atlas is being grown.
            // Paired with their subjects rather than left parallel to
            // `own_windows`: a container whose entry has gone from the view is
            // skipped below, and an index into one list would then name the
            // wrong window in the other. This list is what the pointer is
            // tested against next frame — see `App::drawn_windows`.
            let mut windows: Vec<(WindowSubject, Vec<gump_art::Picture>)> = Vec::new();
            if let Some(view) = self.view.as_ref() {
                for open in &self.own_windows {
                    match open.subject {
                        WindowSubject::Container(serial) => {
                            let Some(gump) = view.containers.get(&serial).copied() else {
                                continue;
                            };
                            let contents: Vec<ContainedItem> =
                                view.contents.get(&serial).cloned().unwrap_or_default();
                            windows.push((open.subject, container::window(gump, &contents, open.at)));
                        }
                        WindowSubject::Paperdoll(serial) => {
                            // Whose body and whose equipment, read off the view
                            // inline rather than through a method: the
                            // surface's window is held mutably across this
                            // loop, and a `&self` call would borrow all of it.
                            // Nothing else asks these questions — the hit test
                            // reads the list this builds (`drawn_windows`)
                            // rather than working out the body a second time,
                            // which is what used to make a paperdoll whose two
                            // answers disagreed a window that could not be
                            // closed.
                            let own = view.player.serial == serial;
                            let body = match own {
                                true => Some((view.player.body.0, view.player.hue)),
                                // A paperdoll of a mobile this client has never
                                // been told the body of: the frame is drawn and
                                // the doll is not, until the `0x77` arrives.
                                false => view.mobiles.get(&serial).map(|m| (m.body.0, m.hue)),
                            };
                            // The `0x88` carries no equipment — see
                            // `WorldView::paperdolls` — so it is read off the
                            // body the window names.
                            let equipment = match own {
                                true => crowd::worn(&view.player.equipment, &self.tiledata),
                                false => match view.mobiles.get(&serial) {
                                    Some(mobile) => crowd::worn(&mobile.equipment, &self.tiledata),
                                    None => Vec::new(),
                                },
                            };
                            let wearer = body.map(|(body, hue)| paperdoll::Wearer {
                                body,
                                hue,
                                equipment: &equipment,
                            });
                            let whose = match own {
                                true => paperdoll::Whose::Own,
                                false => paperdoll::Whose::Another,
                            };
                            windows.push((
                                open.subject,
                                paperdoll::window(wearer.as_ref(), whose, &self.equip_conv, files, open.at),
                            ));
                        }
                    }
                }
            }
            for (_, window) in &windows {
                let art_files = gump_art::ArtFiles {
                    gumps: files,
                    items: &self.art,
                };
                // Everything the window will draw, packed before it is drawn —
                // a picture the atlas grew on the *next* frame would draw the
                // window with a hole in it once. Said and drawn anyway on a
                // failure, for `gump::art_of`'s reason above.
                if let Err(error) = self.gump_atlas.add(art_files, paperdoll::art_of(window)) {
                    eprintln!("packing window art: {error}");
                }
                pictures.extend(window.iter().copied());
            }
            // What the pointer is tested against from here on, and the atlas it
            // is tested in is the one just grown for it: the hit test and the
            // frame are now the same list. Kept even when it is empty — the
            // windows this frame drew none of are windows nothing can click.
            self.drawn_windows = windows;
            if let Some(rows) = self.gump_atlas.take_dirty() {
                pass.upload_rows(&window.queue, self.gump_atlas.pixels(), rows);
            }
            let quads = gump_art::collect(&pictures, &self.gump_atlas);
            pass.render(
                &window.device,
                &window.queue,
                &mut encoder,
                gump_art::Frame {
                    target: &view,
                    width: window.config.width,
                    height: window.config.height,
                    // A whole number, and the same one egui is laying its
                    // widgets out at: gump art is five-bit pixel art sampled
                    // with Nearest, and a fractional scale doubles some of its
                    // rows and not others.
                    // egui's own, and not the window's scale factor rounded:
                    // the art is placed at coordinates egui laid out in
                    // points, so any other number here slides a window's
                    // pictures off its buttons.
                    scale: self
                        .shell
                        .as_ref()
                        .map(|shell| shell.gumps().scale())
                        .unwrap_or(1.0),
                },
                &quads,
            );
        }
        // The UI over it, with no depth attachment: the world's depth buffer
        // ordered the world, and this is drawn on the result.
        if let (Some(shell), Some((_, output, _))) = (self.shell.as_mut(), ui) {
            let painting = Instant::now();
            shell.paint(
                &window.device,
                &window.queue,
                &mut encoder,
                &view,
                output,
                [window.config.width, window.config.height],
            );
            ui_cost += painting.elapsed();
        }
        window.queue.submit([encoder.finish()]);
        // Presentation moved onto the queue in wgpu 30; the texture is consumed.
        window.queue.present(frame);
        // And the next frame is asked for here rather than through the timer,
        // unconditionally while somebody is watching. This is the pacer: the
        // surface presents in FIFO, so `get_current_texture` above blocks the
        // next frame until the display has taken this one, and asking again
        // straight away runs the loop at the display's own rate instead of at a
        // 16ms timer that beats against it.
        //
        // Every frame and not only the gliding ones, which is the change: a
        // client that only redrew when something moved dropped to 12.5 frames a
        // second the moment the player stood still, and however correct the
        // reason was, what it looked like was a stall. The timer stays for the
        // window nobody is looking at — see [`App::pacing`].
        if watched {
            window.window.request_redraw();
        }
        let took = started.elapsed();
        // The interval between two *drawn* frames, and where this one's time
        // went: the pacing and the price, which are the two things a drop in
        // frame rate can be — and the price split between the panels and the
        // world, which are the two things the price can be. See [`frames`].
        //
        // The scene is what is left after the UI and the wait rather than a
        // fourth clock, so the three always add up to the frame exactly: a
        // fourth `Instant` would leave a remainder nobody could account for.
        let scene = took.saturating_sub(ui_cost).saturating_sub(wait);
        self.frames.record(
            started.saturating_duration_since(self.last_frame),
            ui_cost,
            scene,
            wait,
            repacked,
        );
        self.last_frame = started;
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
fn ask_between(body: camera::WorldPixel, cursor: camera::WorldPixel) -> Option<steer::Ask> {
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
fn heading_between(dx: i32, dy: i32) -> Option<Heading> {
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
fn on_screen(direction: Direction) -> (i32, i32) {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Which way the cursor points, for the tests that are about the bearing
    /// and not about the ring it fell in.
    fn heading_to(body: camera::WorldPixel, cursor: camera::WorldPixel) -> Option<Heading> {
        ask_between(body, cursor).map(steer::Ask::heading)
    }

    /// The screen bearings of the eight directions, as the isometric actually
    /// draws them — and they are not the grid's. On screen the diamond is
    /// turned an eighth: north-east points due right, south-east due down.
    /// Anything reading a cursor has to answer in *these* terms, which is the
    /// whole reason the heading is measured here rather than on the grid.
    #[test]
    fn the_screen_bearings_are_the_grid_turned_an_eighth() {
        assert_eq!(on_screen(Direction::NorthEast), (44, 0), "due right");
        assert_eq!(on_screen(Direction::SouthEast), (0, 44), "due down");
        assert_eq!(on_screen(Direction::SouthWest), (-44, 0), "due left");
        assert_eq!(on_screen(Direction::NorthWest), (0, -44), "due up");
        assert_eq!(on_screen(Direction::East), (22, 22), "down and right");
        assert_eq!(on_screen(Direction::North), (22, -22));
        assert_eq!(on_screen(Direction::South), (-22, 22));
        assert_eq!(on_screen(Direction::West), (-22, -22));
    }

    /// A cursor held away from the body in each of those eight screen bearings
    /// asks for that direction — including the one that catches a heading
    /// measured on the grid by mistake: straight down the screen is
    /// *south-east*, and a grid reading would call it south.
    #[test]
    fn a_cursor_on_a_screen_bearing_asks_for_that_direction() {
        let body = camera::WorldPixel { x: 0, y: 0 };
        for direction in Direction::ALL {
            let (sx, sy) = on_screen(direction);
            let cursor = camera::WorldPixel { x: sx * 7, y: sy * 7 };
            let heading = heading_to(body, cursor).expect("the cursor is not on the body");
            assert_eq!(heading.direction, direction, "screen bearing {sx},{sy}");
            assert_eq!(
                heading.lean,
                Lean::Centred,
                "squarely on the bearing leans neither way"
            );
        }
    }

    /// The atlas is grown for the group that will be *drawn*, not for the one
    /// the last packet named.
    ///
    /// The two used to be different lists. `App::wanted_in` asked
    /// `needed_animations` about `self.player`/`self.others`, built at the last
    /// `see`, while `mobiles::collect` drew the group `Crowd::group_for` gives —
    /// and `Crowd::advance` moves a body from walking to standing with no packet
    /// in between. So a body that stopped was drawn from a standing frame the
    /// atlas had never been asked to pack, `mobiles::place` found nothing, and
    /// the sprite disappeared — and stayed gone, because a body standing still
    /// sends nothing that would rebuild the list.
    #[test]
    fn the_group_packed_is_the_group_the_crowd_is_playing() {
        const PLAYER: u16 = 400;
        let mut crowd = Crowd::default();
        let facing = Facing::walking(Direction::East);
        crowd.see(None, Point::new(10, 10, 0), Graphic(PLAYER), facing, Hue::NONE);
        // The snapshot the app would store in `self.player`: walking, because a
        // step had just landed when the packet was folded.
        let stepped = crowd.see(None, Point::new(11, 10, 0), Graphic(PLAYER), facing, Hue::NONE);
        let walking = stepped.group;

        // Long enough that the walk gives up on its own timer. No packet.
        crowd.advance(openshard_movement::WALK_HOLD * 2);
        let standing = crowd.group_for(None).expect("the crowd is tracking this body");
        assert_ne!(walking, standing, "the scene is only a scene if the group moved");

        // Through the list `App::wanted_in` grows the atlases from, and not
        // through `advance_groups` directly: what is being protected is that the
        // packing path goes through the refresh at all.
        let drawn = App::everyone_drawn(&crowd, None, &stepped, &[]);
        let mobiles: Vec<Mobile> = drawn.into_iter().map(|(_, mobile)| mobile).collect();
        let wanted = mobiles::needed_animations(&mobiles, &EquipConv::default());
        let (direction, _) = openshard_uofiles::anim::facing(mobiles[0].facing);
        assert!(
            wanted.contains(&(PLAYER, standing, direction)),
            "the standing group has to be packed to be drawn: {wanted:?}"
        );
    }

    /// And off the bearing, the lean says which side — which is the thing the
    /// eight sectors throw away and the only thing that can settle a corner
    /// with two open ways round it. Straight down the screen is south-east;
    /// nudged to the right of that, the ask is still south-east but is leaning
    /// toward east, which is where east is drawn.
    #[test]
    fn a_cursor_off_the_bearing_leans_toward_the_side_it_is_on() {
        let body = camera::WorldPixel { x: 0, y: 0 };
        let down_and_right = heading_to(body, camera::WorldPixel { x: 6, y: 300 }).unwrap();
        assert_eq!(down_and_right.direction, Direction::SouthEast);
        assert_eq!(down_and_right.lean, Lean::Counter);

        let down_and_left = heading_to(body, camera::WorldPixel { x: -6, y: 300 }).unwrap();
        assert_eq!(down_and_left.direction, Direction::SouthEast);
        assert_eq!(down_and_left.lean, Lean::Clockwise);
    }

    /// The cursor on the body names no direction at all, rather than the
    /// nearest one: an ask nobody made.
    #[test]
    fn a_cursor_on_the_body_asks_for_nothing() {
        let body = camera::WorldPixel { x: 17, y: -3 };
        assert_eq!(ask_between(body, body), None);
    }

    /// And neither does one merely *near* it, all the way round: the dead zone
    /// is a disc, so the same distance means the same thing on the diagonal as
    /// on the cardinal. This is the bug it exists for — a button held with the
    /// mouse sitting still over the character used to walk it off in whichever
    /// of the eight directions the last pixel of hand tremor happened to name.
    #[test]
    fn a_cursor_inside_the_dead_zone_asks_for_nothing() {
        let body = camera::WorldPixel { x: 17, y: -3 };
        for degrees in 0..360 {
            let radians = f64::from(degrees).to_radians();
            let (unit_x, unit_y) = (radians.cos(), radians.sin());
            // Just inside and just outside, in the same bearing: the pair is
            // what pins the radius rather than merely the existence of a zone.
            let at = |distance: f64| camera::WorldPixel {
                x: body.x + (unit_x * distance).round() as i32,
                y: body.y + (unit_y * distance).round() as i32,
            };
            assert_eq!(
                ask_between(body, at(DEAD_ZONE - 2.0)),
                None,
                "{degrees}° inside the dead zone"
            );
            assert!(
                ask_between(body, at(DEAD_ZONE + 2.0)).is_some(),
                "{degrees}° outside the dead zone"
            );
        }
    }

    /// The ring between the two radii: the cursor names a direction, and what
    /// it asks for is a facing rather than a walk. The classic client's, and
    /// the only way a mouse can turn a character on the spot — every other ask
    /// it makes also sets the body walking.
    ///
    /// Swept all the way round, because the ring is a ring: the same distance
    /// has to mean the same thing on the diagonal as on the cardinal, and a
    /// zone written as two axis comparisons would be a square.
    #[test]
    fn a_cursor_inside_the_turn_ring_asks_for_a_facing_and_no_ground() {
        let body = camera::WorldPixel { x: -8, y: 42 };
        let mut checked = 0;
        for degrees in 0..360 {
            let radians = f64::from(degrees).to_radians();
            let (unit_x, unit_y) = (radians.cos(), radians.sin());
            let at = |distance: f64| camera::WorldPixel {
                x: body.x + (unit_x * distance).round() as i32,
                y: body.y + (unit_y * distance).round() as i32,
            };
            // Inside the ring and outside it, on one bearing: the pair is what
            // pins the radius rather than merely the existence of a zone.
            let inside = ask_between(body, at(TURN_ZONE - 2.0)).expect("outside the dead zone");
            assert!(
                matches!(inside, steer::Ask::Turn(_)),
                "{degrees}° inside the turn ring asked to walk: {inside:?}"
            );
            let outside = ask_between(body, at(TURN_ZONE + 2.0)).expect("outside the dead zone");
            assert!(
                matches!(outside, steer::Ask::Walk(_)),
                "{degrees}° outside the turn ring asked only to turn: {outside:?}"
            );
            checked += 1;
        }
        assert_eq!(checked, 360, "every bearing is a case, and every one was checked");

        // And the ring decides what is asked for, never which way: on the eight
        // screen bearings, where rounding a two-pixel offset onto whole pixels
        // cannot tip the answer into a neighbouring sector, both sides of it
        // name the same direction.
        for direction in Direction::ALL {
            let (sx, sy) = on_screen(direction);
            let unit = f64::from(sx).hypot(f64::from(sy));
            let at = |distance: f64| camera::WorldPixel {
                x: body.x + (f64::from(sx) * distance / unit).round() as i32,
                y: body.y + (f64::from(sy) * distance / unit).round() as i32,
            };
            let inside = ask_between(body, at(TURN_ZONE - 2.0)).expect("outside the dead zone");
            let outside = ask_between(body, at(TURN_ZONE + 2.0)).expect("outside the dead zone");
            assert_eq!(inside.heading().direction, direction);
            assert_eq!(outside.heading().direction, direction);
        }
    }

    /// Where a step stops overshooting — and that [`TURN_ZONE`] reaches it,
    /// which is what makes the walk ring start where walking becomes the right
    /// answer rather than at a number somebody liked.
    ///
    /// From `22 / cos 22.5°` out, the step this answers with ends *nearer* the
    /// cursor than the body started, so the ask cannot reverse. Nearer than
    /// that it can, and the dead zone deliberately does not cover the gap: what
    /// it exists for is the jitter at a couple of pixels, and a radius half a
    /// tile wide would be a hole in the picture the player can feel. This test
    /// is here so the number stays a decision — it is derived from the
    /// projection, so a tile drawn 2:1 one day moves it, and the constant above
    /// has to be re-argued rather than silently left behind.
    ///
    /// Swept over every bearing, because the worst case is not on a direction's
    /// own bearing but at the corner of its sector, 22.5° off, where the step
    /// spends most of its length going sideways.
    #[test]
    fn a_step_stops_overshooting_further_out_than_the_dead_zone() {
        // The longest step the projection draws, halved and opened out by the
        // widest the cursor can sit off the bearing that wins its sector.
        let overshoot_free = Direction::ALL
            .into_iter()
            .map(|direction| {
                let (step_x, step_y) = on_screen(direction);
                f64::from(step_x).hypot(f64::from(step_y))
            })
            .fold(0.0_f64, f64::max)
            / (2.0 * 22.5_f64.to_radians().cos());
        assert!(
            DEAD_ZONE < overshoot_free,
            "the dead zone is the smaller of the two on purpose: {DEAD_ZONE} against {overshoot_free}"
        );
        // The band between them is the turn ring, and it has to cover the whole
        // of the overshoot: a cursor anywhere a step would land past is
        // answered with a facing and no ground.
        assert!(
            TURN_ZONE >= overshoot_free,
            "the turn ring stops short of the overshoot: {TURN_ZONE} against {overshoot_free}"
        );

        let body = camera::WorldPixel { x: 0, y: 0 };
        // Counted, because a sweep is only worth what it got to.
        let mut checked = 0;
        for tenths in 0..3600 {
            let radians = (f64::from(tenths) / 10.0).to_radians();
            // Just outside, where a step has the least room to be an
            // improvement — plus the ¾ of a pixel that rounding a bearing onto
            // the whole-pixel grid can move it inward, so every bearing is a
            // case this actually gets to claim something about rather than a
            // skip.
            let distance = TURN_ZONE + 0.75;
            let (cursor_x, cursor_y) = (radians.cos() * distance, radians.sin() * distance);
            let cursor = camera::WorldPixel {
                x: cursor_x.round() as i32,
                y: cursor_y.round() as i32,
            };
            let heading = heading_to(body, cursor).expect("well outside the dead zone");
            let (step_x, step_y) = on_screen(heading.direction);
            let after = f64::from(cursor.x - step_x).hypot(f64::from(cursor.y - step_y));
            let before = f64::from(cursor.x).hypot(f64::from(cursor.y));
            assert!(before > overshoot_free, "the rounding margin holds at {before}");
            assert!(
                after < before,
                "at {}° the step {:?} ends {after} away, having started {before} away",
                f64::from(tenths) / 10.0,
                heading.direction,
            );
            checked += 1;
        }
        assert_eq!(
            checked, 3600,
            "every bearing is a case, and every one was checked"
        );
    }
}
