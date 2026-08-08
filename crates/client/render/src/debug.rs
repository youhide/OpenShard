//! Ways of looking at a lit frame that are not the lit frame.
//!
//! Lighting is the one pass here whose output cannot be checked by looking at
//! it. A tile is dark for three different reasons — no flame reaches it, a wall
//! stopped the one that would, or the flame was never collected at all — and the
//! picture is identical in all three. So this module is the two ways of asking:
//! [`View`], which makes the blit draw one of the values it lit the frame *from*
//! instead of the frame, and [`diagram`], which prints a rectangle of the world
//! as characters for the message a failing test carries.
//!
//! Neither computes anything of its own. A view is a branch at the end of
//! `blit.wgsl` over the same bind group; a diagram calls [`crate::light::sample`],
//! which is the shader's arithmetic in Rust and is held to it by a parity test.
//! Something that lit its own copy of the world would answer about that copy —
//! see `docs/lighting.md`, decisions 8 and 9.

use crate::camera::TileBounds;
use crate::light::{self, Lighting, Spot};

/// What the blit puts on the screen.
///
/// The numbering is the contract with `blit.wgsl`, which switches on it: a value
/// stated in two files that no compiler compares, so the numbers are pinned in a
/// test below.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u32)]
pub enum View {
    /// The world, lit. What a player sees, and the only value that is not a
    /// diagnostic.
    #[default]
    Lit = 0,
    /// Which tile each pixel claims: a checkerboard of tiles, with where in its
    /// tile the pixel is as the other two channels.
    ///
    /// This is the attachment the whole pass rests on, drawn. A wall reading as
    /// one flat cell of the board while its sprite rises out of it is the thing
    /// to look for — that is the wall naming its own tile, which is what lets its
    /// lit face escape its own shadow.
    Place = 1,
    /// What drew each pixel: land, a static, a mobile, or nothing.
    ///
    /// The answer to "are the floor and the walls lit separately" — they are lit
    /// by the same formula from different tiles, and this is where you see which
    /// pixels belong to which.
    Kind = 2,
    /// The height each pixel was drawn at, as a ramp with a band every tile's
    /// worth of `z`.
    ///
    /// A cellar and the street above it are a few pixels apart in the image and
    /// eleven `z` units apart here; this is the channel that tells them apart.
    Height = 3,
    /// The occlusion grid over the picture: which tiles stop light, and whether
    /// the pixel's own height is inside the span they stop it at.
    Occluders = 4,
    /// The lighting alone — ambient plus every flame — with the art thrown away.
    ///
    /// The pools' shapes, with nothing under them to hide a step or a seam.
    /// Everything below `blit.wgsl`'s `KNEE` is drawn as itself, and what is
    /// above it — the middle of every pool, which a clamp used to draw as one
    /// flat white disc — is bent towards white without ever reaching it.
    Light = 5,
    /// The shadow term alone: how much of the *nearest reaching* flame survived
    /// the walk. White is an open path, black is a wall, grey is a partial
    /// occluder, and a fragment no flame reaches at all is blue.
    Shadow = 6,
    /// How many flames actually reached the pixel, as a ramp.
    ///
    /// The first question to ask when a tile is unexpectedly dark: nought means
    /// the search is about collection and radius, not about walls.
    Reach = 7,
    /// How much of the *sun* reached each pixel: white in the open, black in a
    /// wall's shadow, grey behind a pane.
    ///
    /// The view step 11 was built against. A lit patch on a floor behind a
    /// window is a shape, and a shape is the one thing a number cannot be
    /// checked as — this is where it is looked at.
    Sun = 8,
    /// How much of the sky each tile can see: white under open air, black under
    /// a roof, and a gradient across a doorway.
    ///
    /// The instrument the ambient split is judged with, and it is drawn on the
    /// ground rather than as a wireframe of the occluder boxes on purpose. The
    /// failure this field has is a tile that is *wrongly open* — an eave whose
    /// statics stand on the tile next to the one they cover, so the floor under
    /// it reads as sky. A box is drawn for what stands; a hole in a roof is
    /// exactly where there is no box, and would be invisible in the very view
    /// meant to find it. See `docs/lighting_world.md`'s backlog.
    Sky = 9,
    /// What the **flames alone** added, with the ambient taken out: the pool as a
    /// shape, on black.
    ///
    /// [`View::Light`] cannot answer this and it took a person looking at a frame
    /// to notice. It draws the ambient *plus* the flames through a curve, so a
    /// pool sits on a floor of `0.36` and is bent towards white above `0.6` — a
    /// torch's whole falloff is squeezed into the top third of the range, and it
    /// reads as one flat bright blob whichever shape it actually has. Subtract the
    /// ambient and the same pool is a gradient from white to black with nothing
    /// under it: a circle is obviously a circle, and a disc with a flat middle is
    /// obviously a disc with a flat middle.
    ///
    /// Every channel divided by nothing and clamped: what a flame adds is a
    /// multiplier of its own, `1.0` being "this pixel is doubled", and the values
    /// above that are the blown-out middle a person is entitled to see as blown
    /// out.
    Flames = 10,
}

impl View {
    /// Every view, in the order [`View::next`] walks them.
    pub const ALL: [Self; 11] = [
        Self::Lit,
        Self::Place,
        Self::Kind,
        Self::Height,
        Self::Occluders,
        Self::Light,
        Self::Flames,
        Self::Shadow,
        Self::Reach,
        Self::Sun,
        Self::Sky,
    ];

    /// The next one round, which is what the key that cycles them does.
    pub fn next(self) -> Self {
        let at = Self::ALL.iter().position(|view| *view == self).unwrap_or(0);
        Self::ALL[(at + 1) % Self::ALL.len()]
    }

    /// Whether this is the ordinary picture rather than a diagnostic.
    pub fn is_lit(self) -> bool {
        self == Self::Lit
    }

    /// What to call it on screen and in a log line.
    pub fn name(self) -> &'static str {
        match self {
            Self::Lit => "lit",
            Self::Place => "place",
            Self::Kind => "kind",
            Self::Height => "height",
            Self::Occluders => "occluders",
            Self::Light => "light",
            Self::Shadow => "shadow",
            Self::Reach => "reach",
            Self::Sun => "sun",
            Self::Sky => "sky",
            Self::Flames => "flames",
        }
    }
}

/// The characters a brightness is drawn with, dark to bright.
///
/// Ten steps, which is as many as an eye can tell apart in a monospaced grid,
/// and the first is a space so that an unlit room reads as empty rather than as
/// texture.
const RAMP: [char; 10] = [' ', '.', ':', '-', '=', '+', '*', '#', '%', '@'];

/// How bright a tile has to be to leave the first character of [`RAMP`].
///
/// The night ambient is about `0.36` and a lit tile is well past `1.0`, so the
/// diagram is scaled to that band rather than to `0..=1`: a picture where every
/// tile of an unlit room is the same character says nothing at all.
const FLOOR: f32 = 0.36;

/// And where the ramp saturates.
const CEILING: f32 = 1.6;

/// A rectangle of the world as characters: what is lit, what stands in the way,
/// and where the flames are.
///
/// For the message a failing test prints. An assertion about a leak that says
/// only "0.41 is not less than 0.37" costs the reader the whole room; this puts
/// the room in the failure, and the room is usually the answer.
///
/// Every tile is sampled at its middle, at `z`, with [`crate::light::sample`] —
/// the same arithmetic the shader runs. A tile that occludes is drawn as `#`
/// whatever its brightness, and a tile a flame stands on as `*`: both are facts
/// about the scene rather than about the light, and a reader needs them to
/// interpret everything else.
pub fn diagram(lighting: &Lighting, bounds: TileBounds, z: f32) -> String {
    let mut out = String::new();
    // A header of `x` hundreds and tens, so a column can be read off without
    // counting. The rows carry their `y` in full at both ends.
    for row in 0..2 {
        out.push_str("      ");
        for x in bounds.min_x..=bounds.max_x {
            let digit = match row {
                0 => (x / 10).rem_euclid(10),
                _ => x.rem_euclid(10),
            };
            out.push(char::from_digit(digit as u32, 10).unwrap_or('?'));
        }
        out.push('\n');
    }
    for y in bounds.min_y..=bounds.max_y {
        out.push_str(&format!("{y:5} "));
        for x in bounds.min_x..=bounds.max_x {
            out.push(cell(lighting, x, y, z));
        }
        out.push('\n');
    }
    out.push_str("      light '*'  occluder '#'  dark ' ' .:-=+*#%@ bright\n");
    out
}

/// One tile of a [`diagram`].
fn cell(lighting: &Lighting, x: i32, y: i32, z: f32) -> char {
    let middle = crate::geometry::Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
    let standing_here =
        |light: &light::Light| light.at.x.floor() as i32 == x && light.at.y.floor() as i32 == y;
    if lighting.lights.iter().any(standing_here) {
        return '*';
    }
    if lighting.occlusion.at(x, y).is_some() {
        return '#';
    }
    let brightness = light::sample(Spot::at(middle, z, (x, y)), lighting).brightness();
    let step = ((brightness - FLOOR) / (CEILING - FLOOR) * RAMP.len() as f32).floor();
    RAMP[(step.max(0.0) as usize).min(RAMP.len() - 1)]
}

/// The tiles a scene is drawn over: a square of `half` tiles either side of a
/// centre.
///
/// A diagram wants a small, stated rectangle — a camera's own bounds are two
/// hundred tiles across at the widest zoom and would print a wall of spaces.
pub fn around(centre: (u16, u16), half: i32) -> TileBounds {
    TileBounds {
        min_x: i32::from(centre.0) - half,
        max_x: i32::from(centre.0) + half,
        min_y: i32::from(centre.1) - half,
        max_y: i32::from(centre.1) + half,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The numbers `blit.wgsl` switches on. There is nothing in Rust that can
    /// read that file, so the contract is stated here and kept by a person.
    #[test]
    fn the_views_are_the_numbers_the_shader_has() {
        assert_eq!(View::Lit as u32, 0);
        assert_eq!(View::Place as u32, 1);
        assert_eq!(View::Kind as u32, 2);
        assert_eq!(View::Height as u32, 3);
        assert_eq!(View::Occluders as u32, 4);
        assert_eq!(View::Light as u32, 5);
        assert_eq!(View::Shadow as u32, 6);
        assert_eq!(View::Reach as u32, 7);
        assert_eq!(View::Sun as u32, 8);
        assert_eq!(View::Sky as u32, 9);
        assert_eq!(View::Flames as u32, 10);
    }

    /// Cycling visits every view and comes back. The key that does it is the
    /// only way into the diagnostics, so a view missing from `ALL` would be a
    /// view nobody can reach.
    #[test]
    fn cycling_visits_every_view_once() {
        let mut seen = Vec::new();
        let mut view = View::Lit;
        for _ in 0..View::ALL.len() {
            seen.push(view);
            view = view.next();
        }
        assert_eq!(view, View::Lit, "the cycle does not close");
        assert_eq!(seen, View::ALL.to_vec());
    }
}
