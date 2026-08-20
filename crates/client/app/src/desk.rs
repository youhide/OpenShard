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
//! It is *not* the size the world's TTF text is drawn at: that is
//! [`FontSizes`], a real pixel size per kind of text, and the HUD's scale has
//! no bearing on it. See `docs/text_sizes.md`.

use std::path::Path;

use openshard_client_render::atlas::TextSize;
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
    /// How big the client's own windows draw — [`WindowScale`].
    Windows,
}

impl Tab {
    /// Every tab, in the order the bar draws them.
    ///
    /// One list, so the bar and anything that iterates the pages cannot come to
    /// disagree about which tabs exist.
    pub const ALL: [Tab; 9] = [
        Tab::Camera,
        Tab::Rig,
        Tab::Frames,
        Tab::World,
        Tab::Tile,
        Tab::Light,
        Tab::Chat,
        Tab::Audio,
        Tab::Windows,
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
            Tab::Windows => "Windows",
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

/// How much bigger than its own art the client's windows draw: a bag, a doll,
/// a shop, a sheet, the amount picker — every window
/// [`crate::windows::Windows`] holds.
///
/// **Not [`Zoom`], and not `gump::Frame::scale`.** That pair is the *display's*
/// density — egui's `zoom_factor` on top of the monitor's, which the gump pass
/// is handed unchanged so that the art lines up with whatever egui drew beside
/// it (see `App::gump_scale`). This is the one number on top of it that says a
/// window is drawn bigger than the reference client drew it, and it exists
/// because that client had no display scaling at all: its windows are sized in
/// raw art pixels, which on a modern screen makes a container the size of a
/// postage stamp however good the monitor is.
///
/// **Fractional, unlike [`ChatScale`], and that is a choice with a visible
/// cost.** Gump art is five-bit pixel art sampled with `Nearest`, so a factor
/// between two whole numbers repeats some rows of a picture and not others: a
/// window's border comes out two pixels thick along part of an edge and one
/// along the rest, and a one-pixel seam between two pieces of a background can
/// open or close depending on where the fraction lands. A whole number cannot
/// do either. It is offered anyway because the alternative is a client whose
/// only sizes are *this* and *twice this* — 1.5 is the size a great many
/// screens actually want, and the seam is a fair price for it. Whole numbers
/// are still there for anybody who would rather have the clean picture.
///
/// One number for every window and not one per kind: the windows are dragged
/// around each other and drop items into each other, and a bag drawn at twice a
/// doll's size makes the icon that crosses between them change size in the
/// player's hand.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowScale(f32);

impl WindowScale {
    /// The art's own size — the reference client exactly — and three times it,
    /// past which a container is most of a small screen and the cascade drops
    /// later windows off the bottom of it.
    pub const MIN: f32 = 1.0;
    pub const MAX: f32 = 3.0;

    /// Clamp into the range. Takes anything, including what a hand-edited file
    /// offers, the same reason [`Zoom::new`] does — and a NaN becomes the art's
    /// own size rather than propagating, because `f32::clamp` panics on one and
    /// a window drawn at NaN is a window with no pixels to click.
    pub fn new(factor: f32) -> Self {
        if factor.is_nan() {
            return Self(Self::MIN);
        }
        Self(factor.clamp(Self::MIN, Self::MAX))
    }

    /// Real art pixels per drawn pixel, for both halves of the placement: pass
    /// it to `gump::place` when a window's quads go to the surface, and to
    /// [`crate::windows::OwnWindow::local_cursor`] when the pointer comes back
    /// the other way. Never zero and never NaN — see [`new`](Self::new) — so
    /// dividing by it is safe wherever it is read.
    pub fn factor(self) -> f32 {
        self.0
    }
}

impl Default for WindowScale {
    /// The art's own size, which is what this client drew before the knob
    /// existed: a saved file that predates it must not move anybody's windows
    /// or change their size on the first launch after an upgrade.
    fn default() -> Self {
        Self(Self::MIN)
    }
}

// [`Zoom`]'s pair again: written and read as a bare number, and built through
// [`WindowScale::new`] on the way in so a hand-edited `0` — a window drawn at
// nothing, which cannot be clicked to fix itself — cannot reach the pass.
impl Serialize for WindowScale {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for WindowScale {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(WindowScale::new(f32::deserialize(deserializer)?))
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

/// A real pixel size for each kind of text this client draws through a
/// TrueType face.
///
/// **Sizes, not scales** — `docs/text_sizes.md`, whose whole subject this is.
/// The number in `client_ui.toml` is what reaches the rasterizer: eleven means
/// eleven pixels tall, and a dense display multiplies it *before* the glyph is
/// drawn rather than stretching the glyph afterwards. `fontdue` shades an
/// outline analytically at whatever height it is asked for, so a fractional
/// size costs nothing and lands where it says it does.
///
/// One per *role*, and a role is what the text is rather than where it is
/// drawn: a shard's hover text is a tooltip whether it is over the world or
/// over a bag. Not one size with per-role offsets, because an offset is a
/// scale again — the point is that a person can write `stack_count = 11.0` and
/// get eleven pixels.
///
/// [`ChatScale`] is untouched by any of this and stays an integer: it upscales
/// finished `fonts.mul` quads, and a bitmap face has no continuous size to ask
/// for at all.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FontSizes {
    /// A line over a head, and the HUD chat box — the same voice in two
    /// places, so one size.
    #[serde(with = "text_size")]
    pub speech: TextSize,
    /// Captions inside this client's own windows: a button's word, a shop's
    /// row, a sheet's column.
    #[serde(with = "text_size")]
    pub window: TextSize,
    /// The shard's hover text.
    #[serde(with = "text_size")]
    pub tooltip: TextSize,
    /// The digits written on a pile — see `openshard_client_render`'s
    /// `items::stack_label`. Smaller than the rest on purpose: it sits in the
    /// corner of a 30-pixel icon, and it is a number *about* a thing rather
    /// than something anybody said.
    #[serde(with = "text_size")]
    pub stack_count: TextSize,
}

impl Default for FontSizes {
    /// Sixteen for a voice, fourteen for a window, eleven for a count.
    ///
    /// Sixteen is where the client's one baked TrueType height used to sit, chosen to land
    /// near `fonts.mul`'s own faces (roughly 8 to 14 pixels tall) with a line
    /// of air above them. The other two are that size read down for text that
    /// is denser on the screen than speech is: a window is full of captions,
    /// and a count has an icon's corner to fit into.
    fn default() -> Self {
        Self {
            speech: TextSize::new(16.0),
            window: TextSize::new(14.0),
            tooltip: TextSize::new(14.0),
            stack_count: TextSize::new(11.0),
        }
    }
}

/// A [`TextSize`] as the bare number a person writes in the file.
///
/// A `with` shim rather than a second newtype beside
/// [`openshard_client_render::atlas::TextSize`]: the clamping already lives on
/// that type, and a `FontSize(f32)` here would be the same invariant written
/// twice with the two free to disagree. Reading goes through `TextSize::new`,
/// so a hand-edited `0.0` or `400.0` is clamped the way every other knob in
/// this file clamps.
mod text_size {
    use openshard_client_render::atlas::TextSize;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(size: &TextSize, serializer: S) -> Result<S::Ok, S::Error> {
        size.pixels().serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<TextSize, D::Error> {
        Ok(TextSize::new(f32::deserialize(deserializer)?))
    }
}

/// The HUD chat box's own look.
///
/// Two knobs. [`Chat::hue`] is about the player's own line rather than the
/// shard's: it tints the compose line and its caret, never a journal row
/// someone else's message already carries a hue of its own on the wire — see
/// `App::draw`'s chat block for where that split is made. [`Chat::scale`] is
/// about that same line's *size*, and only while `App::ttf_font` is unset: a
/// TrueType face is sized in pixels by [`Desk::fonts`] instead, which is a
/// size rather than a multiple of the box's own and therefore does not live
/// here. See `docs/text_sizes.md`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct Chat {
    /// How big the classic face's glyphs draw — see [`ChatScale`].
    pub scale: ChatScale,
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
    /// How big the client's own windows draw — [`WindowScale`].
    ///
    /// Remembered for [`Light`]'s reason two fields down: a person who has
    /// found the size they can read a bag at should not have to find it again
    /// every launch.
    pub window_scale: WindowScale,
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
    /// How big each kind of TrueType text draws — [`FontSizes`].
    ///
    /// On [`Desk`] rather than on [`Chat`], where the old multiplier lived,
    /// because these reach every piece of text this client draws and not one
    /// box: a window's caption, a tooltip, the count on a pile. Remembered for
    /// the same reason the window's own place is — a person who has found the
    /// size they can read should not have to find it again every launch.
    pub fonts: FontSizes,
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
            window_scale: WindowScale::default(),
            fonts: FontSizes::default(),
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
            window_scale: WindowScale::new(1.5),
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
                hue: 33,
            },
            fonts: FontSizes {
                speech: TextSize::new(18.5),
                window: TextSize::new(13.0),
                tooltip: TextSize::new(12.5),
                stack_count: TextSize::new(9.5),
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
        assert_eq!(back.window_scale, desk.window_scale);
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

    /// [`WindowScale`]'s own version of the same clamp. A `0` here is worse
    /// than a `0` in either of its neighbours: a window drawn at nothing is a
    /// window with nothing to click, so the value that would make the client
    /// unusable is exactly the one a hand-edited file must not be able to set.
    #[test]
    fn a_hand_edited_window_scale_is_clamped_on_the_way_in() {
        let desk: Desk = toml::from_str("window_scale = 400.0").unwrap();
        assert_eq!(desk.window_scale.factor(), WindowScale::MAX);
        let desk: Desk = toml::from_str("window_scale = 0.0").unwrap();
        assert_eq!(desk.window_scale.factor(), WindowScale::MIN);
        // Neither a crash nor a window with no pixels: `f32::clamp` panics on a
        // NaN, and drawing at one would leave nothing on screen to fix it with.
        let desk: Desk = toml::from_str("window_scale = nan").unwrap();
        assert_eq!(desk.window_scale.factor(), WindowScale::MIN);
        // A fraction inside the range is kept as written — this is the knob's
        // whole point, and a rounding here would be the type quietly refusing
        // what the slider offers.
        let desk: Desk = toml::from_str("window_scale = 1.5").unwrap();
        assert_eq!(desk.window_scale.factor(), 1.5);
    }

    /// A file written before the knob existed says nothing about it, and what
    /// it must then draw is what it drew: the art's own size, windows exactly
    /// where and how big they were.
    #[test]
    fn a_file_without_a_window_scale_draws_at_the_arts_own_size() {
        let desk: Desk = toml::from_str("zoom = 1.0").unwrap();
        assert_eq!(desk.window_scale.factor(), 1.0);
    }

    /// The same clamp for a font size — a hand-edited number outside the
    /// range, or not a number at all, cannot reach the rasterizer.
    ///
    /// `NaN` lands on the smallest size rather than on a default: it is the
    /// key a glyph is packed under, and one `NaN` in an ordered map is a
    /// comparison that answers `false` to everything.
    #[test]
    fn a_hand_edited_font_size_is_clamped_on_the_way_in() {
        let desk: Desk = toml::from_str("[fonts]\nspeech = 400.0").unwrap();
        assert_eq!(desk.fonts.speech.pixels(), TextSize::MAX);
        let desk: Desk = toml::from_str("[fonts]\nspeech = 0.0").unwrap();
        assert_eq!(desk.fonts.speech.pixels(), TextSize::MIN);
        let desk: Desk = toml::from_str("[fonts]\nspeech = nan").unwrap();
        assert_eq!(desk.fonts.speech.pixels(), TextSize::MIN);
    }

    /// A size a person wrote is the size that is drawn, to the fraction.
    ///
    /// The assertion that says this is a *size* and not a factor: 13.5 in the
    /// file is 13.5 pixels at the rasterizer, with nothing multiplying it on
    /// the way. See `docs/text_sizes.md`.
    #[test]
    fn a_font_size_is_pixels_and_survives_a_round_trip() {
        let desk: Desk = toml::from_str("[fonts]\nstack_count = 13.5").unwrap();
        assert_eq!(desk.fonts.stack_count.pixels(), 13.5);
        let written = toml::to_string(&desk).unwrap();
        assert!(written.contains("stack_count = 13.5"), "{written}");
    }

    /// A file written before font sizes existed opens with the defaults, and
    /// an old `ttf_scale` in it is ignored rather than read as a size.
    ///
    /// It was a multiplier of a base this file no longer has, so there is
    /// nothing to migrate it *to* — see `docs/text_sizes.md`'s P2.
    #[test]
    fn an_old_ttf_scale_is_ignored_and_the_rest_of_the_file_survives() {
        let desk: Desk = toml::from_str("zoom = 1.25\n[chat]\nttf_scale = 2.0\nscale = 3").unwrap();
        assert_eq!(desk.fonts, FontSizes::default());
        assert_eq!(desk.chat.scale.glyph_scale_factor(), 3);
        assert_eq!(desk.zoom.hud_scale_factor(), 1.25);
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
