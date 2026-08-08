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
//! It is *not* the density the world's TTF text is baked at: that atlas takes
//! one pixel size at startup (see `TTF_BASE_PIXEL_HEIGHT`) and the HUD's scale
//! has no bearing on it.

use std::path::Path;

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
}

impl Tab {
    /// Every tab, in the order the bar draws them.
    ///
    /// One list, so the bar and anything that iterates the pages cannot come to
    /// disagree about which tabs exist.
    pub const ALL: [Tab; 5] = [Tab::Camera, Tab::Rig, Tab::Frames, Tab::World, Tab::Tile];

    /// What the bar calls it.
    pub fn title(self) -> &'static str {
        match self {
            Tab::Camera => "Camera",
            Tab::Rig => "Rig",
            Tab::Frames => "Frames",
            Tab::World => "World",
            Tab::Tile => "Tile",
        }
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

    /// The factor, for `egui::Context::set_zoom_factor`.
    pub fn raw(self) -> f32 {
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
            window: None,
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
    /// `monitors` is each screen's physical rectangle as `(x, y, width, height)`.
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
    pub fn fits(frame: &Frame, monitors: &[(i32, i32, u32, u32)]) -> bool {
        monitors.iter().any(|&(x, y, width, height)| {
            frame.x >= x
                && frame.y >= y
                && frame.x < x.saturating_add(width as i32)
                && frame.y < y.saturating_add(height as i32)
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
        };
        desk.save(&path).unwrap();
        let back = Desk::load(&path).unwrap();
        assert_eq!(back.tab, Tab::Frames);
        assert!(!back.open);
        assert_eq!(back.panel, desk.panel);
        assert_eq!(back.zoom, desk.zoom);
        assert_eq!(back.window, desk.window);
        std::fs::remove_file(&path).unwrap();
    }

    /// A file is something a person edits, so the number in it is an input and
    /// not an invariant — the clamp has to survive deserialization, which is
    /// what the hand-written `Deserialize` is for.
    #[test]
    fn a_hand_edited_zoom_is_clamped_on_the_way_in() {
        let desk: Desk = toml::from_str("zoom = 400.0").unwrap();
        assert_eq!(desk.zoom.raw(), Zoom::MAX);
        let desk: Desk = toml::from_str("zoom = 0.0").unwrap();
        assert_eq!(desk.zoom.raw(), Zoom::MIN);
        let desk: Desk = toml::from_str("zoom = nan").unwrap();
        assert_eq!(desk.zoom.raw(), 1.0);
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
        assert!(!Desk::fits(&frame, &[(0, 0, 2560, 1440)]));
        assert!(Desk::fits(&frame, &[(-1920, 0, 1920, 1080), (0, 0, 2560, 1440)]));
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
        assert!(Desk::fits(&frame, &[(0, 0, 2560, 1440)]));
    }
}
