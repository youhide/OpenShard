//! Where a sprite's pixel is in the world, and which surface it is a pixel of.
//!
//! `docs/lighting_rebuild.md` phase 6. A static is drawn once, as its own
//! picture, and the geometry underneath that picture is found by *intersecting
//! the view ray with the boxes the occlusion grid already holds for it*. The
//! silhouette is the artist's; the volume is ours; and because there is only
//! ever one draw, there is no second silhouette to disagree with the first and
//! no border constant to hide the disagreement behind.
//!
//! # The ray is a constant
//!
//! [`crate::camera::project_exact`] is
//! `((x − y)·22, (x + y − 1)·22 − z·4)`. Hold a screen point fixed and the two
//! equations say `dx = dy` and `44·dx = 4·dz` — so every point that draws to one
//! pixel lies on a line in the single direction [`VIEW`], `(1, 1, 11)` in the
//! world's own units and exactly `(1, 1, 1)` once `z` is divided into tiles.
//! There is no per-fragment unprojection to write and no inverse matrix: a
//! fragment is a *slab test against a constant direction*, which is the whole of
//! why this is cheap enough to do per pixel.
//!
//! Two consequences worth stating because they simplify everything below. **No
//! axis is parallel to the ray** — every component of [`VIEW`] is non-zero — so
//! the parallel-axis branches [`crate::light::ray_vs_solid`] needs do not exist
//! here. And **the camera lies towards `+(1, 1, 1)`**: larger `x + y + z` draws
//! in front, which is what [`crate::depth`] sorts on and what
//! [`crate::solid::Side`] names when it says the three visible faces of a box
//! are `max.x`, `max.y` and `max.z`. So the surface a pixel shows is where the
//! ray *leaves* a box travelling along [`VIEW`], not where it enters one.
//!
//! # This is not a second `ray_vs_solid`
//!
//! [`crate::light::ray_vs_solid`] answers a *segment* between two given points,
//! clamped to `0..=1`, and reports how much of it a box swallowed. This answers
//! an unbounded ray in one fixed direction and reports **which face** it met,
//! which is the thing a normal comes from and the thing that function has no
//! notion of. They share the slab idea and nothing else; writing the shading
//! side in terms of the shadow side would mean threading an axis through a
//! function whose every caller wants a fraction.

use crate::light::{
    TileVec,
    WorldVec,
    Z_PER_TILE,
};

/// The direction every screen pixel's world line runs in, in world units —
/// tiles on `x` and `y`, `z` units on `z`.
///
/// See the module doc for the derivation. It points **towards the camera**: a
/// point further along it draws to the same pixel and stands in front.
pub const VIEW: WorldVec = WorldVec::new(1.0, 1.0, Z_PER_TILE);

/// A tile's width in virtual pixels — `camera::TILE_WIDTH`, restated as a float
/// because every arithmetic here is one.
const TILE_WIDTH: f32 = crate::camera::TILE_WIDTH as f32;

/// How far outside a box, in tiles, a meeting may measure and still be a
/// meeting rather than a miss: **one fragment of the screen grid.**
///
/// A fragment is a virtual pixel, and moving one along the screen's `x` axis
/// moves [`ray_from`]'s answer by `(1, −1) / TILE_WIDTH` of a tile — so the
/// nearest another sample can ever be is this, the diagonal of one step, in the
/// tile space [`crate::light::TileVec`] measures. That makes the number the
/// grid's own quantum rather than a tolerance: a miss shorter than this is an
/// edge that passed **between two fragments**, which no box anywhere can be
/// shaped to fix, and a miss longer than it is art that genuinely hangs off its
/// own volume and has a neighbouring sample to prove it.
///
/// **It replaced a numerical epsilon, and the epsilon was answering a different
/// question.** `TANGENT` was `1e-4` of a tile — thirty times the `3.5e-6` a
/// ray reaching a lid's own corner rounds to — and it was chosen so that
/// tangency would not read as a gap. It says nothing about sampling, and
/// sampling is what the misses turned out to be: `examples/discard_census`'s
/// positive control draws a whole-tile block's own silhouette against that
/// block's own box and reports **forty-four misses, every one of them under one
/// fragment, the worst at 0.71**. Those forty-four are one row of the tile's
/// width, they are on the screen as a dashed line along every tile seam, and
/// under the old constant each of them was a pixel the pass had no measurement
/// for — the tile's centre and no facing, which `blit.wesl` lights from every
/// side and a person reads as a glowing grid over the floor.
///
/// **Argued from both ends**, since a threshold picked from one is a fudge:
///
/// - **Above what the sample grid can produce.** The control's own worst is
///   `1 / TILE_WIDTH` — one fragment along a single world axis, `0.71` of this
///   — and a miss diagonal in both axes reaches exactly this. Anything smaller
///   leaves part of the seam unmeasured.
/// - **Under the distance to the next sample.** One fragment out is where the
///   neighbouring pixel is, so a miss past it is one the picture itself
///   distinguishes. Measured over Britain's 121×121: 12.3% of all misses are
///   under one fragment and the rest run to 133, so this cuts where the two
///   populations already separate rather than in the middle of one.
///
/// The number does **not** move with the zoom. The world passes draw at the
/// virtual resolution whatever the magnification — a real pixel is
/// `1 / scale` of a fragment, `docs/pixels.md` — so the grid this is a step of
/// is the same grid at every rung of the ladder.
pub const FRAGMENT: f32 = std::f32::consts::SQRT_2 / TILE_WIDTH;

/// How many virtual pixels tall one `z` unit draws — `camera::Z_STEP`, restated
/// here as the division of the two constants this module already carries so
/// that it cannot drift from either.
const Z_STEP: f32 = TILE_WIDTH / Z_PER_TILE;

/// Whether a box `lo.z..hi.z` tall has side faces a sample could ever land on.
///
/// **The same argument as [`FRAGMENT`], one dimension over.** A box's side face
/// draws as a band `(hi.z − lo.z) · Z_STEP` virtual pixels tall; under one
/// pixel, no fragment centre falls inside it, so naming it is answering with a
/// surface the picture does not contain. [`meets`] picks the face whose *exit*
/// comes first, and on the rim of a flat box that is a side one — a wall's
/// cosine in the middle of a floor, which is the lattice
/// `docs/lighting_rebuild.md` phase 6i names and `discard` was once introduced
/// for.
///
/// This used to be `hi.z > lo.z`, which was exact while a lid was a *plane*.
/// `docs/parity.md`'s P4 step 1 gave it `occlusion::LID_THICKNESS` — `1/64` of a
/// `z` unit, a **sixteenth of a pixel** tall — and the comparison against zero
/// stopped standing for anything: the face has area, and no screen this renderer
/// draws on can show it. Nothing else in the grid is near the line, which is
/// what makes one pixel a safe place to put it rather than a number to tune: a
/// panel and a body are whole `z` units tall, four pixels and up, and a lid is
/// a sixteenth. There is nothing between.
pub fn shows_a_side(lo_z: f32, hi_z: f32) -> bool {
    (hi_z - lo_z) * Z_STEP > 1.0
}

/// [`FRAGMENT`] as a step of `t` along [`VIEW`] — how much of the ray's own
/// parameter separates two meetings one sample apart.
///
/// One unit of `t` moves the point by `VIEW`, which is `(1, 1, Z_PER_TILE)` in
/// world units and therefore `(1, 1, 1)` in the tile space
/// [`crate::light::TileVec`] measures — so `t` and tiles differ by `√3`, and
/// this is the one place that conversion is written.
///
/// `√3` written out because `f32::consts::SQRT_3` is not stable on the MSRV.
const RIM: f32 = FRAGMENT / 1.732_050_8;

/// What a fragment whose ray met no box is answered with — the **fringe**, and
/// all three answers this renderer has ever given it.
///
/// A knob and not a setting: every state but the default is a way of looking at
/// the frame, in the sense `crate::debug::View` is. The three have each shipped
/// at some point and each was argued out of the others on a measurement, so what
/// this switch is *for* is the one instrument those measurements could not
/// replace — a person looking at two frames.
///
/// `docs/lighting_state.md`'s fringe entry has the numbers. **F2 cycles it** in
/// the client, the way F11 cycles [`crate::debug::View`]; `OPENSHARD_FRINGE`
/// ([`Fringe::from_env`]) is the same switch for a run that has to *start* in
/// one of them — a frame dump, a screenshot, a bug report.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum Fringe {
    /// **The shipped answer.** The nearest point of the box it came closest to,
    /// with that clamp's own first exit as the face. Every pixel the art drew is
    /// on screen, and the ones outside their box lie about *where they are* — by
    /// up to 133 fragments, four tiles.
    #[default]
    Clamp = 0,
    /// **Not drawn at all**, which is what `statics.wesl` did for one session.
    /// The picture is then clipped to the boxes: honest about position, and it
    /// takes 11.83% of drawn static art off the screen — 35.2% of a slate roof,
    /// and the whole top of a display case, which is what a person reported as
    /// furniture with a hole in it.
    Discard = 1,
    /// **The face the volume presents**, [`presented_face`] — measured and
    /// refused as a default, kept here because refusing it by argument is what
    /// this backlog item did twice already. Position is still the clamp; only
    /// the facing changes.
    Volume = 2,
}

impl Fringe {
    /// The next state, wrapping — what a key that cycles this switch does.
    pub fn next(self) -> Self {
        match self {
            Fringe::Clamp => Fringe::Discard,
            Fringe::Discard => Fringe::Volume,
            Fringe::Volume => Fringe::Clamp,
        }
    }

    /// This state's name, for a log line and for `OPENSHARD_FRINGE`.
    pub fn name(self) -> &'static str {
        match self {
            Fringe::Clamp => "clamp",
            Fringe::Discard => "discard",
            Fringe::Volume => "volume",
        }
    }

    /// What `OPENSHARD_FRINGE` asks for, [`Fringe::Clamp`] if it asks for
    /// nothing.
    ///
    /// **An unrecognised value panics rather than falling back.** A debug knob
    /// that silently ignores what it was told is a knob that reports the default
    /// as though it were the setting, and the whole use of this one is to say
    /// which of three pictures is on the screen.
    pub fn from_env() -> Self {
        let Some(asked) = std::env::var_os("OPENSHARD_FRINGE") else {
            return Fringe::Clamp;
        };
        for state in [Fringe::Clamp, Fringe::Discard, Fringe::Volume] {
            if asked == *state.name() {
                return state;
            }
        }
        panic!(
            "OPENSHARD_FRINGE is {asked:?}; it names one of clamp, discard, volume — \
             see impostor::Fringe"
        );
    }
}

/// Which face this box shows the camera at its largest, as an axis — the face a
/// miss takes under [`Fringe::Volume`], the rule this renderer **measured and
/// refused** as its default.
///
/// Nothing in the pipeline calls this on the shipped path. It is here because
/// `examples/discard_census.rs` calls it, and it does so to print what the rule
/// would have cost beside what the shipped one costs, out of one walk. The
/// alternative to keeping it is a document claiming a number no tool can take
/// again.
///
/// # What it was for, and what killed it
///
/// [`meets`] names the face whose exit comes first, which is the right answer
/// for a ray that goes *through* a box and an arbitrary one for a ray that
/// passes beside it: along a silhouette the first exit can flip between two
/// faces from one fragment to the next, and a smooth overhang shaded as a comb
/// is the serrated top edge `docs/lighting_state.md` lists. A face picked from
/// the box alone is the same face at every pixel of that box's overhang, so the
/// flip would have nowhere to happen — which is true, and is not the whole of
/// what the change does.
///
/// **Measured over Britain's `121×121`, per neighbouring pair of drawn pixels:**
/// the comb inside an overhang falls from `0.22%` of pairs to `0.02%`, and the
/// join between an overhang and the art it hangs off goes from `0.30%`
/// disagreeing to **`32.59%`** — `97.68%` for panels, a hard line drawn along
/// the top of every wall in the world. The reason is one number the argument for
/// this rule never had: **`91.79%` of the art bordering an overhang is on the
/// box's own lid.** An overhang hangs *above* its box, so the pixel beside it is
/// where the view ray grazes over the top face — a `z` face even on a wall
/// panel whose every other pixel is a side one. The exit rule agrees with that
/// neighbour by construction, because it is the same clamp one fragment along;
/// this one contradicts it by construction, because a panel presents its side.
///
/// And the control the same walk takes says the comb was never the larger
/// number: two neighbouring pixels that both *hit* disagree at `1.35%`, six
/// times the rate two misses do. The overhang is smoother than the picture it
/// hangs off.
///
/// # The score, which is the part worth keeping
///
/// **It is the face's own projected area and not a preference.** The
/// projection has one null direction, [`VIEW`], so a face's area on screen is
/// its area in the world times the cosine between its normal and that direction
/// — and for an axis-aligned face those are `extent[b] · extent[c]` and
/// `VIEW[a]`, up to the one length all three share. So a panel presents its own
/// side (a `PANEL_THICKNESS`-thin box is `20 · 1 · 1` against `2.2 · 0.2 · 1`),
/// a whole tile three `z` units tall presents its lid (`11` against `3`), and
/// nothing here is chosen by taste.
///
/// A lid is refused a side by [`shows_a_side`] for the same reason [`meets`]
/// refuses it one — a face under a pixel tall is not a face — though the areas
/// alone would already say so.
///
/// Ties go to `z`, then `y`, then `x`, which is [`meets`]'s own order.
pub fn presented_face(lo: WorldVec, hi: WorldVec) -> usize {
    let (lo, hi) = (lo.array(), hi.array());
    let extent = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
    let view = VIEW.array();
    let mut face = 2;
    let mut most = view[2] * extent[0] * extent[1];
    if shows_a_side(lo[2], hi[2]) {
        for a in [1, 0] {
            let area = view[a] * extent[(a + 1) % 3] * extent[(a + 2) % 3];
            if area > most {
                most = area;
                face = a;
            }
        }
    }
    face
}

/// One point of the view ray through a sprite fragment: the point at the
/// static's own base height.
///
/// `across` is how far the fragment is right of the column its sprite is centred
/// on, and `down` how far it is below the row its tile's own centre projects to,
/// both in virtual pixels — the two numbers `statics.wesl`'s vertex stage
/// already hands its fragment stage. `tile` and `base` are the static's own
/// place, which its instance already carries.
///
/// **This is exactly the inverse projection that shader's flat case already
/// spells**, and that is the point rather than a coincidence: a floor's pixel is
/// the point of the view ray at the tile's own height, so the branch that reads
/// a floor today is this ray at `t = 0`. Every other branch it carries — a face
/// pinned to an edge, a height recovered from `pixel_y` — is the same ray met
/// with a different plane, guessed from a stance instead of measured against a
/// box.
pub fn ray_from(tile: (i32, i32), base: f32, across: f32, down: f32) -> WorldVec {
    // `u − v = across/22` and `u + v = down/22 + 1`, solved. Written over the
    // whole tile width rather than the half so that neither line divides twice.
    WorldVec::new(
        tile.0 as f32 + (across + down) / TILE_WIDTH + 0.5,
        tile.1 as f32 + (down - across) / TILE_WIDTH + 0.5,
        base,
    )
}

/// Where a fragment of a **billboard** is: the point of its own view ray on the
/// vertical plane the sprite is drawn on.
///
/// `docs/lighting_rebuild.md` phase 7, and the half of it that is not a choice.
/// A mobile has no volume, so [`meets`] has nothing to meet and the pass falls
/// back to a *point* — the middle of the tile, with the height running down the
/// picture. That point is the same for **every pixel of a screen row**: a
/// billboard is forty pixels wide and all forty of them were told they are at
/// the tile's centre. Two things a person can see follow from it. The sprite is
/// lit flat across, because nothing about the light varies along a row. And
/// `blit.wesl`'s `dither` — which turns the sample pattern by an angle that
/// belongs to the *position* — hands one row one turn and the next row another,
/// so an eight-ray estimate lands in **horizontal bands** across the figure.
/// That is what a person standing next to a torch actually looks like today.
///
/// The plane is the one the picture is drawn on: it stands through the tile's
/// centre, it contains the vertical, and its horizontal direction is the one the
/// screen's own `x` axis runs along — `(1, -1)` in tiles, which is what
/// [`ray_from`] moves along when only `across` changes. So its normal is
/// `(1, 1, 0)`, the horizontal part of [`VIEW`]: the plane is turned towards the
/// camera, which is what a billboard *is*.
///
/// The arithmetic is [`ray_from`]'s ray met with that plane, and it is worth
/// stating that the height falls out unchanged: the meeting is at
/// `t = -down / TILE_WIDTH` along [`VIEW`], which takes `z` to
/// `base - down * Z_PER_TILE / TILE_WIDTH` — and `Z_PER_TILE / TILE_WIDTH` is
/// `1 / Z_STEP` exactly. The pass's old point already had the right `z`; what it
/// had wrong was `x` and `y`, and that is the whole of this function.
///
/// `statics.wesl`'s `billboard_at`, and the two are one formula.
pub fn billboard_at(tile: (i32, i32), base: f32, across: f32, down: f32) -> WorldVec {
    WorldVec::new(
        tile.0 as f32 + 0.5 + across / TILE_WIDTH,
        tile.1 as f32 + 0.5 - across / TILE_WIDTH,
        base - down * Z_PER_TILE / TILE_WIDTH,
    )
}

/// The billboard plane's own normal: `(1, 1, 0)` normalised, [`VIEW`]'s
/// horizontal part. `docs/lighting_rebuild.md` phase 7's "facing the camera"
/// candidate — never wrong, never interesting, and the one this phase picked:
/// the plane a mobile is drawn on has exactly one normal, so giving a fragment
/// of it *that* normal is not a choice among several so much as the plane's own
/// fact, stated rather than left as the zero vector.
///
/// `statics.wesl`'s equivalent, and the two are one formula — see
/// [`billboard_at`].
pub fn billboard_normal() -> [f32; 3] {
    let n = std::f32::consts::FRAC_1_SQRT_2;
    [n, n, 0.0]
}

/// Which run of a frame's [`Volume`] list belongs to one drawn static.
///
/// The association `docs/lighting_rebuild.md` phase 6 rests on: a fragment is
/// met against *these* boxes and no others, so one static's geometry cannot
/// reach a neighbour's picture. A range and not a list per instance because the
/// instance buffer is a flat row of words and this is two of them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Range {
    /// Where this static's boxes start in the frame's list.
    pub offset: u32,
    /// How many it has. Zero for a static this frame's grid holds nothing for,
    /// which is a real answer and not a missing one — see
    /// `statics::push_volumes`.
    pub count:  u32,
}

/// One box a static stands as, with the frame's own name for it.
///
/// **A volume, and for a fitted climbable that is not the same thing as the
/// grid's own solids.** `occlusion::Builder::add` decomposes a flight into
/// *surfaces* — a lid per tread, degenerate in `z`, and a riser per tread,
/// degenerate on the climb axis — whose union encloses nothing. That is right
/// for a shadow ray, which only ever asks what it crossed, and useless for a
/// view ray, which has to land on something for every pixel the art drew. For a
/// flight climbing *away* from the camera those surfaces do happen to cover the
/// visible side; for one climbing *towards* it every riser faces away and is
/// hidden behind its own tread, and the grid holds nothing at all where the art
/// draws the whole front of the staircase. So the volume here is one box a
/// tread — its strip, from the static's own base up to that tread's height —
/// and the grid is joined to rather than read from. Making the grid hold the
/// volume is `docs/lighting_geometry.md`'s question.
///
/// Everything that is not a fitted climbable *is* one of the grid's own boxes,
/// unchanged: a wall's panel, a floor's lid, a body's tile.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Volume {
    /// The low corner, in world units.
    pub lo:    WorldVec,
    /// And the high one.
    pub hi:    WorldVec,
    /// Which solid of this frame's grid a fragment of this box is a point of —
    /// `occlusion::SolidId::word`, so `SolidId::NOBODY` where the grid has none
    /// (a static the cutaway hid, or one past the draw ceiling).
    ///
    /// **One name a box, and for a tread that name is its own lid rather than
    /// its riser.** A fragment standing on the riser needs no name of its own:
    /// it lies exactly in that riser's plane, so a ray leaving it touches the
    /// riser at `t = 0` and nowhere else, which is precisely what phase 5's
    /// origin-touch rule already excuses. Carrying a second id a box would be a
    /// second answer to a question one rule has already closed.
    pub solid: Option<crate::occlusion::SolidId>,
    /// Which sides of its tile the **art** named for this picture — the same
    /// four bits `blit.wesl` mirrors, and
    /// [`crate::occlusion::named_edges`]'s answer rather than
    /// [`crate::occlusion::boxes_of`]'s.
    ///
    /// **The two are a different question and they differ on a climbable.**
    /// `boxes_of` answers *how this box is occluded with*, and it overrides the
    /// art on a fitted prism — `Edges::ANY`, to pick the exact slab test a solid
    /// takes. Filled from there, this field said a staircase named no face,
    /// which is the opposite of what its picture says; see `named_edges`.
    ///
    /// **The one thing about a box that says whether its faces are surfaces.**
    /// A panel's named side and a lid's `z` face are planes somebody drew: the
    /// silhouette detector measured them off the picture. A **body** —
    /// [`Edges::ANY`](crate::occlusion::Edges::ANY), "a thing that stands up
    /// whose facing the art would not name" — has none. Its box is the *tile's*
    /// own walls, a stand-in rather than a measurement, so the face
    /// [`meets`] names there is one nobody drew and a fragment of it claims a
    /// facing it has no right to. `statics.wesl` writes **no facing** for one,
    /// which is the zero vector `blit.wesl` lights from every side.
    ///
    /// It rides free: `lo` is a `vec3<f32>` and the word after it is alignment
    /// padding this side had to write anyway. See [`Volume::write`].
    pub edges: crate::occlusion::Edges,
}

impl Volume {
    /// One of the grid's own boxes as a volume a fragment can be met against.
    ///
    /// The only conversion between the two, so that
    /// [`crate::statics::push_volumes`] and a fixture stating a wall by hand
    /// cannot disagree about which end of a `Solid` is which — see
    /// [`crate::occlusion::Occlusion::box_of`], which is the one place a kind
    /// becomes geometry.
    ///
    /// `edges` is the mask [`crate::occlusion::boxes_of`] handed this box beside
    /// its shape, and it is an argument rather than something read back off
    /// `space` because a `Solid` is corners alone — the mask is what the *art*
    /// said, and the box is where that has already been spent. See
    /// [`Volume::edges`].
    pub fn of(
        space: &crate::solid::Solid,
        edges: crate::occlusion::Edges,
        solid: Option<crate::occlusion::SolidId>,
    ) -> Self {
        Self {
            lo: WorldVec::new(space.min.x as f32, space.min.y as f32, space.min.z as f32),
            hi: WorldVec::new(space.max.x as f32, space.max.y as f32, space.max.z as f32),
            solid,
            edges,
        }
    }

    /// What one box costs on the wire, and the stride `statics.wesl`'s own
    /// `Volume` has.
    ///
    /// Thirty-two rather than the twenty-four the corners add up to: a
    /// `vec3<f32>` aligns to sixteen, so the shader's struct has a word of
    /// padding after each vector whatever this side writes — and both of them
    /// carry something now. `solid` rides after `hi`, [`Volume::edges`] after
    /// `lo`. Neither costs a byte, which is why the first was carried before
    /// anything read it.
    pub const STRIDE: u64 = 32;

    /// This box as the shader's `Volume`, appended to `out`.
    ///
    /// The one place the layout is spelled on this side; the other is the struct
    /// in `statics.wesl`, and `a_volume_is_thirty_two_bytes_the_shader_can_read`
    /// is what holds the two together.
    pub fn write(&self, out: &mut Vec<u8>) {
        for value in self.lo.array() {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&u32::from(self.edges.raw()).to_le_bytes());
        for value in self.hi.array() {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&crate::occlusion::SolidId::word(self.solid).to_le_bytes());
    }
}

/// Where the view ray meets one box, and which of its faces that is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Meeting {
    /// The point, in world units, clamped into the box — see [`Meeting::outside`].
    pub at:      WorldVec,
    /// Which way the surface it met looks: the box's own camera-facing face on
    /// one axis, so always one of `+x`, `+y`, `+z`.
    ///
    /// The face the ray left by — and for a **miss**, which left by none, the
    /// face its clamp reached first. [`presented_face`] carries the rule that
    /// was measured against that and refused.
    pub normal:  WorldVec,
    /// How far, in **tiles**, the ray passed outside the box before the clamp
    /// pulled it back — at most [`FRAGMENT`] for a ray this grid can tell from
    /// one that went through, and [`Meeting::hit`] is that comparison spelled
    /// once.
    ///
    /// The measurement `docs/lighting_rebuild.md` phase 6 asks its own "done
    /// when" to carry, and the reason this is a number rather than an
    /// `Option<[f32; 3]>`: a sprite overhangs its own volume by a pixel or two
    /// wherever the fitted box is narrower than the art, and what matters about
    /// those pixels is not that they exist but *how far out* they are. A frame
    /// where this reaches a third of a tile is a frame whose boxes are wrong.
    pub outside: f32,
    /// How far along [`VIEW`] the meeting is, from the point [`ray_from`] gave.
    ///
    /// Only [`nearest`] reads it — the box with the largest `t` is the one in
    /// front — and it is on the struct rather than recomputed there because the
    /// slab test has already worked it out.
    pub t:       f32,
}

impl Meeting {
    /// Whether the ray went through the box at all, as against passing beside
    /// it and being clamped onto it.
    ///
    /// **"At all" is a question about this screen's grid and not about the
    /// reals.** A fragment is a sample, its centre lands where the grid put it
    /// rather than where an edge is, and an edge crossing between two samples
    /// leaves the nearer one measuring [`FRAGMENT`]-and-under outside a box it
    /// is plainly a pixel of. Asking for zero there answers "this pixel is a
    /// point of nothing" about a pixel drawn *on* the surface — see [`FRAGMENT`]
    /// for the control that measures it and the two ends the number is argued
    /// from.
    pub fn hit(self) -> bool {
        self.outside <= FRAGMENT
    }
}

/// Where the view ray through `from` meets the box `lo..=hi`.
///
/// Always answers. A ray that misses the box outright is answered with the
/// point on its camera-facing corner nearest the ray, and [`Meeting::outside`]
/// is what says so — one expression rather than a branch, because at tangency
/// the two cases are the same point and a branch there would be a seam.
///
/// The face is the one whose *exit* comes first: travelling from the camera
/// inwards a box is entered through whichever of its three visible faces the ray
/// reaches first, and `min` over the three exit parameters is that face.
///
/// **A miss keeps that face**, though a ray that never entered the box left it
/// by none: what it names there is whichever plane the clamp reached first.
/// [`presented_face`] is the rule that was written to replace it and refused on
/// a measurement — read it before proposing the replacement again.
///
/// **Ties go to `z`, then `y`, then `x`, and that order is a decision.** Two
/// exits are equal exactly on an edge or a corner of the box. Preferring `z` is
/// what gives a shared edge to the surface above it; the same choice, one
/// dimension down, is why [`nearest`] gives a tread's shared edge to the tread's
/// top.
///
/// **A face with no area is not a face, and that is a rule rather than a tie.**
/// A box flat in `z` presents `x` and `y` faces that are *lines*: the only face
/// of it a ray can meet is its own plane. Ordering the tie towards `z` used to
/// stand in for that and it only ever covered the exact tie — a ray that leaves
/// the footprint one rounding *before* it reaches the plane is not a tie, it is a
/// miss, and it was answered with a side face's normal. On the isometric grid
/// that is one pixel per tile corner, since the projection puts a tile's corner
/// on a whole pixel: a lattice of dots over a roof, each shaded as a wall in the
/// middle of a floor. Reported twice, and it is the second cause
/// `docs/lighting_rebuild.md` phase 6i's floor entry names.
///
/// **No box `Solid::box_of` builds is flat any more** — `docs/parity.md`'s P4
/// step 1 gave a lid a real span, which is the same defect's other
/// cure and the one that also answers for the lattice's *neighbours*, a rounding
/// out on either side. The rule stays because it is a rule about the geometry it
/// is handed and not about where that geometry came from: a caller may state a
/// degenerate box (`tests/frame.rs` and this module's own cases do), and a face
/// of no area is not one whatever authored it.
///
/// **And "no area" is measured against the grid, not against zero** — see
/// [`shows_a_side`]. Giving a lid a span brought the lattice partly back:
/// `LID_THICKNESS` is `1/64` of a `z` unit, so its side faces stopped being
/// lines and became a *quarter of a pixel* tall, which is still a surface no
/// sample can land on but is no longer caught by `hi.z > lo.z`. Measured by
/// `examples/discard_census`: 338 pixels over Britain's 121×121 would take a
/// side face off a flat box — the same order as the fourteen a person once
/// reported as holes in a roof.
pub fn meets(from: WorldVec, lo: WorldVec, hi: WorldVec) -> Meeting {
    // The internal slab test picks an axis (0/1/2) at runtime, which is
    // exactly why `[f32; 3]` is right here and nowhere else in this module —
    // see `WorldVec::array`.
    let (from, lo, hi) = (from.array(), lo.array(), hi.array());
    let view = VIEW.array();
    // The `z` face, which every box in this grid has: `Solid::box_of` gives a
    // panel `PANEL_THICKNESS` and everything else a whole tile, so `x` and `y`
    // are never degenerate and this face never has zero area.
    let mut axis = 2;
    let lid = (hi[2] - from[2]) / view[2];
    let mut exit = lid;
    // And the two side faces, which a lid does not have: both of their areas
    // carry the `z` extent as a factor, so one test retires both. The test is
    // [`shows_a_side`] — thinner than the grid that reads it, and there is no
    // face here to name.
    if shows_a_side(lo[2], hi[2]) {
        for a in [1, 0] {
            // No `abs(delta) < eps` case: every component of `VIEW` is positive,
            // so `lo` is always the near end of the slab and `hi` the far one,
            // and neither division can be by zero.
            let far = (hi[a] - from[a]) / view[a];
            if far < exit {
                exit = far;
                axis = a;
            }
        }
        // **And a side wins only by more than the picture can show** — the
        // [`FRAGMENT`] argument a third time, and the last place on this box the
        // grid was not being asked about. `shows_a_side` retires a face too thin
        // to hold a sample anywhere; this retires one that holds this sample by
        // less than the distance to the next.
        //
        // The band it takes is the box's own top edge, one fragment wide, where
        // the lid ends and the side begins — and along a projected diagonal one
        // fragment reads as the stepped dashed line a person reports as a seam.
        // What made it a *defect* rather than a rounding is what stands beyond
        // that edge: on a run of abutting bodies the art carries straight on
        // over the join, so the pixel handed a side face is drawn as tabletop
        // and lit as a wall — `blit.wesl` gives a vertical face a full cosine
        // where the lid took a grazing one, which was measured at fifteen times
        // the flame term across a single row.
        //
        // **It is a rule about this box and not about its neighbours**, which is
        // the property that matters: two counters standing flush are two boxes
        // whose join is a surface the world does not have, and nothing per-box
        // can see the other one. Deciding the edge by the grid instead settles
        // it for an isolated table and for a run of twenty alike, with no
        // dependence on whether anything merged.
        if lid - exit <= RIM {
            axis = 2;
            exit = lid;
        }
    }

    // The exit axis's own coordinate is **stated** and not computed. `exit` is
    // the `t` at which this ray reaches the plane `hi[axis]`, so the meeting is
    // on that plane by definition; `from + exit * VIEW` is a division and a
    // multiplication that only round back to it, and `VIEW.z` is `Z_PER_TILE`
    // — eleven, and no power of two — so the `z` round trip has nothing exact
    // about it and a GPU contracting the pair into an `fma` need not land where
    // a CPU does. Everything downstream leans on a fragment being a point *of*
    // the box it names, so the plane is taken from the number that chose it.
    // `tests/frame.rs`'s `a_sprite_fragment_is_a_point_of_the_primitive_it_
    // names` holds it as an equality, and `impostor.wesl`'s `meets` is the twin.
    let mut at = [
        from[0] + exit * view[0],
        from[1] + exit * view[1],
        from[2] + exit * view[2],
    ];
    at[axis] = hi[axis];
    let clamped = [
        at[0].clamp(lo[0], hi[0]),
        at[1].clamp(lo[1], hi[1]),
        at[2].clamp(lo[2], hi[2]),
    ];
    // In tiles, which means the `z` term is divided into them: a third of a tile
    // out sideways and a third of a tile out in height are the same miss, and
    // three `z` units are not. [`crate::light::TileVec`] is that space, and
    // `between` is the only place the division is written.
    let outside = TileVec::between(WorldVec::from_array(clamped), WorldVec::from_array(at)).length();

    // **A miss keeps this face too**, and that is a measurement rather than an
    // omission — see [`presented_face`], which is the rule that was weighed
    // against it and refused.
    let mut normal = [0.0; 3];
    normal[axis] = 1.0;
    Meeting {
        at: WorldVec::from_array(clamped),
        normal: WorldVec::from_array(normal),
        // A distance rather than the slab test's own `enter <= exit` predicate,
        // and the two cannot disagree because this one *is* the clamp above: a
        // ray that goes through the box is clamped by nothing and measures zero.
        outside,
        t: exit,
    }
}

/// Which of a static's own boxes a sprite fragment is a pixel of.
///
/// The box in front wins — largest `t` along [`VIEW`] — among those the ray
/// genuinely goes through. If it goes through none of them, the one it passes
/// closest to wins instead, which is the sprite overhanging its own volume and
/// is the case [`Meeting::outside`] exists to measure.
///
/// Ties go to the box the iterator yielded first, on both counts. For a flight
/// that is the grid's own order — a tread's top before its own riser, see
/// `occlusion::Part` — so the shared edge where a tread's lid stops and its
/// riser begins reads as the lid. That is a decision and not an accident: the
/// two surfaces meet along one line, and a horizontal one is what a person
/// looking at a step expects its outer edge to be.
///
/// `None` only for an empty iterator, which is a static the grid holds nothing
/// for — dropped by the cutaway, or past the draw ceiling. Its caller has to
/// have an answer for that; there is no box to invent one from here.
pub fn nearest<T, I>(from: WorldVec, boxes: I) -> Option<(T, Meeting)>
where
    I: IntoIterator<Item = (T, WorldVec, WorldVec)>,
{
    let mut best: Option<(T, Meeting)> = None;
    for (what, lo, hi) in boxes {
        let met = meets(from, lo, hi);
        let better = match &best {
            None => true,
            // A real hit always beats a miss, however near the miss came; two
            // hits are decided by which is in front, and two misses by which
            // came closer.
            Some((_, had)) => {
                match (had.hit(), met.hit()) {
                    (false, true) => true,
                    (true, false) => false,
                    (true, true) => met.t > had.t,
                    (false, false) => met.outside < had.outside,
                }
            }
        };
        if better {
            best = Some((what, met));
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The refused rule still answers what it was refused for**, which is what
    /// keeps `examples/discard_census`'s second column a measurement of the
    /// candidate rather than of whatever [`presented_face`] drifted into.
    ///
    /// Each of the three is the case the argument turns on: a panel presents its
    /// own long side, a whole tile of roof presents its lid even against three
    /// `z` units of side, and a lid presents its lid because it has no side to
    /// present. A person re-proposing the rule needs these to still hold before
    /// the numbers in that function's doc mean anything.
    #[test]
    fn a_box_presents_its_largest_face_to_the_camera() {
        let panel = (
            WorldVec::new(100.8, 101.0, 0.0),
            WorldVec::new(101.0, 102.0, 20.0),
        );
        assert_eq!(presented_face(panel.0, panel.1), 0, "a wall thin in x");
        let across = (
            WorldVec::new(100.0, 101.8, 0.0),
            WorldVec::new(101.0, 102.0, 20.0),
        );
        assert_eq!(presented_face(across.0, across.1), 1, "a wall thin in y");
        let roof = (WorldVec::new(100.0, 101.0, 0.0), WorldVec::new(101.0, 102.0, 3.0));
        assert_eq!(presented_face(roof.0, roof.1), 2, "a whole tile three z tall");
        // `11 · 1 · 1` against `1 · 1 · 11`: the tie the doc's own order settles.
        let cube = (
            WorldVec::new(100.0, 101.0, 0.0),
            WorldVec::new(101.0, 102.0, 11.0),
        );
        assert_eq!(presented_face(cube.0, cube.1), 2, "a tile-cube ties to z");
        let lid = (
            WorldVec::new(100.0, 101.0, 0.0),
            WorldVec::new(101.0, 102.0, 1.0 / 64.0),
        );
        assert_eq!(presented_face(lid.0, lid.1), 2, "a lid has no side to present");
    }

    /// The unit cube on tile `(100, 101)`, ten `z` tall.
    fn cube() -> (WorldVec, WorldVec) {
        (
            WorldVec::new(100.0, 101.0, 0.0),
            WorldVec::new(101.0, 102.0, 10.0),
        )
    }

    /// **A billboard's fragment is on the plane, and the plane is turned towards
    /// the camera.**
    ///
    /// Stated as the two properties that make it one rather than as the numbers
    /// it comes out with: every point is in the plane through the tile's centre
    /// whose normal is `VIEW`'s horizontal part, and the point is on the
    /// fragment's *own* view ray, so it draws where it was drawn. A formula that
    /// satisfies both is the meeting, and nothing else is.
    #[test]
    fn a_billboards_fragment_is_its_view_ray_met_with_a_plane_facing_the_camera() {
        let tile = (100, 101);
        let base = 7.0;
        let centre = [tile.0 as f32 + 0.5, tile.1 as f32 + 0.5];
        for across in [-20.0, -3.0, 0.0, 11.5, 22.0] {
            for down in [-8.0, 0.0, 4.0, 37.0] {
                let at = billboard_at(tile, base, across, down);
                // In the plane: the normal is `(1, 1, 0)` and the centre is on it.
                let off_plane = (at.x - centre[0]) + (at.y - centre[1]);
                assert!(off_plane.abs() < 1e-5, "{across}/{down}: {off_plane}");
                // And on the fragment's own view ray, which is what says it draws
                // on the pixel it was drawn on: the two differ by a multiple of
                // `VIEW` and by nothing else.
                let start = ray_from(tile, base, across, down);
                let along = [at.x - start.x, at.y - start.y, at.z - start.z];
                let view = VIEW.array();
                let t = along[0] / view[0];
                for axis in 0..3 {
                    assert!(
                        (along[axis] - t * view[axis]).abs() < 1e-4,
                        "{across}/{down}: axis {axis} of {along:?} is not {t} of {view:?}",
                    );
                }
            }
        }
    }

    /// And the height is the one the pass already drew with, to the bit.
    ///
    /// The point of the entry is what phase 7 does *not* change: `z` was right
    /// before — `base - down / Z_STEP` — and a new derivation that moved it would
    /// be lifting every mobile off the ground it stands on. `Z_PER_TILE /
    /// TILE_WIDTH` is `1 / Z_STEP` exactly, which is why the two spellings agree
    /// rather than nearly agree.
    #[test]
    fn a_billboards_height_is_what_the_pass_already_drew() {
        for down in [-8.0, 0.0, 4.0, 37.0, 130.0] {
            let at = billboard_at((100, 101), 7.0, 13.0, down);
            assert_eq!(at.z, 7.0 - down / crate::camera::Z_STEP as f32, "at {down}");
        }
    }

    #[test]
    fn a_volume_is_thirty_two_bytes_the_shader_can_read() {
        // The two spellings of one layout: this side's `write` and
        // `statics.wesl`'s `struct Volume`. Nothing but a person reading both
        // compares the *order*, so what is asserted here is every field's own
        // offset — a row that grew a byte would silently read every box past the
        // first at the wrong place, which is the failure `SpriteQuad::write`'s
        // own test was written from.
        let mut out = Vec::new();
        Volume {
            lo:    WorldVec::new(100.0, 101.0, 3.0),
            hi:    WorldVec::new(101.0, 102.0, 7.0),
            solid: Some(crate::occlusion::SolidId::new(9)),
            edges: crate::occlusion::Edges::EAST,
        }
        .write(&mut out);
        assert_eq!(out.len() as u64, Volume::STRIDE);
        assert_eq!(&out[0..4], &100f32.to_le_bytes(), "lo.x");
        assert_eq!(&out[8..12], &3f32.to_le_bytes(), "lo.z");
        assert_eq!(
            &out[12..16],
            &u32::from(crate::occlusion::Edges::EAST.raw()).to_le_bytes(),
            "the sides the art named, in what the vector's alignment left"
        );
        assert_eq!(&out[16..20], &101f32.to_le_bytes(), "hi.x");
        assert_eq!(&out[24..28], &7f32.to_le_bytes(), "hi.z");
        assert_eq!(&out[28..32], &9u32.to_le_bytes(), "and which solid it is");
    }

    #[test]
    fn the_view_direction_is_one_tile_per_tile_per_z_per_tile() {
        // The derivation in the module doc, restated as the thing a reader can
        // check: `Z_PER_TILE` is the whole of the third component, so a step of
        // one tile along `x` and along `y` is a step of one *tile* of height.
        assert_eq!(VIEW, WorldVec::new(1.0, 1.0, 11.0));
        assert_eq!(VIEW.z / Z_PER_TILE, 1.0);
    }

    #[test]
    fn a_point_of_the_ray_projects_to_the_offsets_it_was_built_from() {
        // The inverse is only worth having if it is one, so this goes forward
        // again through `project_exact` and compares — not against a second copy
        // of the algebra, against the projection itself.
        for (across, down) in [(0.0, 0.0), (7.0, 3.0), (-11.0, 20.0), (21.5, -9.25)] {
            let at = ray_from((100, 101), 5.0, across, down);
            let point = crate::camera::project_exact(crate::camera::WorldSpot {
                x: f64::from(at.x),
                y: f64::from(at.y),
                z: f64::from(at.z),
            });
            let centre = crate::camera::project_exact(crate::camera::WorldSpot {
                x: 100.5,
                y: 101.5,
                z: 5.0,
            });
            // A thousandth of a pixel, and the bound is `f32`'s rather than the
            // algebra's: a world coordinate of a hundred has about eight
            // millionths of a tile between neighbouring floats, which is two
            // ten-thousandths of a pixel once projected. The arithmetic itself
            // is exact — the same two divisions run backwards.
            assert!(
                (point.x - centre.x - f64::from(across)).abs() < 1e-3,
                "across: {across} gave {}",
                point.x - centre.x
            );
            assert!(
                (point.y - centre.y - f64::from(down)).abs() < 1e-3,
                "down: {down} gave {}",
                point.y - centre.y
            );
        }
    }

    #[test]
    fn every_point_of_one_ray_draws_to_one_pixel() {
        // The claim the whole module rests on: `VIEW` is the projection's own
        // null direction, so walking along it does not move the picture.
        let from = ray_from((100, 101), 0.0, 4.0, -6.0);
        let start = crate::camera::project_exact(crate::camera::WorldSpot {
            x: f64::from(from.x),
            y: f64::from(from.y),
            z: f64::from(from.z),
        });
        for t in [-3.0f32, 0.25, 1.0, 17.5] {
            let at = crate::camera::project_exact(crate::camera::WorldSpot {
                x: f64::from(from.x + t * VIEW.x),
                y: f64::from(from.y + t * VIEW.y),
                z: f64::from(from.z + t * VIEW.z),
            });
            assert!(
                (at.x - start.x).abs() < 1e-9 && (at.y - start.y).abs() < 1e-9,
                "t: {t} moved the pixel to {at:?} from {start:?}"
            );
        }
    }

    #[test]
    fn a_pixel_over_the_middle_of_a_cube_meets_its_lid() {
        let (lo, hi) = cube();
        // Forty pixels above the row the tile's own centre projects to, which on
        // a cube ten `z` tall — forty pixels of lift, at four a unit — is the
        // middle of its top face.
        let met = meets(ray_from((100, 101), 0.0, 0.0, -40.0), lo, hi);
        assert_eq!(met.normal, WorldVec::new(0.0, 0.0, 1.0), "at: {:?}", met.at);
        assert!(met.hit(), "{met:?}");
        assert!(
            (met.at.x - 100.5).abs() < 1e-4 && (met.at.y - 101.5).abs() < 1e-4,
            "the middle of the lid: {:?}",
            met.at
        );
        assert!((met.at.z - 10.0).abs() < 1e-4, "at: {:?}", met.at);
    }

    #[test]
    fn a_pixel_on_the_lower_right_of_a_cube_meets_its_east_face() {
        let (lo, hi) = cube();
        // Right of the sprite's own column and below the lid's near corner: the
        // hexagon's lower-right facet, which `crate::solid::Side::East` names.
        let met = meets(ray_from((100, 101), 0.0, 14.0, 0.0), lo, hi);
        assert_eq!(met.normal, WorldVec::new(1.0, 0.0, 0.0), "at: {:?}", met.at);
        assert!(met.hit(), "{met:?}");
        assert!((met.at.x - 101.0).abs() < 1e-4, "at: {:?}", met.at);
    }

    #[test]
    fn a_pixel_on_the_lower_left_of_a_cube_meets_its_south_face() {
        let (lo, hi) = cube();
        let met = meets(ray_from((100, 101), 0.0, -14.0, 0.0), lo, hi);
        assert_eq!(met.normal, WorldVec::new(0.0, 1.0, 0.0), "at: {:?}", met.at);
        assert!(met.hit(), "{met:?}");
        assert!((met.at.y - 102.0).abs() < 1e-4, "at: {:?}", met.at);
    }

    #[test]
    fn the_three_faces_a_meeting_can_name_are_the_three_a_camera_sees() {
        // Not a case list: a sweep over the cube's own screen rectangle, every
        // pixel of it, asserting that no meeting ever names a face turned away
        // from the viewer. `crate::solid::Side` is the claim being checked.
        let (lo, hi) = cube();
        let mut seen = [false; 3];
        let mut outside = 0.0f32;
        for step_down in -60..=60 {
            for step_across in -30..=30 {
                let from = ray_from((100, 101), 0.0, step_across as f32, step_down as f32);
                let met = meets(from, lo, hi);
                assert!(
                    met.normal.array().iter().all(|n| *n >= 0.0),
                    "a face turned away from the camera: {:?}",
                    met.normal
                );
                if met.hit() {
                    let axis = met.normal.array().iter().position(|n| *n == 1.0).unwrap();
                    seen[axis] = true;
                } else {
                    outside = outside.max(met.outside);
                }
            }
        }
        assert_eq!(seen, [true; 3], "the sweep should reach all three faces");
        assert!(outside > 0.0, "a sweep this wide should also miss the cube");
    }

    /// **What a ray exactly through the box's own corner is answered with** —
    /// `docs/parity.md` P5's G4, and it is a *record* rather than an
    /// endorsement.
    ///
    /// Two exits are equal exactly on an edge of the box, and there the rule is
    /// emergent from the order of three comparisons rather than stated anywhere:
    /// the loop runs `[1, 0]` and keeps a face only on a strict `<`, so `z` wins
    /// every tie it is in, and between `x` and `y` the answer is `+Y`. That last
    /// one is the whole of the window-parity artefact: a ray through a wall's
    /// **vertical** corner is answered with a face that has no area from here,
    /// and in a `+X` wall it drew a green line one pixel wide down every tile.
    ///
    /// The centring repair took the *primary* samples off that edge — the proof
    /// is `camera::tests::no_primary_sample_lands_on_a_whole_virtual_pixel` —
    /// and left this rule exactly as it was, reachable by anything that samples
    /// a boundary deliberately: `light::sample` at a face's own centre, a probe
    /// handed whole coordinates, a test. So the rule is written down here, with
    /// its ties built as exact `f32` equalities rather than approached, and the
    /// backlog says what it honestly wants to be instead (the face the
    /// *neighbouring* box continues, which no per-box meet can see).
    #[test]
    fn a_ray_through_a_boxs_own_corner_is_answered_by_the_order_of_three_ifs() {
        let (lo, hi) = cube();
        let (lo, hi) = (lo.array(), hi.array());
        let view = VIEW.array();

        // **The vertical corner**: the `x` and `y` exits are equal and the lid
        // is further on. Built by hand and not through `ray_from`, so the tie is
        // an exact equality of two `f32`s rather than two roundings that nearly
        // agree — and asserted as the premise, because a case that stopped being
        // a tie would test nothing while still passing.
        let from = [100.9, 101.9, 0.0];
        assert_eq!(
            hi[0] - from[0],
            hi[1] - from[1],
            "the premise: this ray reaches the `x` plane and the `y` plane at one `t`",
        );
        let met = meets(
            WorldVec::from_array(from),
            WorldVec::from_array(lo),
            WorldVec::from_array(hi),
        );
        assert_eq!(
            met.normal,
            WorldVec::new(0.0, 1.0, 0.0),
            "a ray through the vertical corner is answered `+Y` — docs/parity.md's window-parity \
             entry, recorded here and not repaired",
        );

        // **And it is a knife edge.** One float either side of the tie flips the
        // answer, which is what "emergent rather than stated" means: the face a
        // corner pixel gets is decided by a rounding, and half the world's walls
        // hide the result because their visible face is `+Y` anyway.
        let nudged = |axis: usize, by: f32| {
            let mut at = from;
            at[axis] = by;
            meets(
                WorldVec::from_array(at),
                WorldVec::from_array(lo),
                WorldVec::from_array(hi),
            )
            .normal
        };
        assert_eq!(
            nudged(0, f32::from_bits(from[0].to_bits() - 1)),
            WorldVec::new(0.0, 1.0, 0.0),
            "one ulp further from the `x` plane and `y` still exits first",
        );
        assert_eq!(
            nudged(1, f32::from_bits(from[1].to_bits() - 1)),
            WorldVec::new(1.0, 0.0, 0.0),
            "one ulp further from the `y` plane and the same corner reads `+X`",
        );

        // **The `z` ties, which the documented rule does cover.** A ray through
        // the box's own top corner leaves all three slabs at one `t`, and one
        // through the top edge leaves two — `hi[2] > lo[2]` lets the side faces
        // be considered at all, and neither is kept without a strict `<`.
        let corner = [100.5, 101.5, 4.5];
        assert_eq!((hi[0] - corner[0]) / view[0], (hi[2] - corner[2]) / view[2]);
        assert_eq!(
            meets(
                WorldVec::from_array(corner),
                WorldVec::from_array(lo),
                WorldVec::from_array(hi)
            )
            .normal,
            WorldVec::new(0.0, 0.0, 1.0),
            "ties go to `z`: the lid is the surface a shared edge belongs to",
        );
        let edge = [100.0, 101.5, 4.5];
        assert_eq!((hi[1] - edge[1]) / view[1], (hi[2] - edge[2]) / view[2]);
        assert!(
            (hi[0] - edge[0]) / view[0] > (hi[2] - edge[2]) / view[2],
            "and `x` is not in this tie"
        );
        assert_eq!(
            meets(
                WorldVec::from_array(edge),
                WorldVec::from_array(lo),
                WorldVec::from_array(hi)
            )
            .normal,
            WorldVec::new(0.0, 0.0, 1.0),
            "the `y`/`z` edge is the lid's too",
        );
    }

    /// **A lid has no side faces, and a ray through its corner says so.**
    ///
    /// The other half of the rule above, and the one that *is* stated: the two
    /// side slabs are only considered when `hi[2] > lo[2]`, so a flat box
    /// answers with its own plane wherever a ray meets it. Without that guard
    /// the tie order would stand in for it and only for the exact tie — which
    /// is the lattice of wall-shaded dots over every roof that `meets`'s own doc
    /// reports.
    #[test]
    fn a_lid_answers_with_its_own_plane_at_the_corner_a_body_would_call_a_wall() {
        let lo = WorldVec::new(100.0, 101.0, 7.0);
        let hi = WorldVec::new(101.0, 102.0, 7.0);
        // The same vertical-corner ray as above, and the same exact tie.
        let from = WorldVec::new(100.9, 101.9, 7.0);
        assert_eq!(hi.x - from.x, hi.y - from.y);
        assert_eq!(
            meets(from, lo, hi).normal,
            WorldVec::new(0.0, 0.0, 1.0),
            "a face with no area is not a face: a lid's only face is its plane",
        );
    }

    /// **The seam a person reports on a run of counters, and the control beside
    /// it.**
    ///
    /// A body's own top edge is one line on the screen: on one side of it the
    /// ray leaves by the lid, on the other by a side face, and the fragments
    /// *on* it beat the lid by less than the distance to the next sample. Those
    /// are [`RIM`]'s, and they are the whole of the dashed line — on a run of
    /// abutting bodies the art carries straight over the join, so a pixel drawn
    /// as tabletop was being lit as a wall.
    ///
    /// The two halves are asserted together on purpose. A rule that took the
    /// *visible* side of a table with it would be a worse defect than the one it
    /// repairs and would still pass a test that only looked at the edge, so the
    /// control walks the same column down the face and holds that every fragment
    /// past the first keeps its side.
    #[test]
    fn a_bodys_own_top_edge_reads_as_its_lid_and_the_side_below_it_does_not() {
        // One tile, five `z` units — what `examples/seam_probe.rs` prints for
        // the furniture the report came from.
        let (tile, base) = ((100, 101), 30.0);
        let lo = WorldVec::new(100.0, 101.0, base);
        let hi = WorldVec::new(101.0, 102.0, base + 5.0);
        // Down the middle column of the sprite, one virtual pixel a step, from
        // the lid into the face below it. `across = 0` is the column the tile's
        // own corner projects to, where the `x` edge is met.
        let faces: Vec<WorldVec> = (0..24)
            .map(|down| meets(ray_from(tile, base, 0.5, down as f32 + 0.5), lo, hi).normal)
            .collect();
        let lid = WorldVec::new(0.0, 0.0, 1.0);
        let first_side = faces
            .iter()
            .position(|face| *face != lid)
            .expect("this column leaves the lid and reaches a side face");
        assert!(
            faces[..first_side].iter().all(|face| *face == lid),
            "the lid is answered as the lid all the way to the edge: {faces:?}",
        );
        // **The control**: everything past the edge is the side, uninterrupted.
        // Without it this test would pass on a rule that answered `lid`
        // everywhere and deleted the table's own front.
        assert!(
            faces[first_side..].iter().all(|face| *face != lid),
            "past the edge the face is the side's, and stays it: {faces:?}",
        );
        assert!(
            first_side > 0 && first_side < faces.len() - 4,
            "the edge is inside this column rather than at either end: {first_side}",
        );
        // **And the edge is where the rim puts it, from both sides** — which is
        // the assertion that fails if the rule is removed *or* widened, and the
        // reason it is stated as a pair. The last lid row beats its own side
        // exit by less than a fragment, which is what had been making it a side;
        // the first side row beats it by more, which is what keeps this a
        // one-sample band instead of a rule that eats a table's front.
        let gap = |down: f32| {
            let from = ray_from(tile, base, 0.5, down);
            ((hi.z - from.z) / VIEW.z - (hi.x - from.x) / VIEW.x).abs()
        };
        assert!(
            gap(first_side as f32 - 0.5) <= RIM,
            "the last lid row is inside the rim: {}",
            gap(first_side as f32 - 0.5),
        );
        assert!(
            gap(first_side as f32 + 0.5) > RIM,
            "and the first side row is outside it: {}",
            gap(first_side as f32 + 0.5),
        );
    }

    #[test]
    fn a_ray_that_misses_takes_the_nearest_point_of_the_box_and_says_how_far() {
        let (lo, hi) = cube();
        // Two tiles east of the cube's own column: nothing of it is on this
        // line at all.
        let from = ray_from((100, 101), 0.0, 3.0 * TILE_WIDTH, 0.0);
        let met = meets(from, lo, hi);
        assert!(met.outside > 0.0, "this ray misses: {met:?}");
        // Whatever it answers is a point *of the box* — that is what the clamp
        // buys, and it is the property every reader downstream leans on.
        let (at, lo, hi) = (met.at.array(), lo.array(), hi.array());
        for a in 0..3 {
            assert!(
                at[a] >= lo[a] - 1e-5 && at[a] <= hi[a] + 1e-5,
                "axis {a} left the box: {:?}",
                met.at
            );
        }
    }

    #[test]
    fn a_lid_with_no_thickness_is_met_on_its_own_plane() {
        // A floor: `min.z == max.z`, which `crate::solid::Solid`'s own doc says
        // is a legal box and means the plane it lies at.
        let lo = WorldVec::new(100.0, 101.0, 7.0);
        let hi = WorldVec::new(101.0, 102.0, 7.0);
        let met = meets(ray_from((100, 101), 0.0, 0.0, -28.0), lo, hi);
        assert!(met.hit(), "{met:?}");
        assert_eq!(met.normal, WorldVec::new(0.0, 0.0, 1.0));
        assert!((met.at.z - 7.0).abs() < 1e-4, "at: {:?}", met.at);

        // A pixel just inside the south-west rim, where the `z` slab and the `y`
        // slab all but run out together: still a lid.
        let rim = meets(ray_from((100, 101), 0.0, -21.12, -28.0), lo, hi);
        assert!(rim.hit(), "{rim:?}");
        assert_eq!(rim.normal, WorldVec::new(0.0, 0.0, 1.0), "at: {:?}", rim.at);

        // And **on** the corner itself, which is where this test used to stop.
        // It said: the two slabs do not tie at all, their `t`s differ by
        // `3.5e-6` of a tile, so which face is named there is decided by
        // rounding rather than by the tie rule — "that is one pixel of a picture
        // and no claim is made about it".
        //
        // **One pixel of a picture per tile corner** is what that came to. The
        // isometric projection puts a tile's corner on a whole pixel, so the ray
        // that leaves the footprint a rounding before it reaches the plane is
        // drawn once per corner and nowhere else, and it was answered with a
        // *side* face's normal — a wall's cosine in the middle of a roof. A
        // person reported it twice as holes in the roofs, and the second report
        // named the tile: `1492, 1643`. Measured on that frame in `View::Light`:
        // fourteen isolated bright pixels on a lattice of exactly `TILE_WIDTH`,
        // now none, and thirty pixels of the whole frame changed.
        //
        // It is a claim now, and it is not the tie rule doing the work: a lid's
        // side faces have no area, so `meets` does not offer them at all. Both
        // corners of the sprite, since the two halves of the lattice run out on
        // different slabs — the `y` one first and then the `x` one.
        //
        // The `3.5e-6` is gone from the answer as well, and that is the same
        // sentence rather than a second effect: the exit is solved for the plane
        // the lid *is*, so a corner ray lands on it exactly and `outside` reads
        // `0.0` instead of a rounding's worth of miss.
        for (across, which) in [(-22.0, "the west corner"), (22.0, "the east corner")] {
            let corner = meets(ray_from((100, 101), 0.0, across, -28.0), lo, hi);
            assert!(corner.hit(), "{which}: {corner:?}");
            assert_eq!(
                corner.normal,
                WorldVec::new(0.0, 0.0, 1.0),
                "{which}: a lid has no side face to be met on: {corner:?}",
            );
            assert_eq!(corner.at.z, hi.z, "{which}: off the lid's own plane: {corner:?}");
        }
    }

    #[test]
    fn the_box_in_front_wins() {
        // Two cubes on one line of sight, one a tile further along `VIEW` than
        // the other. The near one is the one with the larger `x + y + z`, which
        // is what `crate::depth` sorts on.
        let far = (
            WorldVec::new(100.0, 101.0, 0.0),
            WorldVec::new(101.0, 102.0, 10.0),
        );
        let near = (
            WorldVec::new(101.0, 102.0, 11.0),
            WorldVec::new(102.0, 103.0, 21.0),
        );
        let from = ray_from((100, 101), 0.0, 0.0, 0.0);
        let (which, met) = nearest(from, [("far", far.0, far.1), ("near", near.0, near.1)]).unwrap();
        assert_eq!(which, "near", "{met:?}");
        // And the order it is offered in does not decide it.
        let (which, _) = nearest(from, [("near", near.0, near.1), ("far", far.0, far.1)]).unwrap();
        assert_eq!(which, "near");
    }

    #[test]
    fn a_hit_beats_a_miss_however_near_the_miss_came() {
        let (lo, hi) = cube();
        // A box the ray passes a hair beside, offered first, and the cube it
        // goes through, offered second.
        let beside = (
            WorldVec::new(103.0, 101.0, 0.0),
            WorldVec::new(103.01, 102.0, 10.0),
        );
        let from = ray_from((100, 101), 0.0, 0.0, 0.0);
        let (which, met) = nearest(from, [("beside", beside.0, beside.1), ("cube", lo, hi)]).unwrap();
        assert_eq!(which, "cube", "{met:?}");
        assert!(met.hit(), "{met:?}");
    }

    #[test]
    fn nothing_to_meet_is_answered_with_nothing() {
        let empty: [(u32, WorldVec, WorldVec); 0] = [];
        assert!(nearest(WorldVec::new(100.5, 101.5, 0.0), empty).is_none());
    }

    #[test]
    fn a_tread_and_its_neighbour_s_riser_meet_with_no_pixel_between_them() {
        // The whole of what the phase is for, one seam of it: a step's top and
        // the rise in front of it are two primitives sharing one line, and every
        // pixel of a sweep across that line is a pixel *of* one of them. A hole
        // here — a fragment answered by the nearest-point fallback — is what
        // `WIDTH_OVERLAP` grew a mesh to cover; there is nothing to grow when the
        // two shapes are met by one ray.
        //
        // A **north**-climbing flight of two treads on tile `(100, 101)`, three
        // and six `z` tall, exactly as `occlusion::Builder::add` pushes it: a
        // top and a riser a tread, in climb order. North because the climb then
        // runs *away* from the camera, which is the case with a seam in it —
        // see the sibling test for the other direction, where the flight turns
        // its own back and there is no vertical surface at all.
        let lid_low = (WorldVec::new(100.0, 101.5, 3.0), WorldVec::new(101.0, 102.0, 3.0));
        let riser_low = (WorldVec::new(100.0, 102.0, 0.0), WorldVec::new(101.0, 102.0, 3.0));
        let lid_high = (WorldVec::new(100.0, 101.0, 6.0), WorldVec::new(101.0, 101.5, 6.0));
        let riser_high = (WorldVec::new(100.0, 101.5, 3.0), WorldVec::new(101.0, 101.5, 6.0));

        let mut surfaces = Vec::new();
        // A quarter of a pixel a step, down the sprite's own middle column,
        // over the whole of what the four solids cover.
        for step in -184i16..=88 {
            let down = f32::from(step) / 4.0;
            let from = ray_from((100, 101), 0.0, 0.0, down);
            let (which, met) = nearest(
                from,
                [
                    ("lid_low", lid_low.0, lid_low.1),
                    ("riser_low", riser_low.0, riser_low.1),
                    ("lid_high", lid_high.0, lid_high.1),
                    ("riser_high", riser_high.0, riser_high.1),
                ],
            )
            .unwrap();
            assert!(met.hit(), "a pixel of no surface at down {down}: {met:?}");
            if surfaces.last() != Some(&which) {
                surfaces.push(which);
            }
        }
        // Down the screen: the upper tread, the rise below its lip, the lower
        // tread, the rise below *its* lip. Each once — a surface coming back a
        // second time would be two of them disagreeing about where the seam is,
        // which is the defect a grown mesh hides rather than fixes.
        assert_eq!(surfaces, ["lid_high", "riser_high", "lid_low", "riser_low"]);
    }

    /// **Every pixel a block's own picture draws meets that block's own box** —
    /// the claim [`FRAGMENT`] exists for, and the one this grid had never been
    /// held to.
    ///
    /// The picture and the box are the *same* block, stated once and read two
    /// ways: [`crate::facing::blocks_silhouette`] draws what the art of a
    /// whole-tile body would be, and the box is that body's own volume. So there
    /// is nothing here for a footprint to be wrong about and nothing for an
    /// artist to overhang — a miss can only be the two grids disagreeing, and the
    /// only disagreement available is the sample.
    ///
    /// It measured **forty-four misses at either height** before the tolerance
    /// became one fragment: one row of the tile's own width, `1 / TILE_WIDTH` of
    /// a tile out — and on a real frame they were a dashed line along every tile
    /// seam, drawn with no facing and therefore lit from every side. The glowing
    /// grid over a lit floor that a person reported was those pixels.
    ///
    /// **Three controls rather than one assertion**, because "no misses" is what
    /// a tolerance of infinity also reports:
    ///
    /// - the same picture against a box a hundred tiles away misses *everything*,
    ///   so the walk is reading the boxes at all;
    /// - the worst surviving miss is over half a fragment, so the constant is
    ///   load-bearing at its own size — halving it puts the seam back;
    /// - and two heights, since a floor that grows with the box would say the two
    ///   disagree about the projection rather than about the grid.
    ///
    /// `examples/discard_census`'s own control is this measurement over the
    /// client's real art; this is it over art nobody had to ship.
    #[test]
    fn every_pixel_of_a_blocks_picture_meets_that_blocks_own_box() {
        use crate::facing::{
            Block,
            Blocks,
            Span,
            blocks_silhouette,
        };

        for top in [5u8, 10] {
            let block = Block::new(Span::new(0, 8), Span::new(0, 8), Span::new(0, top))
                .expect("a whole tile with a height");
            let image = blocks_silhouette(&Blocks::new(&[block]).expect("one block"));
            let (width, height) = (image.width(), image.height());
            let middle = f32::from(width) / 2.0;
            // The tile's own centre row: a sprite's bottom edge stands on the
            // diamond's bottom vertex, half a tile below it. `statics::stand_on`,
            // and the same convention `discard_census` reads a picture in.
            let centre_row = f32::from(height) - (crate::camera::TILE_HEIGHT / 2) as f32;
            let (lo, hi) = (
                WorldVec::new(0.0, 0.0, 0.0),
                WorldVec::new(1.0, 1.0, f32::from(top)),
            );
            let far = (
                WorldVec::new(100.0, 100.0, 0.0),
                WorldVec::new(101.0, 101.0, f32::from(top)),
            );

            let (mut drawn, mut missed, mut missed_far, mut worst) = (0u32, 0u32, 0u32, 0.0f32);
            for row in 0..height {
                for column in 0..width {
                    let opaque = image
                        .pixel(column, row)
                        .is_some_and(|pixel| !pixel.is_transparent());
                    if !opaque {
                        continue;
                    }
                    drawn += 1;
                    let across = f32::from(column) + 0.5 - middle;
                    let down = f32::from(row) + 0.5 - centre_row;
                    let from = ray_from((0, 0), 0.0, across, down);
                    let met = meets(from, lo, hi);
                    if met.hit() {
                        worst = worst.max(met.outside);
                    } else {
                        missed += 1;
                    }
                    missed_far += u32::from(!meets(from, far.0, far.1).hit());
                }
            }

            assert!(drawn > 1000, "{top} z tall drew only {drawn} pixels");
            assert_eq!(
                missed, 0,
                "{missed} of {drawn} pixels of a block's own picture missed that block's own box \
                 at {top} z tall — the tile-seam line FRAGMENT was measured to take",
            );
            assert_eq!(
                missed_far, drawn,
                "the same picture met a box a hundred tiles away, so this walk is not reading \
                 boxes at all and the zero above says nothing",
            );
            assert!(
                worst > FRAGMENT / 2.0,
                "the worst surviving miss is {worst} of a tile, under half of FRAGMENT ({FRAGMENT}) \
                 — either the sample grid stopped producing this or the constant is now twice the \
                 size it has to be",
            );
        }
    }
}
