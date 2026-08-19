//! What the HUD remembers between runs.
//!
//! This client is bare `egui` on `winit` and `wgpu` — there is no `eframe`, and
//! `eframe` is the crate that would otherwise be saving `egui::Memory` and the
//! window's own geometry. So nothing was persisted, and nothing was going to be:
//! it is not that the saving was broken, it is that there was nobody to do it.
//! This module is that somebody.
//!
//! Deliberately *not* a serialized `egui::Memory`. That would carry every
//! widget's scroll offset and every id egui happened to allocate, in a format
//! whose meaning is egui's version rather than ours — an upgrade silently
//! restores nonsense or nothing. What is here is the list of things a player
//! would notice missing when the client reopens, written out one field at a
//! time, in a file a human can read and delete.
//!
//! # The two densities
//!
//! [`Zoom`] is egui's `zoom_factor` — the HUD's own scale, on top of the
//! monitor's `scale_factor`, which is the platform's business and is never
//! saved (a file that pinned it would fight the compositor on the next screen).
//! It is *not* the density the world's TTF text is baked at: that atlas is
//! [`Chat::ttf_scale`]'s own multiple of `TTF_BASE_PIXEL_HEIGHT`, and the
//! HUD's scale has no bearing on it.

use std::path::Path;

use openshard_client_render::light;
use serde::{Deserialize, Serialize};

/// Where the state lives: beside `openshard.toml`, in the working directory.
///
/// The same place the operator's own config is, for the same reason — it is
/// per-checkout, visible, and deleting it is how you get the defaults back.
pub const PATH: &str = "client_ui.toml";

/// Which page of the dev window is in front.
///
/// The dev panels were five floating windows once, and five windows is five
/// things to arrange and lose. One window with these as its tabs is the same
/// panels; what changed is that there is one thing to place and one thing to
/// remember.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tab {
    /// Where the eye is and what it is looking at.
    #[default]
    Camera,
    /// Every number the camera is made of, and the bench's scenarios.
    Rig,
    /// What the last few seconds of the event loop cost.
    Frames,
    /// What the view has decoded: mobiles and ground items.
    World,
    /// The tile under the cursor, the overlays, and what a click would take.
    Tile,
    /// Every number the lighting is turned by — [`Light`].
    Light,
    /// How big the HUD chat box's glyphs draw and what colour the player's own
    /// line takes — [`Chat`].
    Chat,
    /// The effects and music mixer gains.
    Audio,
}

impl Tab {
    /// Every tab, in the order the bar draws them.
    ///
    /// One list, so the bar and anything that iterates the pages cannot come to
    /// disagree about which tabs exist.
    pub const ALL: [Tab; 8] = [
        Tab::Camera,
        Tab::Rig,
        Tab::Frames,
        Tab::World,
        Tab::Tile,
        Tab::Light,
        Tab::Chat,
        Tab::Audio,
    ];

    /// What the bar calls it.
    pub fn title(self) -> &'static str {
        match self {
            Tab::Camera => "Camera",
            Tab::Rig => "Rig",
            Tab::Frames => "Frames",
            Tab::World => "World",
            Tab::Tile => "Tile",
            Tab::Light => "Light",
            Tab::Chat => "Chat",
            Tab::Audio => "Audio",
        }
    }
}

/// The lighting's own knobs, as they are written to the file.
///
/// **A second spelling of [`light::Tuning`] and deliberately so.** The renderer
/// never opens a file — that is the whole of what keeps it runnable against an
/// offscreen texture in a test and a canvas in a browser — so it carries no
/// `serde`, and giving it one to save four numbers would be paying for the
/// dependency in every build of the crate that draws. What is duplicated here is
/// the *field list* and nothing else: every number's meaning, its domain and its
/// clamp live over there, and [`Light::tuning`] is the one place the two meet.
/// `the_saved_light_is_the_renderers_own_default` is what holds them together —
/// it is what goes red when a field is added on one side only.
///
/// `#[serde(default)]` per the struct, as [`Desk`] itself has: a file written by
/// a build that predates a knob is missing that line and nothing else, and the
/// rest of the page must survive it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Light {
    /// How big a flame's body is, in tiles — the softness of every shadow.
    pub flame_radius: f32,
    /// How many rays each fragment casts at each flame.
    pub shadow_rays: u32,
    /// What every flame's brightness is multiplied by.
    pub brightness: f32,
    /// And its reach.
    pub reach: f32,
    /// What the ambient's sky term is multiplied by,
    pub sky: f32,
    /// and its floor.
    pub ground: f32,
    /// Where the sun stands, in degrees around the map's own axes,
    pub sun_azimuth: f32,
    /// how steeply it climbs, in tiles up per tile along,
    pub sun_rise: f32,
    /// how much it adds where it reaches,
    pub sun_intensity: f32,
    /// and its colour — [`light::SunTuning::color`], a literal rather than a
    /// tint, for the reason stated there.
    pub sun_color: [f32; 3],
    /// A tint the player's own light is multiplied through — see
    /// [`light::Tuning::headlight_color`]. `[1.0, 1.0, 1.0]` leaves it whatever
    /// colour it was built as.
    pub headlight_color: [f32; 3],
    /// A tint every lantern the map itself burns is multiplied through — see
    /// [`light::Tuning::lantern_color`]. `[1.0, 1.0, 1.0]` leaves torches and
    /// campfires their own colour.
    pub lantern_color: [f32; 3],
    /// A tint the sky and the floor under a roof are each multiplied through
    /// — see [`light::Tuning::ambient_color`]. `[1.0, 1.0, 1.0]` leaves the
    /// ambient the colour it was authored as.
    pub ambient_color: [f32; 3],
}

impl Light {
    /// The numbers the renderer draws with when nothing has been turned.
    pub fn new() -> Self {
        Self::from_tuning(light::Tuning::DEFAULT)
    }

    /// The renderer's own knobs, with everything the file said inside the domain
    /// the renderer states for it.
    ///
    /// **Clamped here rather than in a hand-written `Deserialize`**, unlike
    /// [`Zoom`]: the clamp is [`light::Tuning::clamped`], which belongs to the
    /// crate that knows what these numbers mean, and calling it on the way out
    /// covers the file *and* anything a slider could be talked into. What a
    /// per-field `Deserialize` would buy is a clamped value in the struct
    /// itself, and the struct itself is never read except through here.
    pub fn tuning(self) -> light::Tuning {
        light::Tuning {
            flame_radius: self.flame_radius,
            shadow_rays: light::ShadowRays::new(self.shadow_rays),
            brightness: self.brightness,
            reach: self.reach,
            sky: self.sky,
            ground: self.ground,
            sun: light::SunTuning {
                azimuth_degrees: self.sun_azimuth,
                rise_per_tile: self.sun_rise,
                color: self.sun_color,
                intensity: self.sun_intensity,
            },
            headlight_color: self.headlight_color,
            lantern_color: self.lantern_color,
            ambient_color: self.ambient_color,
        }
        .clamped()
    }

    /// And back: what the file writes, from the renderer's own numbers.
    fn from_tuning(tuning: light::Tuning) -> Self {
        Self {
            flame_radius: tuning.flame_radius,
            shadow_rays: tuning.shadow_rays.raw(),
            brightness: tuning.brightness,
            reach: tuning.reach,
            sky: tuning.sky,
            ground: tuning.ground,
            sun_azimuth: tuning.sun.azimuth_degrees,
            sun_rise: tuning.sun.rise_per_tile,
            sun_intensity: tuning.sun.intensity,
            sun_color: tuning.sun.color,
            headlight_color: tuning.headlight_color,
            lantern_color: tuning.lantern_color,
            ambient_color: tuning.ambient_color,
        }
    }
}

impl Default for Light {
    fn default() -> Self {
        Self::new()
    }
}

/// The HUD's scale on top of the monitor's density: egui's `zoom_factor`.
///
/// A wrapper with an invariant, so the field is private and both directions have
/// a name: a zoom of zero lays the whole UI out at nothing and a zoom of fifty
/// asks egui for a texture no GPU has. The bounds are egui's own Ctrl+`+` /
/// Ctrl+`-` range, which is what actually writes this value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Zoom(f32);

impl Zoom {
    /// The smallest and largest the HUD may be scaled to.
    pub const MIN: f32 = 0.5;
    pub const MAX: f32 = 4.0;

    /// Clamp a factor into the range. Takes anything, including what a
    /// hand-edited file offers, because that is where hostile numbers come from.
    ///
    /// A NaN clamps to 1.0 rather than propagating: `f32::clamp` panics on one,
    /// and a scale nobody can read is not worth a crash on startup.
    pub fn new(factor: f32) -> Self {
        if factor.is_nan() {
            return Self(1.0);
        }
        Self(factor.clamp(Self::MIN, Self::MAX))
    }

    /// The multiplier egui applies to every HUD point.
    ///
    /// This is egui's `zoom_factor`, not the operating system's monitor density;
    /// pass it directly to [`egui::Context::set_zoom_factor`].
    pub fn hud_scale_factor(self) -> f32 {
        self.0
    }
}

impl Default for Zoom {
    fn default() -> Self {
        Self(1.0)
    }
}

// Written and read as a bare number — `zoom = 1.25` — and *built through
// [`Zoom::new`] on the way in*, which is the whole point of the manual impl: a
// derived one would hand back whatever the file said.
impl Serialize for Zoom {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Zoom {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Zoom::new(f32::deserialize(deserializer)?))
    }
}

/// How much bigger than `fonts.mul`'s own pixels the HUD chat box's glyphs
/// draw.
///
/// Not [`Zoom`]: that scales the whole HUD, dev window included, and turning
/// it up to read the chat more easily also grows every slider in this window.
/// A bitmap face has no continuous size of its own to ask for either — every
/// glyph is baked at whatever pixels the art shipped
/// (`openshard_uofiles::font`'s own doc) — so this is an integer upscale
/// applied to the finished glyph quads, nearest-sampled the same way a camera
/// zoom step already grows a world sprite. See `App::draw`'s chat block for
/// where it is applied and why it stops at the classic face rather than also
/// reaching the TrueType path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChatScale(u32);

impl ChatScale {
    /// Unscaled — `fonts.mul`'s own pixels — and four times as big, past which
    /// a six-line journal no longer fits above the input line on a small
    /// window.
    pub const MIN: u32 = 1;
    pub const MAX: u32 = 4;

    /// Clamp into the range. Takes anything, including what a hand-edited file
    /// offers, the same reason [`Zoom::new`] does.
    pub fn new(factor: u32) -> Self {
        Self(factor.clamp(Self::MIN, Self::MAX))
    }

    /// The multiplier applied to each classic glyph quad.
    ///
    /// Pass this to the bitmap chat layout and rendering paths; TrueType text
    /// remains at its independently chosen pixel size.
    pub fn glyph_scale_factor(self) -> u32 {
        self.0
    }
}

impl Default for ChatScale {
    /// Twice `fonts.mul`'s own pixels — legible without a chat box that eats
    /// half the screen, and the reason a fresh `client_ui.toml` already reads
    /// bigger than the classic client's own chat did.
    fn default() -> Self {
        Self(2)
    }
}

// The same reason [`Zoom`]'s pair exists: written and read as a bare number,
// and built through [`ChatScale::new`] on the way in so a hand-edited `0` or
// `4000` cannot reach the renderer.
impl Serialize for ChatScale {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ChatScale {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(ChatScale::new(u32::deserialize(deserializer)?))
    }
}

/// How big a TrueType face's glyphs rasterize, as a multiple of
/// [`crate::TTF_BASE_PIXEL_HEIGHT`], when `App::ttf_font` is set.
///
/// [`ChatScale`]'s continuous twin, and the reason it need not be an integer
/// the way that one is: `fonts.mul` has no continuous size to ask for and
/// [`ChatScale::glyph_scale_factor`] upscales the *finished quad*,
/// nearest-sampled, so a fractional factor would split a pixel across two —
/// see that type's own doc. `fontdue` rasterizes a TrueType outline at
/// whatever pixel height it is asked for, analytically, so there is no
/// blockiness for a fractional multiple to introduce, and clamping this to
/// whole numbers would only make the slider coarser for no reason.
///
/// Unlike `ChatScale`, this reaches past the chat box: overhead speech, the
/// HUD's own speech line and every window's caption draw through the same one
/// [`openshard_client_render::atlas::TtfAtlas`] this scales, because a
/// TrueType face bakes one pixel size for all of them at once — see
/// `openshard_uofiles::ttf_font`'s "One face, not ten" doc. It lives on
/// [`Chat`] anyway, and not as a fourth field on [`Desk`] directly, because
/// the Chat tab is where a player who finds `--ttf-font`'s text too small to
/// read is already looking, and because the "back to the defaults" button
/// there should undo this the same click it undoes [`ChatScale`] with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TtfScale(f32);

impl TtfScale {
    /// Half `TTF_BASE_PIXEL_HEIGHT` and three times it — past which a
    /// six-line journal stops fitting above the compose line the same way
    /// [`ChatScale::MAX`] does.
    pub const MIN: f32 = 0.5;
    pub const MAX: f32 = 3.0;

    /// Clamp into the range. Takes anything, including what a hand-edited
    /// file offers, the same reason [`Zoom::new`] does.
    pub fn new(factor: f32) -> Self {
        if factor.is_nan() {
            return Self(1.0);
        }
        Self(factor.clamp(Self::MIN, Self::MAX))
    }

    /// The multiplier on [`crate::TTF_BASE_PIXEL_HEIGHT`].
    pub fn factor(self) -> f32 {
        self.0
    }
}

impl Default for TtfScale {
    fn default() -> Self {
        Self(1.0)
    }
}

// The same reason [`Zoom`]'s pair exists: written and read as a bare number,
// and built through [`TtfScale::new`] on the way in so a hand-edited `0` or
// `4000` cannot reach the atlas.
impl Serialize for TtfScale {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for TtfScale {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(TtfScale::new(f32::deserialize(deserializer)?))
    }
}

/// The HUD chat box's own look.
///
/// Three knobs. [`Chat::hue`] is about the player's own line rather than the
/// shard's: it tints the compose line and its caret, never a journal row
/// someone else's message already carries a hue of its own on the wire — see
/// `App::draw`'s chat block for where that split is made. The two scales
/// are about the same line's *size* instead, and only one of them ever
/// draws anything: [`Chat::scale`] when `App::ttf_font` is unset,
/// [`Chat::ttf_scale`] when it is — see [`TtfScale`]'s own doc for why they
/// cannot both apply at once.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct Chat {
    /// How big the classic face's glyphs draw — see [`ChatScale`].
    pub scale: ChatScale,
    /// How big a TrueType face's glyphs draw instead — see [`TtfScale`].
    pub ttf_scale: TtfScale,
    /// What the player's own compose line and caret are tinted, as a wire hue
    /// (`openshard_protocol::wire::Hue`'s own representation, not the type
    /// itself: this crate's `Deserialize` is what a hand-edited file can hand
    /// back nonsense through, and a raw `u16` has no invariant to violate).
    /// `0` is [`openshard_protocol::wire::Hue::NONE`] — the font's own ink,
    /// untinted.
    pub hue: u16,
}

/// The two independent gains the sound mixer exposes.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Audio {
    /// Positional effects such as strikes, spells and creature voices.
    pub effects: f32,
    /// Region and combat music.
    pub music: f32,
}

impl Default for Audio {
    fn default() -> Self {
        Self {
            effects: 0.8,
            music: 0.45,
        }
    }
}

impl Audio {
    /// Keep hand-edited persisted values inside the mixer's meaningful range.
    pub fn clamped(self) -> Self {
        let defaults = Self::default();
        Self {
            effects: if self.effects.is_finite() {
                self.effects
            } else {
                defaults.effects
            }
            .clamp(0.0, 1.0),
            music: if self.music.is_finite() {
                self.music
            } else {
                defaults.music
            }
            .clamp(0.0, 1.0),
        }
    }
}

/// Where the dev window sits inside the HUD, in egui's logical points.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Panel {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Where the operating system's window sits, in physical pixels.
///
/// Physical and not logical, for the same reason [`crate::App::create_window`]
/// asks for a physical size: a logical size means a different number of pixels
/// on every monitor, which is the opposite of restoring what was there.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    /// Outer position — the frame the compositor draws, which is what
    /// `Window::outer_position` answers and `with_position` takes.
    pub x: i32,
    pub y: i32,
    /// Inner size — the surface, which is what `with_inner_size` takes.
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
}

/// One monitor's physical rectangle.
///
/// This is not a saved [`Frame`]: its origin is the monitor's outer position
/// and its extent is the complete screen, while a frame holds an application's
/// outer position and inner size. Keeping the two rectangles apart makes the
/// restore check say which one it is reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Monitor {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Everything the HUD remembers.
///
/// `#[serde(default)]` on the struct rather than `Option` per field: a file
/// written by an older build is missing whole fields, and every one of them has
/// a sensible default. The two fields that *are* `Option` are absent in the
/// sense the style guide means — no window has been placed yet — and the first
/// run is exactly that.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Desk {
    /// Which page of the dev window is in front.
    pub tab: Tab,
    /// Whether the dev window is shown at all. Closed, it is reopened from the
    /// status strip's toggle or with F1.
    pub open: bool,
    /// Where the dev window was left. `None` before it has ever been drawn.
    pub panel: Option<Panel>,
    /// The HUD's scale.
    pub zoom: Zoom,
    /// Where the operating system's window was left. `None` on a first run, and
    /// ignored when it names a screen that is no longer there — see
    /// [`Desk::fits`].
    pub window: Option<Frame>,
    /// What the lighting has been turned to — [`Light`].
    ///
    /// Remembered for the same reason the window's own place is: a person who
    /// has found the shadow hardness they want should not have to find it again
    /// every launch. It is also what makes the file the honest record of *why*
    /// a frame looks the way it does — a screenshot of a client with the sky
    /// turned to nothing is otherwise indistinguishable from a bug report.
    pub light: Light,
    /// What the HUD chat box has been turned to — [`Chat`].
    pub chat: Chat,
    /// What the audio mixer has been turned to — [`Audio`].
    pub audio: Audio,
}

impl Default for Desk {
    fn default() -> Self {
        Self {
            tab: Tab::default(),
            // Shown until closed: a dev client whose panels are hidden by
            // default is a dev client that looks broken on a first run.
            open: true,
            panel: None,
            zoom: Zoom::default(),
            light: Light::new(),
            window: None,
            chat: Chat::default(),
            audio: Audio::default(),
        }
    }
}

/// What can go wrong reading or writing the file. A type rather than a string —
/// and the path is in it, because "permission denied" without one names nothing.
#[derive(Debug)]
pub enum DeskError {
    Read(std::path::PathBuf, std::io::Error),
    Write(std::path::PathBuf, std::io::Error),
    Parse(std::path::PathBuf, toml::de::Error),
    Encode(toml::ser::Error),
}

impl std::fmt::Display for DeskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeskError::Read(path, error) => write!(f, "reading {}: {error}", path.display()),
            DeskError::Write(path, error) => write!(f, "writing {}: {error}", path.display()),
            DeskError::Parse(path, error) => write!(f, "parsing {}: {error}", path.display()),
            DeskError::Encode(error) => write!(f, "encoding the UI state: {error}"),
        }
    }
}

impl std::error::Error for DeskError {}

impl Desk {
    /// Read the file, or the defaults if there is no file yet.
    ///
    /// A missing file is not an error: it is the first run, and it is the state
    /// a player gets by deleting the file. Anything else — unreadable, or
    /// present and not TOML — *is* one, and is handed back rather than swallowed,
    /// so the caller can say so before carrying on with the defaults. Silently
    /// defaulting on a parse error is how a typo eats a layout every launch and
    /// nobody finds out.
    pub fn load(path: &Path) -> Result<Self, DeskError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(DeskError::Read(path.to_path_buf(), error)),
        };
        toml::from_str(&text).map_err(|error| DeskError::Parse(path.to_path_buf(), error))
    }

    /// Write it out.
    pub fn save(&self, path: &Path) -> Result<(), DeskError> {
        let text = toml::to_string_pretty(self).map_err(DeskError::Encode)?;
        std::fs::write(path, text).map_err(|error| DeskError::Write(path.to_path_buf(), error))
    }

    /// Whether a saved frame still names somewhere a window can be seen.
    ///
    /// `monitors` are each screen's physical rectangles.
    /// A window restored onto a monitor that has since been unplugged opens
    /// offscreen, and an offscreen window is indistinguishable from a client that
    /// failed to start — so the test is whether the *title bar's* left corner
    /// lands inside some screen, which is the part you need in order to drag the
    /// rest back.
    ///
    /// Deliberately not a containment test of the whole rectangle: a window
    /// hanging a little off the right edge of a screen is a normal thing for a
    /// player to have left, and refusing to restore it would be the fix being
    /// worse than the bug.
    pub fn fits(frame: &Frame, monitors: &[Monitor]) -> bool {
        monitors.iter().any(|monitor| {
            frame.x >= monitor.x
                && frame.y >= monitor.y
                && frame.x < monitor.x.saturating_add(monitor.width as i32)
                && frame.y < monitor.y.saturating_add(monitor.height as i32)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_is_the_defaults_and_not_an_error() {
        let desk = Desk::load(Path::new("/nonexistent/openshard/client_ui.toml")).unwrap();
        assert_eq!(desk.tab, Tab::Camera);
        assert!(desk.open);
        assert!(desk.window.is_none());
    }

    #[test]
    fn what_is_written_is_what_comes_back() {
        let dir = std::env::temp_dir().join("openshard-desk-roundtrip");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("client_ui.toml");
        let desk = Desk {
            tab: Tab::Frames,
            open: false,
            panel: Some(Panel {
                x: 12.0,
                y: 34.0,
                width: 320.0,
                height: 240.0,
            }),
            zoom: Zoom::new(1.25),
            window: Some(Frame {
                x: -1920,
                y: 40,
                width: 1600,
                height: 900,
                maximized: true,
            }),
            light: Light {
                flame_radius: 0.25,
                shadow_rays: 16,
                brightness: 1.5,
                reach: 0.5,
                sky: 2.0,
                ground: 0.0,
                sun_azimuth: 90.0,
                sun_rise: 0.5,
                sun_intensity: 0.4,
                sun_color: [1.0, 0.9, 0.7],
                headlight_color: [1.0, 0.8, 0.6],
                lantern_color: [1.0, 0.5, 0.2],
                ambient_color: [0.8, 0.9, 1.0],
            },
            chat: Chat {
                scale: ChatScale::new(3),
                ttf_scale: TtfScale::new(1.5),
                hue: 33,
            },
            audio: Audio {
                effects: 0.25,
                music: 0.75,
            },
        };
        desk.save(&path).unwrap();
        let back = Desk::load(&path).unwrap();
        assert_eq!(back.tab, Tab::Frames);
        assert!(!back.open);
        assert_eq!(back.panel, desk.panel);
        assert_eq!(back.zoom, desk.zoom);
        assert_eq!(back.window, desk.window);
        assert_eq!(back.light, desk.light);
        assert_eq!(back.chat, desk.chat);
        assert_eq!(back.audio, desk.audio);
        std::fs::remove_file(&path).unwrap();
    }

    /// The file's own defaults are the renderer's own, field for field.
    ///
    /// The one statement that holds the two spellings of [`light::Tuning`]
    /// together — see [`Light`]'s own note. A field added on one side and not
    /// the other either fails to compile here or comes back with a number the
    /// renderer never chose, and both are this assertion.
    #[test]
    fn the_saved_light_is_the_renderers_own_default() {
        assert_eq!(Light::new().tuning(), light::Tuning::DEFAULT);
    }

    /// And a hand-edited file is an input: every number in it goes through the
    /// renderer's own clamp on the way to a frame, so a negative brightness or a
    /// ray count of nothing cannot reach the walk.
    #[test]
    fn a_hand_edited_light_is_clamped_on_the_way_out() {
        let desk: Desk =
            toml::from_str("[light]\nbrightness = -3.0\nshadow_rays = 0\nreach = 1e9\n").unwrap();
        let tuning = desk.light.tuning();
        assert_eq!(tuning.brightness, 0.0);
        assert_eq!(tuning.shadow_rays, light::ShadowRays::new(1));
        assert_eq!(tuning.reach, light::Tuning::MOST);
        // And what the file did not say is what the renderer draws with.
        assert_eq!(tuning.sky, light::Tuning::DEFAULT.sky);
    }

    /// A file is something a person edits, so the number in it is an input and
    /// not an invariant — the clamp has to survive deserialization, which is
    /// what the hand-written `Deserialize` is for.
    #[test]
    fn a_hand_edited_zoom_is_clamped_on_the_way_in() {
        let desk: Desk = toml::from_str("zoom = 400.0").unwrap();
        assert_eq!(desk.zoom.hud_scale_factor(), Zoom::MAX);
        let desk: Desk = toml::from_str("zoom = 0.0").unwrap();
        assert_eq!(desk.zoom.hud_scale_factor(), Zoom::MIN);
        let desk: Desk = toml::from_str("zoom = nan").unwrap();
        assert_eq!(desk.zoom.hud_scale_factor(), 1.0);
    }

    /// [`ChatScale`]'s own version of the zoom clamp above: a hand-edited `0`
    /// or a scale past what a six-line journal fits above the input line
    /// cannot reach `App::draw`.
    #[test]
    fn a_hand_edited_chat_scale_is_clamped_on_the_way_in() {
        let desk: Desk = toml::from_str("[chat]\nscale = 400").unwrap();
        assert_eq!(desk.chat.scale.glyph_scale_factor(), ChatScale::MAX);
        let desk: Desk = toml::from_str("[chat]\nscale = 0").unwrap();
        assert_eq!(desk.chat.scale.glyph_scale_factor(), ChatScale::MIN);
    }

    /// [`TtfScale`]'s own version of the same clamp — a hand-edited number
    /// outside its range, or not a number at all, cannot reach
    /// `Screen::sync_ttf_scale`.
    #[test]
    fn a_hand_edited_ttf_scale_is_clamped_on_the_way_in() {
        let desk: Desk = toml::from_str("[chat]\nttf_scale = 400.0").unwrap();
        assert_eq!(desk.chat.ttf_scale.factor(), TtfScale::MAX);
        let desk: Desk = toml::from_str("[chat]\nttf_scale = 0.0").unwrap();
        assert_eq!(desk.chat.ttf_scale.factor(), TtfScale::MIN);
        let desk: Desk = toml::from_str("[chat]\nttf_scale = nan").unwrap();
        assert_eq!(desk.chat.ttf_scale.factor(), 1.0);
    }

    /// A file written by a build that predates a field must not lose the rest of
    /// the layout to the one line it is missing.
    #[test]
    fn an_older_file_keeps_what_it_does_have() {
        let desk: Desk = toml::from_str("tab = \"world\"").unwrap();
        assert_eq!(desk.tab, Tab::World);
        assert!(desk.open);
        assert_eq!(desk.zoom, Zoom::default());
    }

    /// The unplugged-monitor case: the frame is on a screen that no longer
    /// exists, so restoring it would open the window where nobody can reach it.
    #[test]
    fn a_frame_on_a_vanished_screen_does_not_fit() {
        let frame = Frame {
            x: -1920,
            y: 0,
            width: 800,
            height: 600,
            maximized: false,
        };
        assert!(!Desk::fits(
            &frame,
            &[Monitor {
                x: 0,
                y: 0,
                width: 2560,
                height: 1440,
            }],
        ));
        assert!(Desk::fits(
            &frame,
            &[
                Monitor {
                    x: -1920,
                    y: 0,
                    width: 1920,
                    height: 1080,
                },
                Monitor {
                    x: 0,
                    y: 0,
                    width: 2560,
                    height: 1440,
                },
            ],
        ));
    }

    /// Hanging off the right edge is a normal place to have left a window, and
    /// the corner you drag it back by is still on screen.
    #[test]
    fn a_frame_hanging_off_an_edge_still_fits() {
        let frame = Frame {
            x: 2400,
            y: 1400,
            width: 800,
            height: 600,
            maximized: false,
        };
        assert!(Desk::fits(
            &frame,
            &[Monitor {
                x: 0,
                y: 0,
                width: 2560,
                height: 1440,
            }],
        ));
    }
}
