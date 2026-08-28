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
use std::time::Duration;

use openshard_client_render::atlas::TextSize;
use openshard_client_render::follow::Rig;
use openshard_client_render::light;
use openshard_client_render::solid::Cut;
use openshard_client_render::{frame, interiors};
use openshard_protocol::speech::Font;
use openshard_protocol::world::RangedRange;
use serde::{Deserialize, Serialize};

use crate::crowd::Ease;
use crate::graphics::{GraphicsSettings, HighlightStyle, HighlightTarget, MELEE_SIGHT_REACH};

/// Where the state lives: beside `openshard.toml`, in the working directory.
///
/// The same place the operator's own config is, for the same reason — it is
/// per-checkout, visible, and deleting it is how you get the defaults back.
pub const PATH: &str = "client_ui.ron";

/// The format written by clients before persistent F1 settings existed.
///
/// It is read only as a one-way migration; all later writes use [`PATH`].
const LEGACY_PATH: &str = "client_ui.toml";

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
    /// Staff-only tools for creating test items in the character's backpack.
    Admin,
    /// What this client has been told about fighting, with the gaps visible.
    Combat,
}

impl Tab {
    /// Every tab, in the order the bar draws them.
    ///
    /// One list, so the bar and anything that iterates the pages cannot come to
    /// disagree about which tabs exist.
    pub const ALL: [Tab; 11] = [
        Tab::Admin,
        Tab::Combat,
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
            Tab::Chat => "Chat/Font",
            Tab::Audio => "Audio",
            Tab::Windows => "Windows",
            Tab::Admin => "Admin",
            Tab::Combat => "Combat",
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
/// **Fractional, and that is a choice with a visible
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

/// A real pixel size for each kind of text this client draws through a
/// TrueType face, and the matching fractional scale for `fonts.mul`.
///
/// **Sizes, not scales** — `docs/text_sizes.md`, whose whole subject this is.
/// The number in `client_ui.ron` is what reaches the rasterizer: eleven means
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

/// Which text face the client draws when an operator supplied a TrueType font.
///
/// `Automatic` keeps the behaviour of every saved desk made before this choice
/// existed: use the supplied face when there is one, otherwise the classic
/// bitmap face. The other two choices are remembered from F1.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FontFace {
    #[default]
    Automatic,
    Classic,
    TrueType,
}

impl FontFace {
    /// Whether this choice draws the supplied TrueType face.
    pub const fn uses_ttf(self, available: bool) -> bool {
        match self {
            Self::Automatic => available,
            Self::Classic => false,
            Self::TrueType => available,
        }
    }
}

/// One of the ten bitmap faces a client ships in `fonts.mul`.
///
/// Kept separate from the wire [`Font`]: a packet may carry any `u16` and is
/// validated by the asset atlas at draw time, while this is a player setting
/// with exactly ten meaningful choices.  The manual serde implementation
/// makes a hand-edited `client_ui.ron` stay inside that table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BitmapFont(u16);

impl BitmapFont {
    /// How many faces `fonts.mul` contains.
    pub const COUNT: u16 = openshard_uofiles::font::FONT_COUNT as u16;

    /// Build a face selection, clamped to a real face.
    #[must_use]
    pub const fn new(face: u16) -> Self {
        Self(if face >= Self::COUNT {
            Self::COUNT - 1
        } else {
            face
        })
    }

    /// Its `fonts.mul` index, for the F1 selector.
    #[must_use]
    pub const fn index(self) -> u16 {
        self.0
    }

    /// The wire-font value every bitmap text collector expects.
    #[must_use]
    pub const fn font(self) -> Font {
        Font(self.0)
    }
}

impl Default for BitmapFont {
    fn default() -> Self {
        Self::new(Font::DEFAULT.0)
    }
}

impl Serialize for BitmapFont {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for BitmapFont {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::new(u16::deserialize(deserializer)?))
    }
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

impl FontSizes {
    // These are calibration points for finished bitmap quads, not raster
    // sizes. They keep the old defaults: chat was 2x its eight-pixel base,
    // while the other roles were drawn at their native size.
    const BITMAP_SPEECH_REFERENCE: f32 = 8.0;
    const BITMAP_WINDOW_REFERENCE: f32 = 14.0;
    const BITMAP_TOOLTIP_REFERENCE: f32 = 14.0;
    const BITMAP_STACK_COUNT_REFERENCE: f32 = 11.0;

    pub(crate) fn bitmap_speech_scale(self) -> f32 {
        self.speech.pixels() / Self::BITMAP_SPEECH_REFERENCE
    }

    pub(crate) fn bitmap_window_scale(self) -> f32 {
        self.window.pixels() / Self::BITMAP_WINDOW_REFERENCE
    }

    pub(crate) fn bitmap_tooltip_scale(self) -> f32 {
        self.tooltip.pixels() / Self::BITMAP_TOOLTIP_REFERENCE
    }

    pub(crate) fn bitmap_stack_count_scale(self) -> f32 {
        self.stack_count.pixels() / Self::BITMAP_STACK_COUNT_REFERENCE
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
/// [`Chat::hue`] is about the player's own line rather than the
/// shard's: it tints the compose line and its caret, never a journal row
/// someone else's message already carries a hue of its own on the wire — see
/// `App::draw`'s chat block for where that split is made. [`Desk::fonts`]
/// controls the size for both face types.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct Chat {
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
    /// How big each text role draws — [`FontSizes`].
    ///
    /// On [`Desk`] rather than on [`Chat`], where the old multiplier lived,
    /// because these reach every piece of text this client draws and not one
    /// box: a window's caption, a tooltip, the count on a pile. Remembered for
    /// the same reason the window's own place is — a person who has found the
    /// size they can read should not have to find it again every launch.
    pub fonts: FontSizes,
    /// Which face draws text when this run has an operator-supplied TTF.
    pub font_face: FontFace,
    /// Replace every bitmap font requested by the shard or one of this
    /// client's windows with [`Desk::bitmap_font`].  This is the reference
    /// client's "override all fonts" setting; off preserves the packet's
    /// chosen face exactly.
    pub override_all_fonts: bool,
    /// The classic face used while [`Desk::override_all_fonts`] is on.
    pub bitmap_font: BitmapFont,
    /// What the audio mixer has been turned to — [`Audio`].
    pub audio: Audio,
    /// Movement preferences, saved beside the rest of the client UI state.
    pub movement: Movement,
    /// Values retained by F1's staff item creator.
    pub admin_item: AdminItem,
    /// Where the full item-art browser was left in F1.
    pub admin_catalogue: AdminCatalogue,
    /// What F1's combat recorder page was left showing.
    pub combat_recorder: CombatRecorder,
    /// The F1 controls whose live state belongs to the app rather than to an
    /// egui widget. `None` is a RON file written before this set existed;
    /// keeping that distinct lets its first launch retain the command-line
    /// diagnostic defaults.
    pub f1: Option<F1Settings>,
}

/// Persistent controls from F1 that are applied outside the HUD itself.
///
/// The shell owns text fields, sliders and layout directly, but the World,
/// Tile and Rig pages post requests to the application.  Keeping their stable
/// values here makes the configuration complete without serializing transient
/// diagnostics such as a frame dump, a navigation bake, a replay or a
/// hand-authored collision prism.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct F1Settings {
    pub draw_land: bool,
    pub draw_statics: bool,
    pub draw_items: bool,
    pub draw_houses: bool,
    pub draw_mobiles: bool,
    pub cutaway_disabled: bool,
    pub body_overlap_transparency_disabled: bool,
    pub time_of_day: bool,
    pub night: bool,
    pub show_terrain: bool,
    pub show_sight: bool,
    pub sight_reach: u8,
    pub show_interiors: bool,
    pub buildings: bool,
    pub z_slice: bool,
    pub z_slice_manual: Option<(i8, i8)>,
    pub floor_manual: Option<i8>,
    pub show_occluders: bool,
    pub show_solids: bool,
    pub solids_only: bool,
    pub solids_opaque: bool,
    pub solids_everything: bool,
    pub highlight: HighlightTarget,
    pub highlight_style: HighlightStyle,
    pub rig_plane_tau: f32,
    pub rig_lift_tau: f32,
    /// The finite threshold kept even while [`Self::rig_never_cut`] is on, so
    /// toggling the checkbox restores the last useful value.
    pub rig_lift_cut: f32,
    /// The rig's meaningful infinite cut: never cut the lift.
    pub rig_never_cut: bool,
    pub body_ease_tau: f32,
    pub scope_seconds: f32,
}

impl Default for F1Settings {
    fn default() -> Self {
        let rig = Rig::HARD;
        Self {
            draw_land: true,
            draw_statics: true,
            draw_items: true,
            draw_houses: true,
            draw_mobiles: true,
            cutaway_disabled: false,
            body_overlap_transparency_disabled: false,
            time_of_day: true,
            night: false,
            show_terrain: false,
            show_sight: false,
            sight_reach: MELEE_SIGHT_REACH.get(),
            show_interiors: false,
            buildings: false,
            z_slice: false,
            z_slice_manual: None,
            floor_manual: None,
            show_occluders: false,
            show_solids: false,
            solids_only: false,
            solids_opaque: false,
            solids_everything: false,
            highlight: HighlightTarget::default(),
            highlight_style: HighlightStyle::default(),
            rig_plane_tau: rig.plane_tau,
            rig_lift_tau: rig.lift_tau,
            rig_lift_cut: rig.lift_cut,
            rig_never_cut: !rig.lift_cut.is_finite(),
            body_ease_tau: crate::STARTUP_EASE.tau,
            scope_seconds: crate::SCOPE_SPAN.as_secs_f32(),
        }
    }
}

impl F1Settings {
    /// Clamp hand-edited values at the persistence boundary before they reach
    /// the renderer. The limits deliberately mirror the F1 widgets.
    fn finite(value: f32, default: f32, minimum: f32, maximum: f32) -> f32 {
        if value.is_finite() {
            value.clamp(minimum, maximum)
        } else {
            default
        }
    }

    pub fn rig(self) -> Rig {
        let defaults = Self::default();
        Rig {
            plane_tau: Self::finite(self.rig_plane_tau, defaults.rig_plane_tau, 0.0, 0.5),
            lift_tau: Self::finite(self.rig_lift_tau, defaults.rig_lift_tau, 0.0, 0.5),
            lift_cut: match self.rig_never_cut {
                true => f32::INFINITY,
                false => Self::finite(self.rig_lift_cut, 0.0, 0.0, 256.0),
            },
        }
    }

    pub fn ease(self) -> Ease {
        Ease {
            tau: Self::finite(self.body_ease_tau, crate::STARTUP_EASE.tau, 0.0, 0.5),
        }
    }

    pub fn scope_span(self) -> Duration {
        Duration::from_secs_f32(Self::finite(
            self.scope_seconds,
            crate::SCOPE_SPAN.as_secs_f32(),
            0.5,
            20.0,
        ))
    }

    /// Apply the settings that are owned by the graphics subsystem.
    pub fn apply_to_graphics(self, graphics: &mut GraphicsSettings) {
        graphics.drawing = frame::Draw {
            land: self.draw_land,
            statics: self.draw_statics,
            items: self.draw_items,
            houses: self.draw_houses,
            mobiles: self.draw_mobiles,
        };
        graphics.cutaway_disabled = self.cutaway_disabled;
        graphics.body_overlap_transparency_disabled = self.body_overlap_transparency_disabled;
        graphics.time_of_day = self.time_of_day;
        graphics.night = self.night;
        graphics.show_terrain = self.show_terrain;
        graphics.show_sight = self.show_sight;
        graphics.sight_reach = RangedRange::new(self.sight_reach).unwrap_or(MELEE_SIGHT_REACH);
        graphics.show_interiors = self.show_interiors;
        graphics.buildings = self.buildings;
        graphics.z_slice = self.z_slice;
        graphics.z_slice_view = self
            .z_slice_manual
            .map_or(interiors::ZSliceView::Auto, |(lower, upper)| {
                interiors::ZSliceView::Manual { lower, upper }
            });
        graphics.floor_view = self.floor_manual.map_or(interiors::FloorView::Auto, |relative| {
            interiors::FloorView::Manual { relative }
        });
        graphics.show_occluders = self.show_occluders;
        graphics.show_solids = self.show_solids;
        graphics.solids_only = self.solids_only;
        graphics.solids_opaque = self.solids_opaque;
        graphics.solids_everything = self.solids_everything;
        graphics.highlight = self.highlight;
        graphics.highlight_style = self.highlight_style;
    }

    /// Capture exactly the user-facing state, deliberately excluding caches,
    /// counters and the current player's height.
    pub fn from_runtime(graphics: &GraphicsSettings, rig: Rig, ease: Ease, scope_span: Duration) -> Self {
        Self {
            draw_land: graphics.drawing.land,
            draw_statics: graphics.drawing.statics,
            draw_items: graphics.drawing.items,
            draw_houses: graphics.drawing.houses,
            draw_mobiles: graphics.drawing.mobiles,
            cutaway_disabled: graphics.cutaway_disabled,
            body_overlap_transparency_disabled: graphics.body_overlap_transparency_disabled,
            time_of_day: graphics.time_of_day,
            night: graphics.night,
            show_terrain: graphics.show_terrain,
            show_sight: graphics.show_sight,
            sight_reach: graphics.sight_reach.get(),
            show_interiors: graphics.show_interiors,
            buildings: graphics.buildings,
            z_slice: graphics.z_slice,
            z_slice_manual: match graphics.z_slice_view {
                interiors::ZSliceView::Auto => None,
                interiors::ZSliceView::Manual { lower, upper } => Some((lower, upper)),
            },
            floor_manual: match graphics.floor_view {
                interiors::FloorView::Auto => None,
                interiors::FloorView::Manual { relative } => Some(relative),
            },
            show_occluders: graphics.show_occluders,
            show_solids: graphics.show_solids,
            solids_only: graphics.solids_only,
            solids_opaque: graphics.solids_opaque,
            solids_everything: graphics.solids_everything,
            highlight: graphics.highlight,
            highlight_style: graphics.highlight_style,
            rig_plane_tau: rig.plane_tau,
            rig_lift_tau: rig.lift_tau,
            rig_lift_cut: if rig.lift_cut.is_finite() {
                rig.lift_cut
            } else {
                0.0
            },
            rig_never_cut: !rig.lift_cut.is_finite(),
            body_ease_tau: ease.tau,
            scope_seconds: scope_span.as_secs_f32(),
        }
    }

    /// Fold a just-drawn F1 request into the shutdown snapshot.
    ///
    /// Requests intentionally apply on the next frame. A user may change a
    /// checkbox and close the client before that frame happens, however, and
    /// persisting the previous value in that case is indistinguishable from
    /// losing their setting. This updates only durable preferences; actions
    /// such as item creation, map editing and frame capture remain unspent.
    pub fn apply_pending_request(&mut self, request: &crate::shell::Request) {
        if let Some(rig) = request.rig {
            self.rig_plane_tau = rig.plane_tau;
            self.rig_lift_tau = rig.lift_tau;
            self.rig_lift_cut = if rig.lift_cut.is_finite() {
                rig.lift_cut
            } else {
                0.0
            };
            self.rig_never_cut = !rig.lift_cut.is_finite();
        }
        if let Some(ease) = request.ease {
            self.body_ease_tau = ease.tau;
        }
        if let Some(span) = request.scope_span {
            self.scope_seconds = span.as_secs_f32();
        }
        if let Some(draw) = request.draw {
            self.draw_land = draw.land;
            self.draw_statics = draw.statics;
            self.draw_items = draw.items;
            self.draw_houses = draw.houses;
            self.draw_mobiles = draw.mobiles;
        }
        if let Some(value) = request.cutaway_disabled {
            self.cutaway_disabled = value;
        }
        if let Some(value) = request.body_overlap_transparency_disabled {
            self.body_overlap_transparency_disabled = value;
        }
        if let Some(value) = request.time_of_day {
            self.time_of_day = value;
        }
        if let Some(value) = request.night {
            self.night = value;
        }
        if let Some(value) = request.show_terrain {
            self.show_terrain = value;
        }
        if let Some(value) = request.show_sight {
            self.show_sight = value;
        }
        if let Some(value) = request.sight_reach {
            self.sight_reach = value.get();
        }
        if let Some(value) = request.show_interiors {
            self.show_interiors = value;
        }
        if let Some(value) = request.buildings {
            self.buildings = value;
        }
        if let Some(value) = request.z_slice {
            self.z_slice = value;
        }
        if let Some(value) = request.z_slice_view {
            self.z_slice_manual = match value {
                interiors::ZSliceView::Auto => None,
                interiors::ZSliceView::Manual { lower, upper } => Some((lower, upper)),
            };
        }
        if let Some(value) = request.floor_view {
            self.floor_manual = match value {
                interiors::FloorView::Auto => None,
                interiors::FloorView::Manual { relative } => Some(relative),
            };
        }
        if let Some(value) = request.show_occluders {
            self.show_occluders = value;
        }
        if let Some(value) = request.show_solids {
            self.show_solids = value;
        }
        if let Some(value) = request.solids_only {
            self.solids_only = value;
        }
        if let Some(value) = request.solids_opaque {
            self.solids_opaque = value;
        }
        if let Some(value) = request.solid_cut {
            self.solids_everything = matches!(value, Cut::Nothing);
        }
        if let Some(value) = request.highlight {
            self.highlight = value;
        }
        if let Some(value) = request.highlight_style {
            self.highlight_style = value;
        }
    }
}

/// The two everyday movement conveniences.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Movement {
    /// Move at running pace unless shift is held.
    pub always_run: bool,
    /// Use a closed door when the next movement step meets it.
    pub auto_open_doors: bool,
}

impl Default for Movement {
    fn default() -> Self {
        Self {
            always_run: true,
            auto_open_doors: true,
        }
    }
}

/// The values last entered into F1's staff item creator.
///
/// They stay as strings while edited so a useful intermediate value such as
/// `0x` is not erased beneath the typist. The panel validates them only when
/// it decides whether creation may be submitted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AdminItem {
    pub graphic: String,
    pub hue: String,
    pub amount: String,
    pub stackable: bool,
}

/// What the combat recorder's page was left showing.
///
/// Kept beside the rest of the F1 state for its reason: somebody who has narrowed
/// the log to their own body and is chasing one defect across several launches
/// should not have to narrow it again each time. The *note* is remembered too —
/// a person marking the same stall over and over is typing the same word.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CombatRecorder {
    /// What the next mark will say.
    pub note: String,
    /// Show only this client's own body, rather than everyone in sight.
    pub only_me: bool,
    /// How many of the newest lines the page draws. The log keeps far more; this
    /// is what fits on a screen without turning the panel into the whole file.
    pub shown: usize,
}

impl Default for CombatRecorder {
    /// Narrowed to your own body and the last few dozen lines, because that is
    /// what somebody who has just seen their own character stop is looking at.
    fn default() -> Self {
        Self {
            note: String::new(),
            only_me: true,
            shown: 60,
        }
    }
}

/// The query and page of F1's installed-client item-art browser.
///
/// It is deliberately just navigation state: art stays in the client resource
/// archive and is decoded only for the page currently on screen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AdminCatalogue {
    pub query: String,
    pub category: AdminItemCategory,
}

/// Gameplay family used to narrow the administrator's complete item browser.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum AdminItemCategory {
    /// Every static graphic in the installed client.
    #[default]
    All,
    /// Items in the shard's classic weapon table.
    Weapons,
    /// Items in the shard's classic armour table.
    Armor,
}

impl Default for AdminCatalogue {
    fn default() -> Self {
        Self {
            query: String::new(),
            category: AdminItemCategory::All,
        }
    }
}

impl Default for AdminItem {
    fn default() -> Self {
        Self {
            graphic: "0x0eed".to_owned(),
            hue: "0".to_owned(),
            amount: "100".to_owned(),
            stackable: true,
        }
    }
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
            font_face: FontFace::default(),
            override_all_fonts: false,
            bitmap_font: BitmapFont::default(),
            light: Light::new(),
            window: None,
            chat: Chat::default(),
            audio: Audio::default(),
            movement: Movement::default(),
            admin_item: AdminItem::default(),
            admin_catalogue: AdminCatalogue::default(),
            combat_recorder: CombatRecorder::default(),
            f1: None,
        }
    }
}

/// What can go wrong reading or writing the file. A type rather than a string —
/// and the path is in it, because "permission denied" without one names nothing.
#[derive(Debug)]
pub enum DeskError {
    Read(std::path::PathBuf, std::io::Error),
    Write(std::path::PathBuf, std::io::Error),
    Parse(std::path::PathBuf, ron::error::SpannedError),
    ParseLegacy(std::path::PathBuf, toml::de::Error),
    Encode(ron::Error),
}

impl std::fmt::Display for DeskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeskError::Read(path, error) => write!(f, "reading {}: {error}", path.display()),
            DeskError::Write(path, error) => write!(f, "writing {}: {error}", path.display()),
            DeskError::Parse(path, error) => write!(f, "parsing {}: {error}", path.display()),
            DeskError::ParseLegacy(path, error) => {
                write!(f, "parsing legacy {}: {error}", path.display())
            }
            DeskError::Encode(error) => write!(f, "encoding the UI state: {error}"),
        }
    }
}

impl std::error::Error for DeskError {}

impl Desk {
    /// Read the file, or the defaults if there is no file yet.
    ///
    /// A missing file is not an error: it is the first run, and it is the state
    /// a player gets by deleting the file. When the RON file has not been
    /// created yet, the old TOML file is imported once. Anything else —
    /// unreadable, or present and malformed — is handed back rather than
    /// swallowed, so the caller can say so before carrying on with the
    /// defaults. Silently defaulting on a parse error is how a typo eats a
    /// layout every launch and nobody finds out.
    pub fn load(path: &Path) -> Result<Self, DeskError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Self::load_legacy(path);
            }
            Err(error) => return Err(DeskError::Read(path.to_path_buf(), error)),
        };
        ron::from_str(&text).map_err(|error| DeskError::Parse(path.to_path_buf(), error))
    }

    /// Write it out by atomically replacing the old file after the complete
    /// replacement has reached the filesystem. A partial write can therefore
    /// never turn the next launch into a reset of every F1 choice.
    pub fn save(&self, path: &Path) -> Result<(), DeskError> {
        let text =
            ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::new()).map_err(DeskError::Encode)?;
        let mut temporary = None;
        for attempt in 0..100 {
            let candidate = path.with_extension(format!("ron-writing-{}-{attempt}", std::process::id()));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&candidate)
            {
                Ok(mut file) => {
                    use std::io::Write;
                    if let Err(error) = file.write_all(text.as_bytes()).and_then(|()| file.sync_all()) {
                        let _ = std::fs::remove_file(&candidate);
                        return Err(DeskError::Write(candidate, error));
                    }
                    temporary = Some(candidate);
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(DeskError::Write(candidate, error)),
            }
        }
        let temporary = temporary.ok_or_else(|| {
            DeskError::Write(
                path.to_path_buf(),
                std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "could not reserve a temporary UI-state file",
                ),
            )
        })?;
        if let Err(error) = std::fs::rename(&temporary, path) {
            let _ = std::fs::remove_file(&temporary);
            return Err(DeskError::Write(path.to_path_buf(), error));
        }
        Ok(())
    }

    fn load_legacy(path: &Path) -> Result<Self, DeskError> {
        // `Desk::load` is also useful to tests and tools with arbitrary names;
        // only the canonical RON filename asks for a sibling migration.
        if path.file_name().is_none_or(|name| name != PATH) {
            return Ok(Self::default());
        }
        let legacy = path.with_file_name(LEGACY_PATH);
        let text = match std::fs::read_to_string(&legacy) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(DeskError::Read(legacy, error)),
        };
        toml::from_str(&text).map_err(|error| DeskError::ParseLegacy(legacy, error))
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
        let desk = Desk::load(Path::new("/nonexistent/openshard/client_ui.ron")).unwrap();
        assert_eq!(desk.tab, Tab::Camera);
        assert!(desk.open);
        assert!(desk.window.is_none());
        assert!(desk.movement.always_run);
        assert!(desk.movement.auto_open_doors);
    }

    #[test]
    fn what_is_written_is_what_comes_back() {
        let dir = std::env::temp_dir().join("openshard-desk-roundtrip");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("client_ui.ron");
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
            chat: Chat { hue: 33 },
            fonts: FontSizes {
                speech: TextSize::new(18.5),
                window: TextSize::new(13.0),
                tooltip: TextSize::new(12.5),
                stack_count: TextSize::new(9.5),
            },
            font_face: FontFace::Classic,
            override_all_fonts: true,
            bitmap_font: BitmapFont::new(7),
            audio: Audio {
                effects: 0.25,
                music: 0.75,
            },
            movement: Movement {
                always_run: false,
                auto_open_doors: true,
            },
            admin_item: AdminItem {
                graphic: "0x0f0e".to_owned(),
                hue: "0x0481".to_owned(),
                amount: "25".to_owned(),
                stackable: false,
            },
            admin_catalogue: AdminCatalogue {
                query: "0x0f52".to_owned(),
                category: AdminItemCategory::Weapons,
            },
            combat_recorder: CombatRecorder {
                note: "he stops here".to_owned(),
                only_me: false,
                shown: 120,
            },
            f1: Some(F1Settings {
                show_terrain: true,
                sight_reach: 10,
                z_slice: true,
                z_slice_manual: Some((-5, 22)),
                floor_manual: Some(-1),
                rig_plane_tau: 0.2,
                rig_never_cut: true,
                body_ease_tau: 0.1,
                scope_seconds: 12.0,
                ..F1Settings::default()
            }),
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
        assert_eq!(back.font_face, desk.font_face);
        assert_eq!(back.override_all_fonts, desk.override_all_fonts);
        assert_eq!(back.bitmap_font, desk.bitmap_font);
        assert_eq!(back.audio, desk.audio);
        assert_eq!(back.movement.always_run, desk.movement.always_run);
        assert_eq!(back.admin_catalogue, desk.admin_catalogue);
        assert_eq!(back.movement.auto_open_doors, desk.movement.auto_open_doors);
        assert_eq!(back.admin_item, desk.admin_item);
        assert_eq!(back.combat_recorder, desk.combat_recorder);
        assert_eq!(back.f1, desk.f1);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn old_ui_files_keep_their_startup_diagnostics_until_the_first_save() {
        let desk: Desk = ron::from_str("(tab: world)").unwrap();
        assert_eq!(desk.f1, None);
    }

    #[test]
    fn a_legacy_toml_file_is_imported_then_replaced_by_ron() {
        let dir = std::env::temp_dir().join(format!("openshard-desk-migration-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ron = dir.join(PATH);
        let legacy = dir.join(LEGACY_PATH);
        let _ = std::fs::remove_file(&ron);
        let _ = std::fs::remove_file(&legacy);
        std::fs::write(&legacy, "zoom = 1.25\n[movement]\nalways_run = false\n").unwrap();

        let desk = Desk::load(&ron).unwrap();
        assert_eq!(desk.zoom.hud_scale_factor(), 1.25);
        assert!(!desk.movement.always_run);
        assert!(!ron.exists());

        desk.save(&ron).unwrap();
        assert!(ron.exists());
        assert_eq!(Desk::load(&ron).unwrap().movement.always_run, false);
        std::fs::remove_file(&ron).unwrap();
        std::fs::remove_file(&legacy).unwrap();
    }

    #[test]
    fn hand_edited_f1_numbers_are_safe_to_apply() {
        let settings: F1Settings = ron::from_str(
            "(rig_plane_tau: NaN, rig_lift_tau: 9, rig_lift_cut: -1, body_ease_tau: NaN, scope_seconds: 100, sight_reach: 0)",
        )
        .unwrap();
        assert_eq!(settings.rig().plane_tau, F1Settings::default().rig_plane_tau);
        assert_eq!(settings.rig().lift_tau, 0.5);
        assert_eq!(settings.rig().lift_cut, 0.0);
        assert_eq!(settings.ease().tau, crate::STARTUP_EASE.tau);
        assert_eq!(settings.scope_span(), Duration::from_secs(20));
        assert_eq!(
            RangedRange::new(settings.sight_reach).unwrap_or(MELEE_SIGHT_REACH),
            MELEE_SIGHT_REACH
        );
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

    #[test]
    fn automatic_font_choice_preserves_the_old_startup_rule() {
        assert!(FontFace::Automatic.uses_ttf(true));
        assert!(!FontFace::Automatic.uses_ttf(false));
        assert!(!FontFace::Classic.uses_ttf(true));
        assert!(FontFace::TrueType.uses_ttf(true));
        assert!(!FontFace::TrueType.uses_ttf(false));
    }

    #[test]
    fn a_hand_edited_bitmap_font_stays_inside_fonts_mul() {
        let desk: Desk = ron::from_str("(override_all_fonts: true, bitmap_font: 999)").unwrap();
        assert!(desk.override_all_fonts);
        assert_eq!(desk.bitmap_font.index(), BitmapFont::COUNT - 1);
    }

    /// And a hand-edited file is an input: every number in it goes through the
    /// renderer's own clamp on the way to a frame, so a negative brightness or a
    /// ray count of nothing cannot reach the walk.
    #[test]
    fn a_hand_edited_light_is_clamped_on_the_way_out() {
        let desk: Desk = ron::from_str("(light: (brightness: -3.0, shadow_rays: 0, reach: 1e9))").unwrap();
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
        let desk: Desk = ron::from_str("(zoom: 400.0)").unwrap();
        assert_eq!(desk.zoom.hud_scale_factor(), Zoom::MAX);
        let desk: Desk = ron::from_str("(zoom: 0.0)").unwrap();
        assert_eq!(desk.zoom.hud_scale_factor(), Zoom::MIN);
        let desk: Desk = ron::from_str("(zoom: NaN)").unwrap();
        assert_eq!(desk.zoom.hud_scale_factor(), 1.0);
    }

    /// [`WindowScale`]'s own version of the same clamp. A `0` here is worse
    /// than a `0` in either of its neighbours: a window drawn at nothing is a
    /// window with nothing to click, so the value that would make the client
    /// unusable is exactly the one a hand-edited file must not be able to set.
    #[test]
    fn a_hand_edited_window_scale_is_clamped_on_the_way_in() {
        let desk: Desk = ron::from_str("(window_scale: 400.0)").unwrap();
        assert_eq!(desk.window_scale.factor(), WindowScale::MAX);
        let desk: Desk = ron::from_str("(window_scale: 0.0)").unwrap();
        assert_eq!(desk.window_scale.factor(), WindowScale::MIN);
        // Neither a crash nor a window with no pixels: `f32::clamp` panics on a
        // NaN, and drawing at one would leave nothing on screen to fix it with.
        let desk: Desk = ron::from_str("(window_scale: NaN)").unwrap();
        assert_eq!(desk.window_scale.factor(), WindowScale::MIN);
        // A fraction inside the range is kept as written — this is the knob's
        // whole point, and a rounding here would be the type quietly refusing
        // what the slider offers.
        let desk: Desk = ron::from_str("(window_scale: 1.5)").unwrap();
        assert_eq!(desk.window_scale.factor(), 1.5);
    }

    /// A file written before the knob existed says nothing about it, and what
    /// it must then draw is what it drew: the art's own size, windows exactly
    /// where and how big they were.
    #[test]
    fn a_file_without_a_window_scale_draws_at_the_arts_own_size() {
        let desk: Desk = ron::from_str("(zoom: 1.0)").unwrap();
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
        let desk: Desk = ron::from_str("(fonts: (speech: 400.0))").unwrap();
        assert_eq!(desk.fonts.speech.pixels(), TextSize::MAX);
        let desk: Desk = ron::from_str("(fonts: (speech: 0.0))").unwrap();
        assert_eq!(desk.fonts.speech.pixels(), TextSize::MIN);
        let desk: Desk = ron::from_str("(fonts: (speech: NaN))").unwrap();
        assert_eq!(desk.fonts.speech.pixels(), TextSize::MIN);
    }

    /// A size a person wrote is the size that is drawn, to the fraction.
    ///
    /// The assertion that says this is a *size* and not a factor: 13.5 in the
    /// file is 13.5 pixels at the rasterizer, with nothing multiplying it on
    /// the way. See `docs/text_sizes.md`.
    #[test]
    fn a_font_size_is_pixels_and_survives_a_round_trip() {
        let desk: Desk = ron::from_str("(fonts: (stack_count: 13.5))").unwrap();
        assert_eq!(desk.fonts.stack_count.pixels(), 13.5);
        let written = ron::to_string(&desk).unwrap();
        assert!(written.contains("stack_count:13.5"), "{written}");
    }

    #[test]
    fn bitmap_roles_use_the_same_sizes_with_fractional_scales() {
        let fonts = FontSizes {
            speech: TextSize::new(12.0),
            window: TextSize::new(17.5),
            tooltip: TextSize::new(10.5),
            stack_count: TextSize::new(13.75),
        };
        assert_eq!(fonts.bitmap_speech_scale(), 1.5);
        assert_eq!(fonts.bitmap_window_scale(), 1.25);
        assert_eq!(fonts.bitmap_tooltip_scale(), 0.75);
        assert_eq!(fonts.bitmap_stack_count_scale(), 1.25);
    }

    /// A file written before font sizes existed opens with the defaults, and
    /// an old `ttf_scale` in it is ignored rather than read as a size.
    ///
    /// It was a multiplier of a base this file no longer has, so there is
    /// nothing to migrate it *to* — see `docs/text_sizes.md`'s P2.
    #[test]
    fn old_per_face_scales_are_ignored_and_the_rest_of_the_file_survives() {
        let desk: Desk = ron::from_str("(zoom: 1.25, chat: (ttf_scale: 2.0, scale: 3))").unwrap();
        assert_eq!(desk.fonts, FontSizes::default());
        assert_eq!(desk.zoom.hud_scale_factor(), 1.25);
    }

    /// A file written by a build that predates a field must not lose the rest of
    /// the layout to the one line it is missing.
    #[test]
    fn an_older_file_keeps_what_it_does_have() {
        let desk: Desk = ron::from_str("(tab: world)").unwrap();
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
