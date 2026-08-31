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

use std::fmt::Write;

use openshard_map::grid::Tile;

use crate::camera::TileBounds;
use crate::light::{
    self,
    Lighting,
    Spot,
};

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
    /// The two records the whole pass rests on, drawn together — the tile off
    /// the row a fragment's id names, the fraction off its position. A wall
    /// reading as one flat cell of the board while its sprite rises out of it is
    /// the thing to look for: that is the wall naming its own tile, which is
    /// what lets its lit face escape its own shadow.
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
    /// Which way each pixel's surface looks: the G-buffer's normal plane, each
    /// axis a channel, `-1..1` mapped into `0..1` the way every normal map is
    /// read.
    ///
    /// `docs/lighting_rebuild.md` phase 2's own "done when", and the reason it
    /// is a *view* rather than a number is that the failure it looks for is a
    /// shape: a run of wall whose two faces are one colour, a corner whose
    /// halves did not split, a flight of steps whose treads and risers read the
    /// same. None of those is a value a test can name in advance, and all three
    /// are obvious in a picture.
    ///
    /// The shade to look for is the neutral grey in the middle, which is the
    /// zero vector — a surface with no known facing. It is the honest answer for
    /// a mobile and for a tree today, and phases 6 and 7 are the work of leaving
    /// less of it in a frame.
    Normal = 11,
    /// **Which primitive of the occlusion grid each pixel is a point of** — the
    /// `SolidId` the position plane's fourth channel carries, hashed to a
    /// colour, and black where a pixel is a point of none.
    ///
    /// The instrument four defects in a row asked for. 6f (a fragment of a
    /// flight naming the wrong tread), 6h (a fragment met against a face buried
    /// inside a merged solid), the lid whose corner pixel took a side face, and
    /// the seam that still lights up are all one sentence — *the fragment names
    /// the wrong box* — and every one of them was read by hand-decoding this
    /// channel through a throwaway edit to the shader. It is the join every
    /// shadow rule turns on: identity, D2's plane and the walk's own exemption
    /// all start from this number, and until now it was the one plane of the
    /// G-buffer with no picture.
    ///
    /// The colour carries no meaning beyond *same or different*, which is the
    /// whole question a picture of an identity can answer: three prime moduli
    /// over the id, so neighbouring ids are far apart in all three channels and
    /// a seam between two primitives reads at a glance.
    Solid = 12,
    /// [`View::Normal`] with only the pixels whose vector is **measured
    /// geometry** left in it, and black everywhere else.
    ///
    /// Four producers write the normal plane and one picture of it draws all
    /// four alike, so a box face the view ray actually met and a billboard
    /// turned towards the camera are two colours with nothing to say which is
    /// which. This is the half that is a statement about a shape that exists:
    /// the land patch's own normal, a mesh face's, and the face of the
    /// impostor's box a static's fragment met.
    ///
    /// The split is read off the vector rather than off the solid the fragment
    /// names — a static can meet a real box the occlusion grid holds no solid
    /// for, and the first version of this view drew every one of those as a
    /// sprite. See `blit.wesl`'s own note: for a static, a non-zero normal is
    /// the mark of a measured surface, because the one branch that leaves it
    /// zero is the picture the grid holds no volume for.
    ///
    /// Black is not a normal — `normal * 0.5 + 0.5` reaches it only at
    /// `(-1, -1, -1)`, which is not a unit vector — so nothing left out of the
    /// layer can be misread as a direction.
    NormalGeometry = 13,
    /// The other half: the pixels whose normal comes from the **picture**
    /// rather than from a shape — a mobile's billboard plane, and a static the
    /// grid holds no volume for, whose zero vector says only that nothing was
    /// measured.
    ///
    /// The pair is the instrument for *a fringe along a silhouette*. A fragment
    /// carries one normal, so a sprite drawn over geometry is a hole in
    /// [`View::NormalGeometry`] with the same shape filled in here — and which
    /// of the two pictures a fringe appears in names the producer that drew it,
    /// which a single normal plane cannot do.
    NormalSprites = 14,
    /// **The picture's own outline, at the art's resolution** — the fragments a
    /// neighbouring *texel* of the sprite ended, in orange.
    ///
    /// A magnified frame draws two edges at two resolutions. This one steps once
    /// a texel: one texel is [`crate::camera::Projection`]'s `scale` real pixels
    /// square, and `nearest` sampling means it cannot be finer, so at `4x` it is
    /// a four-pixel stair. That stair is what a person points at and calls the
    /// zigzags. [`View::SilhouetteBox`] is the other, one real pixel a step, and
    /// the two are one instrument.
    ///
    /// **They are not two halves of one outline, and this pair is how that was
    /// established.** Since a box miss stopped being discarded, the silhouette of
    /// the picture is entirely the art's and this layer is the whole of it; the
    /// box's line is a seam *inside* the picture. `docs/silhouettes.md`'s backlog
    /// asked whether the zigzag a person points at is even a silhouette, and it
    /// is — the finer line beside it is not.
    ///
    /// The other layer is drawn dim in this picture, because a line is read
    /// against the one beside it rather than across a keypress.
    ///
    /// **White is both, and it is the same white in [`View::SilhouetteBox`].**
    /// Not an overlap to be resolved: it is the seam reaching the outline, which
    /// is where the two resolutions genuinely meet along one line, and folding
    /// such a fragment into one layer would delete the subject to satisfy a
    /// partition.
    ///
    /// Only the statics pass stamps these bits, and only it can — see
    /// `place_format.wesl` for why neither `Meeting::outside` nor a
    /// neighbourhood test in `blit.wgsl` can answer instead.
    SilhouetteArt = 15,
    /// **And the seam inside it, at the fragment's resolution** — where the boxes
    /// an instance stands as run out and the rest of the same sprite carries on
    /// unmeasured, in blue, one real pixel a step.
    ///
    /// The line matters beyond its sharpness: the two sides of it are lit by
    /// different rules. Inside, a fragment is a point of a measured face and
    /// takes that face's normal; outside, it sits at the tile's centre with no
    /// facing and is lit from every side. See [`View::SilhouetteArt`] for the
    /// pair.
    ///
    /// A mobile, and a static the occlusion grid holds no shape for, can never
    /// appear in this layer — there is no box to run out — which is what makes
    /// the two of them its positive control.
    SilhouetteBox = 16,
}

impl View {
    /// Every view, in the order [`View::next`] walks them.
    pub const ALL: [Self; 17] = [
        Self::Lit,
        Self::Place,
        Self::Kind,
        Self::Height,
        Self::Normal,
        Self::NormalGeometry,
        Self::NormalSprites,
        Self::SilhouetteArt,
        Self::SilhouetteBox,
        Self::Solid,
        Self::Occluders,
        Self::Light,
        Self::Flames,
        Self::Shadow,
        Self::Reach,
        Self::Sun,
        Self::Sky,
    ];

    /// The next one round, which is what the key that cycles them does.
    ///
    /// # Panics
    ///
    /// Panics if [`ALL`](Self::ALL) omits this view.
    pub fn next(self) -> Self {
        let at = Self::ALL
            .iter()
            .position(|view| *view == self)
            .expect("View::ALL must contain every view");
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
            Self::Normal => "normal",
            Self::NormalGeometry => "normal-geometry",
            Self::NormalSprites => "normal-sprites",
            Self::SilhouetteArt => "silhouette-art",
            Self::SilhouetteBox => "silhouette-box",
            Self::Solid => "solid",
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
        write!(&mut out, "{y:5} ").expect("writing to a String cannot fail");
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
pub fn around(centre: Tile, half: i32) -> TileBounds {
    TileBounds {
        min_x: i32::from(centre.x) - half,
        max_x: i32::from(centre.x) + half,
        min_y: i32::from(centre.y) - half,
        max_y: i32::from(centre.y) + half,
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
        assert_eq!(View::Normal as u32, 11);
        assert_eq!(View::Solid as u32, 12);
        assert_eq!(View::NormalGeometry as u32, 13);
        assert_eq!(View::NormalSprites as u32, 14);
        assert_eq!(View::SilhouetteArt as u32, 15);
        assert_eq!(View::SilhouetteBox as u32, 16);
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
