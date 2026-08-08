//! Which tile a pixel belongs to.
//!
//! The world passes draw pictures; this is the second thing they write while
//! they do it — for every pixel, the tile and height of *the thing drawn there*.
//! [`crate::blit`] reads it and lights the frame in world coordinates, which is
//! the whole of `docs/lighting.md`'s first decision.
//!
//! # Why the picture alone is not enough
//!
//! The screen folds height into `y`: a brazier in a cellar and a lantern on the
//! street above it land a few pixels apart. Worse, a wall's sprite *stands* on
//! the tile it occludes from — 44 pixels of picture rising out of a diamond at
//! the floor — so anything decided from a pixel's screen position alone puts the
//! face of the wall nearest the flame into the shadow the wall itself casts.
//! There is no arrangement of screen-space masks that separates those; the tile
//! the pixel came from does it exactly.
//!
//! # The format
//!
//! `Rgba16Uint`, as `(id, height and stance, kind and the fraction)` — the
//! first channel is spare rather than a fifth field. Integers because these are
//! an id and a `z`, and a `u16` holds either exactly: a coordinate on the
//! largest facet a client ships (7,168 across), or `docs/gbuffer.md` decision
//! 2's id, which nothing this client has ever drawn a frame of has come near
//! filling. `Rgba16Uint` is colour-renderable in WebGL2, which is the ceiling
//! this crate draws under — see the crate docs.
//!
//! The height is not one of those integers, and that is `docs/lighting_height.md`
//! phase 1: the third channel is `z`'s whole units offset by 128, then
//! [`Z_FRAC_SHIFT`]'s four bits of sixteenths, then [`STANCE_SHIFT`]'s stance.
//! A tile's height *is* an integer, so nothing on the ground or on a lid
//! noticed; a vertical face's is not, and rounding it made a staircase of a
//! wall. [`packed_height`] and [`unpacked_height`] are the two ends of it here.
//!
//! **The tile itself does not ride here for any kind.** `docs/gbuffer.md`
//! step 3 moved a static's and a mobile's `x`/`y` to their own pass's instance
//! buffer, addressed by the id these two channels hold instead; step 7 did
//! the same for the ground. What still lives in [`Place`] and
//! [`Place::packed`] below is the *row*'s own shape — the two words a
//! [`SpriteQuad`](crate::sprite::SpriteQuad) or a
//! [`GroundQuad`](crate::ground::GroundQuad) carries on the GPU, which is
//! where a tile is still a literal `x`/`y` rather than an id into anything.
//!
//! The fourth channel carries **the kind in its low two bits, then seven bits of
//! tile-local `x` and seven of tile-local `y`** — where in its tile the pixel
//! is, to a hundred-and-twenty-eighth of one. That fraction is not decoration:
//! without it every pixel of a tile is the same distance from every flame, and a
//! pool of light comes out as flat 44-pixel tiles with a step at each edge
//! rather than as a gradient. It is written by the shaders and read by
//! `blit.wgsl`; the packing appears in three files and only a person reading all
//! three can check that they agree.
//!
//! A sprite's fraction depends on which way its picture faces, which is
//! [`Stance`]: a floor lies in its tile and every pixel of it is somewhere
//! different in the world, while a wall stands on one and its picture is height.
//! Getting this wrong is not subtle — a floor whose pixels all claim the middle
//! of their tile is lit as one flat value with a step at every seam, which is
//! what a room's floor looked like before this existed.
//!
//! A fragment a sprite discarded writes nothing here either, so what this holds
//! is what is *visible*, which is the question lighting asks.

/// What kind of thing wrote a pixel, or [`Place::NOWHERE`]'s zero for "nothing
/// did".
///
/// The kinds are distinct rather than a single "something is here" bit because
/// they cost nothing — the channel is 16 bits wide and holds a 2 — and the
/// question "is this pixel a mobile" is one a later pass (an outline, a
/// selection) asks without wanting a second attachment for it.
///
/// `wgsl` has these as constants in `blit.wgsl`, and the two must agree; there
/// is a test below that states each value, which is the only thing that can be
/// compared against text a Rust compiler never reads.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Kind {
    /// Nothing was drawn here — the cleared background, or a pass that draws
    /// something which is not part of the world at all.
    ///
    /// **Such a pixel is not lit and not dimmed**: the blit passes it through
    /// exactly as the world pass left it. The two things with no place in the
    /// world are the background, which is black either way, and the text drawn
    /// over a speaker's head, which is a message rather than a thing standing in
    /// the street — night must not make it unreadable.
    Nothing = 0,
    /// A land tile.
    Land = 1,
    /// A static, or an item the server put on the ground.
    Static = 2,
    /// A mobile, or something it is wearing.
    Mobile = 3,
}

/// Which way a sprite's picture faces, and therefore what its pixels mean.
///
/// A sprite is a rectangle of art standing on a tile, and what that rectangle is
/// a picture *of* decides how a pixel of it maps back into the world:
///
/// - [`Stance::Flat`] — a floor, a rug, a road: `TileFlags::FLOOR`, the bit
///   ClassicUO calls `Background`. Its picture *is* the tile's diamond, so both
///   fractions come out of where in that diamond the pixel is, and the height is
///   the tile's own everywhere.
/// - The four **faces** — a wall, told apart by which edge of its tile it stands
///   on. Its picture is a billboard rising from one 22-pixel edge of the
///   diamond, so a pixel's horizontal position is how far *along that edge* it
///   is and what runs down the picture is height. Which edge is measured from the
///   art: [`crate::facing::facing_of`], because nothing in `tiledata.mul` records
///   it. This is what makes a row of wall tiles one continuous surface instead of
///   a row of separately lit sprites — see [`crate::facing::Face::place_at`].
/// - The four **corners** — a picture that is two of those faces at once, which
///   is what a building's corner piece is. Which of the two a pixel belongs to
///   is which half of the tile's column it is drawn on, so a corner is resolved
///   to one of the four faces **per fragment**, in `statics.wgsl`, and what
///   reaches the attachment is always a single face. Nothing downstream of the
///   world passes has ever to know a corner exists: see
///   [`crate::facing::Facing::on_half`].
/// - [`Stance::Upright`] — everything else that stands up: a tree, a body, a
///   post, a wall whose art the detector could not read. It stands on the tile
///   and what varies down its picture is height, but across it nothing varies:
///   the fraction is the tile's middle everywhere. That is a statement about
///   what is *not* known rather than a shortcut — reading the horizontal offset
///   as `x - y`, as this did for one commit, spreads the pixels along the one
///   direction no wall ever runs, and it looks like it.
///
/// It rides in **four bits** of the instance's second word, which is what the
/// eleven values need and what [`Place::packed`] reserves — and it reaches the
/// attachment too, in the top four bits of the third channel's `u16`, above the
/// height and the height's fraction. See [`STANCE_SHIFT`]. Four where six
/// values needed three, and neither word had to grow: both have eight or more
/// bits spare above it.
///
/// **Why the lighting needs it, when the fraction is already there.** A fraction
/// says where in its tile a pixel is; a face says **which way that surface
/// looks**. The two are not the same fact and the second cannot be recovered
/// from the first: a wall's two faces are one tile and one plane, so a pixel of
/// the street side and a pixel of the room side of the same wall carry the same
/// tile, the same fraction and the same height. Without the stance a torch in a
/// room lights the outside of the house exactly as brightly as the inside, which
/// is what it did, and reads as a wall made of glass.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Stance {
    /// Standing on its tile with nothing known about which way it runs: a tree,
    /// a body, a corner, a wall the art did not name an edge for.
    Upright = 0,
    /// Lying in it: a floor, a rug, a road.
    Flat = 1,
    /// A wall on the tile's `y0` edge, running along `+x`.
    FaceNorth = 2,
    /// A wall on the tile's `x1` edge, running along `+y`.
    FaceEast = 3,
    /// A wall on the tile's `y1` edge, running along `+x`.
    FaceSouth = 4,
    /// A wall on the tile's `x0` edge, running along `+y`.
    FaceWest = 5,
    /// A corner: the north face on the right half of the picture, the south face
    /// on the left.
    ///
    /// The four corner values are laid out so the two faces come out of the
    /// number by arithmetic rather than by a table, because `statics.wgsl` has to
    /// do it per fragment: `right = FaceNorth + ((v - CORNER) >> 1)` and
    /// `left = FaceSouth + ((v - CORNER) & 1)`. Right is always `North` or
    /// `East` and left always `South` or `West` — the halves of the tile's
    /// column, which is the invariant [`crate::facing::Facing::Corner`] carries.
    CornerNorthSouth = 6,
    /// The north face on the right half, the west face on the left.
    CornerNorthWest = 7,
    /// The east face on the right half, the south face on the left. The corner
    /// the client's own art draws: those are the two faces a camera can see, so
    /// this is nearly every corner of nearly every building.
    CornerEastSouth = 8,
    /// The east face on the right half, the west face on the left.
    CornerEastWest = 9,
    /// Not a real stance: a routing sentinel `docs/gbuffer.md` step 4c's mesh
    /// pass writes instead of one of the five above.
    ///
    /// `blit.wgsl` reads `face_instances[id]` for every other `Kind::Static`
    /// pixel; seeing this one in the attachment's stance bits tells it to read
    /// `mesh_instances[id]` instead — a different, smaller row (tile and the
    /// face's *real* stance, one of [`Stance::Flat`]/the four
    /// [`Stance::FaceNorth`]-family values), because a mesh face has no
    /// picture of its own to share `face_instances`' shape with. Never
    /// returned by [`Stance::of`], never written into a
    /// [`SpriteQuad`](crate::sprite::SpriteQuad) — see
    /// [`Stance::of_normal`], which produces the row's real stance instead.
    MeshFace = 10,
}

/// The first of the four corner stances. `statics.wgsl` has the same number, and
/// the arithmetic that takes a corner apart is stated on
/// [`Stance::CornerNorthSouth`].
pub const STANCE_CORNER: u8 = Stance::CornerNorthSouth as u8;

impl Stance {
    /// Which way a static's picture faces: the client's own bit first, then what
    /// the art said.
    ///
    /// The order is the policy and it is here rather than at the two call sites,
    /// so that a floor cannot be given a face by a detector that never should
    /// have been shown it. `TileFlags::FLOOR` — `UFLAG1_FLOOR` in Sphere,
    /// `Background` in ClassicUO — is set on floors, rugs, roads and cave floors
    /// and on nothing that stands up: a table is `BLOCK | PLATFORM` and carries
    /// it not at all, which is the pair worth checking, because "you can stand on
    /// it" is a different question and `PLATFORM` is how it is asked.
    ///
    /// `facing` is [`crate::atlas::Sprite::facing`], measured once when the
    /// picture was packed. `None` — a post, a tree, a graphic the client ships no
    /// readable wall for — falls back to [`Stance::Upright`], which is exactly
    /// what every static did before faces existed. Nothing gets worse anywhere.
    pub fn of(tile: &openshard_uofiles::tiledata::StaticTile, facing: Option<crate::facing::Facing>) -> Self {
        use crate::facing::{Face, Facing};

        if tile.flags.is_background() {
            return Self::Flat;
        }
        match facing {
            Some(Facing::One(face)) => Self::face(face),
            // The pairing is the one `Facing::Corner` guarantees — a right-half
            // face and a left-half one — so the sixteen combinations the types
            // allow are the four the detector can produce, and anything else is
            // a facing built by hand and not by measurement. It falls back to
            // the whole-tile answer rather than picking a half.
            Some(Facing::Corner { right, left }) => match (right, left) {
                (Face::North, Face::South) => Self::CornerNorthSouth,
                (Face::North, Face::West) => Self::CornerNorthWest,
                (Face::East, Face::South) => Self::CornerEastSouth,
                (Face::East, Face::West) => Self::CornerEastWest,
                _ => Self::Upright,
            },
            None => Self::Upright,
        }
    }

    /// The stance of a single face.
    pub fn face(face: crate::facing::Face) -> Self {
        match face {
            crate::facing::Face::North => Self::FaceNorth,
            crate::facing::Face::East => Self::FaceEast,
            crate::facing::Face::South => Self::FaceSouth,
            crate::facing::Face::West => Self::FaceWest,
        }
    }

    /// Which real stance an honest [`crate::mesh::Face`] normal names, or
    /// `None` for a vector nothing here produces.
    ///
    /// [`crate::facing::Prism::mesh`] builds every one of its faces' normals
    /// from exactly two shapes — `[0, 0, 1]` for a top, or
    /// [`crate::facing::Face::outward`] folded into three dimensions for a
    /// riser — so this is a closed set today, not a general vector decoder:
    /// `docs/gbuffer.md`'s "Not settled" section leaves the general case
    /// (a packed arbitrary normal) open for whenever a producer that is not
    /// axis-aligned exists to measure a bit layout against. This is that
    /// question's answer for the five that do exist, reusing
    /// `blit.wgsl`'s existing `outward(stance)` rather than inventing a
    /// second encoding no consumer needs yet.
    pub fn of_normal(normal: [f32; 3]) -> Option<Self> {
        use crate::facing::Face;

        if normal == [0.0, 0.0, 1.0] {
            return Some(Self::Flat);
        }
        [Face::North, Face::East, Face::South, Face::West]
            .into_iter()
            .find(|face| {
                let [ox, oy] = face.outward();
                normal == [ox, oy, 0.0]
            })
            .map(Self::face)
    }
}

/// Where in the world a pixel's picture came from.
///
/// Not a [`Point`](openshard_protocol::world::Point): the `kind` is half of what
/// the attachment carries, and a `Point` with a kind beside it in every quad
/// struct is the same thing said in two fields that can disagree.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Place {
    /// The tile's `x`.
    pub x: u16,
    /// The tile's `y`.
    pub y: u16,
    /// The height it was drawn at.
    ///
    /// The ground pass ignores this and writes the height its corner
    /// interpolation gives the pixel — a hillside's pixels each carry their own
    /// — so for a [`GroundQuad`](crate::ground::GroundQuad) it is the tile's
    /// base and nothing reads it.
    pub z: i8,
    /// What drew it.
    pub kind: Kind,
    /// Which way its picture faces. The ground pass ignores this — a land tile
    /// is a diamond by construction and its shader has always read the position
    /// inside it.
    pub stance: Stance,
}

impl Place {
    /// No place at all: the clear value, and what a pass that draws something
    /// outside the world writes.
    pub const NOWHERE: Self = Self {
        x: 0,
        y: 0,
        z: 0,
        kind: Kind::Nothing,
        stance: Stance::Upright,
    };

    /// A pixel of the land on a tile. See [`Place::z`] for why the height is not
    /// an argument.
    pub fn land(x: u16, y: u16) -> Self {
        Self {
            x,
            y,
            z: 0,
            kind: Kind::Land,
            stance: Stance::Flat,
        }
    }

    /// A pixel of a static or a ground item standing on `at` — a wall, a barrel,
    /// a tree. See [`Place::of_floor`] for the ones that lie in their tile
    /// instead.
    pub fn of_static(at: openshard_protocol::world::Point) -> Self {
        Self {
            x: at.x,
            y: at.y,
            z: at.z,
            kind: Kind::Static,
            stance: Stance::Upright,
        }
    }

    /// A pixel of a static lying flat in its tile: a floor, a rug, a road.
    ///
    /// The same kind as [`Place::of_static`] — what it is standing on is not
    /// what a later pass asks about — and a different [`Stance`], which is the
    /// whole of the difference: this one's picture is the tile's diamond and its
    /// pixels are spread across the tile rather than up it.
    pub fn of_floor(at: openshard_protocol::world::Point) -> Self {
        Self {
            stance: Stance::Flat,
            ..Self::of_static(at)
        }
    }

    /// A pixel of a mobile, or of what it is wearing, standing at `at`.
    pub fn of_mobile(at: openshard_protocol::world::Point) -> Self {
        Self {
            x: at.x,
            y: at.y,
            z: at.z,
            kind: Kind::Mobile,
            stance: Stance::Upright,
        }
    }

    /// The two words an instance buffer carries this in.
    ///
    /// Packed rather than four fields because a vertex attribute is fetched in
    /// four-byte words either way, and two `u32`s is the smallest this fits in:
    /// `(x | y << 16, (z + 128) | kind << 8 | stance << 16)`. The shader takes it
    /// apart with the same shifts, which are written out there rather than
    /// shared — there is nothing in Rust for a WGSL function to call.
    ///
    /// The stance rides above the kind in **four bits**, and is *masked off*
    /// again where the world pass writes the attachment's fourth channel: that
    /// channel is two bits of kind and fourteen of fraction with nothing spare,
    /// and the stance's job is finished by the time the fraction has been
    /// computed. Four bits because there are ten stances — one flat, one
    /// unknown, four wall faces and four corners — and the word has room above
    /// them.
    pub fn packed(self) -> [u32; 2] {
        [
            u32::from(self.x) | u32::from(self.y) << 16,
            (i32::from(self.z) + 128) as u32 | (self.kind as u32) << 8 | (self.stance as u32) << 16,
        ]
    }
}

/// Where a [`Stance`] rides in the attachment's third channel, above the height
/// and above its fraction.
///
/// The channel is a `u16`. Eight bits hold `z`'s whole units offset by 128,
/// four hold [`Z_FRAC_SHIFT`]'s sixteenths of a unit, and the four above those
/// carry the stance — so a fragment can ask which way the surface it is looking
/// at faces without a second attachment or a wider format.
///
/// What arrives here is never a corner: `statics.wgsl` resolves a corner to the
/// face of the half the fragment is on before it writes this, so the reader —
/// `blit.wgsl`'s `outward` — sees one surface with one normal and has no case
/// for two.
///
/// `place_format.wesl`'s `PLACE_STANCE_SHIFT` is the other place this number
/// appears, and nothing but a person can compare the two. [`packed_height`]
/// below is what everything on this side goes through, and a test round-trips
/// it.
pub const STANCE_SHIFT: u32 = 12;

/// Where the height's fraction rides in that channel, between the whole units
/// and the stance: sixteenths of a `z` unit, four bits of them.
///
/// `docs/lighting_height.md` phase 1. Before it the channel held `round(z)`
/// alone — exact on a floor or a lid, because a lid *is* at an integer `z`, and
/// a lie on anything standing up: height varies continuously down a vertical
/// face, and rounding it to the nearest unit turns one surface into a staircase
/// of one-unit treads, each lit as though it were a whole unit higher or lower
/// than it really is.
///
/// Sixteenths because a `z` unit is four screen pixels at zoom 1, so one step
/// is a quarter of a pixel — under anything a frame can show — and four bits
/// were already spare here. `place_format.wesl`'s `PLACE_Z_FRAC_SHIFT`.
pub const Z_FRAC_SHIFT: u32 = 8;

/// The fraction's mask: `place_format.wesl`'s `PLACE_Z_FRAC_MASK`.
pub const Z_FRAC_MASK: u16 = 15;

/// Steps of that fraction in one `z` unit — [`Z_FRAC_MASK`] plus one, named
/// apart from it because every use is arithmetic on an `f32` rather than a
/// mask. `place_format.wesl`'s `PLACE_Z_FRAC_STEPS`.
pub const Z_FRAC_STEPS: f32 = 16.0;

/// The attachment's third channel, packed: a continuous height and a stance.
///
/// The Rust twin of `place_format.wesl`'s `pack_place`, for the two things on
/// this side that write the attachment without a world pass — [`crate::plan`]'s
/// diagnostic pictures, and the tests that upload a frame of places to compare
/// the blit against [`crate::light::sample`]. Both used to spell the packing
/// out by hand, which is a copy of a format that has now moved once; a copy
/// that stays behind does not fail to compile, it just draws a wall in
/// one-unit treads again.
///
/// The height is quantised once, in sixteenths, and only then split — the same
/// arithmetic and the same reason as the shader's: a remainder rounded up
/// separately can carry a full unit into the field above, and the field above
/// is the stance, not a higher digit of anything.
pub fn packed_height(z: f32, stance: Stance) -> u16 {
    // The range the two fields can express together: `-128 ..= 127 + 15/16`.
    // A height outside it is pinned to the nearer end, which is what the
    // channel has always done with a `z` no map holds.
    let steps = (z * Z_FRAC_STEPS)
        .round()
        .clamp(-128.0 * Z_FRAC_STEPS, 128.0 * Z_FRAC_STEPS - 1.0) as i32;
    // `div_euclid`/`rem_euclid` rather than `/` and `%`: this is a `floor` into
    // the whole units with a remainder that stays positive, so that -3.5 packs
    // as -4 and eight sixteenths — the way [`unpacked_height`] adds them back —
    // and not as -3 and minus eight.
    let whole = steps.div_euclid(i32::from(Z_FRAC_MASK) + 1);
    let frac = steps.rem_euclid(i32::from(Z_FRAC_MASK) + 1);
    (whole + 128) as u16 | (frac as u16) << Z_FRAC_SHIFT | (stance as u16) << STANCE_SHIFT
}

/// The height that channel holds, whole units and fraction together: the twin
/// of `place_format.wesl`'s `unpack_place_z`, and what a test that reads a
/// rendered attachment back should decode a height with.
///
/// Not a `& 0xFF`: reading only the whole units still compiles and still looks
/// like a height, and quietly puts a vertical face's fragment back on the
/// staircase the fraction exists to remove.
pub fn unpacked_height(channel: u16) -> f32 {
    f32::from(channel & 0xFF) - 128.0 + f32::from((channel >> Z_FRAC_SHIFT) & Z_FRAC_MASK) / Z_FRAC_STEPS
}

/// The format of the attachment. See this module's header.
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Uint;

/// Create the attachment at a render target's size.
///
/// Here rather than in the caller for the reason
/// [`depth_texture`](crate::renderer::depth_texture) is: the format is this
/// crate's decision, and a texture created with another one fails at
/// pipeline-bind time with an error that names neither side.
pub fn texture(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("place"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            // So a frame test can read it back and assert that a wall's pixel
            // names the wall's tile, which is the only way to know the channel
            // is right rather than merely present.
            | wgpu::TextureUsages::COPY_SRC
            // And so a test can *write* one: the parity test hands the blit a
            // frame whose every pixel names a tile it chose, and compares what
            // comes out with `light::sample`. Uploading the attachment is what
            // lets that test exist without a client install and without art —
            // it is about two implementations of one formula, and a rendered
            // sprite would only be a way of producing places more slowly. See
            // `docs/lighting.md`, decision 9.
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

/// What an untouched pixel of the attachment is left as: [`Kind::Nothing`]
/// everywhere.
pub const CLEAR: wgpu::Color = wgpu::Color {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 0.0,
};

#[cfg(test)]
mod tests {
    use openshard_protocol::world::Point;

    use super::*;

    /// The packing, stated in numbers. It is a contract with two shaders that no
    /// compiler checks, so the bits are pinned here.
    #[test]
    fn a_place_packs_into_two_words() {
        let packed = Place::of_static(Point::new(0x1234, 0x5678, -3)).packed();
        assert_eq!(packed[0], 0x5678_1234, "y in the high half, x in the low");
        assert_eq!(packed[1], (125) | (2 << 8), "z offset by 128, then the kind");
        // And the stance above both, which is the bit `statics.wgsl` branches on
        // to decide whether a pixel's picture is spread across its tile or up it.
        let floor = Place::of_floor(Point::new(0x1234, 0x5678, -3)).packed();
        assert_eq!(
            floor[1],
            125 | (2 << 8) | (1 << 16),
            "the flat bit is the seventeenth"
        );
        assert_eq!(floor[0], packed[0], "and nothing else moved");
    }

    /// A cleared texel and a [`Place::NOWHERE`] quad say the same thing, and it
    /// is the *kind* that says it.
    ///
    /// The clear is all zeros, which decodes to a `z` of -128; `NOWHERE` writes
    /// a `z` of 0. They differ in that channel and must not differ in the one
    /// that is read: [`Kind::Nothing`] is zero on both sides, and a reader that
    /// looked at the height of a pixel nothing drew would be reading a number
    /// nobody wrote.
    #[test]
    fn nothing_drawn_and_nothing_cleared_are_one_kind() {
        assert_eq!(Kind::Nothing as u32, 0, "the clear value's kind");
        assert_eq!(Place::NOWHERE.packed()[0], 0);
        assert_eq!(Place::NOWHERE.packed()[1] >> 8, 0, "and nothing else is read");
        // The shaders write the kind into the low bits of that channel and the
        // sub-tile fraction above it, so "nothing here" has to be a value the
        // fraction cannot reach into: it is the *kind* that is zero, and the
        // kinds occupy two bits.
        assert!((Kind::Mobile as u32) < 4, "a kind no longer fits its two bits");
    }

    /// A corner's number is the two faces it is made of, by arithmetic.
    ///
    /// `statics.wgsl` takes a corner apart per fragment with two shifts rather
    /// than with a switch, so the layout of these four values is a contract with
    /// it: `right` is `FaceNorth` plus the high bit of the offset from
    /// [`STANCE_CORNER`], `left` is `FaceSouth` plus the low one. Stated here
    /// because a value reordered in the enum would compile, draw, and shade every
    /// corner in the world along the wrong axis.
    #[test]
    fn a_corner_s_number_holds_both_of_its_faces() {
        for (stance, right, left) in [
            (Stance::CornerNorthSouth, Stance::FaceNorth, Stance::FaceSouth),
            (Stance::CornerNorthWest, Stance::FaceNorth, Stance::FaceWest),
            (Stance::CornerEastSouth, Stance::FaceEast, Stance::FaceSouth),
            (Stance::CornerEastWest, Stance::FaceEast, Stance::FaceWest),
        ] {
            let offset = stance as u8 - STANCE_CORNER;
            assert_eq!(Stance::FaceNorth as u8 + (offset >> 1), right as u8, "{stance:?}");
            assert_eq!(Stance::FaceSouth as u8 + (offset & 1), left as u8, "{stance:?}");
        }
        // And all ten fit the four bits both words reserve for them.
        assert!((Stance::CornerEastWest as u32) < 16);
    }

    /// A corner is what the detector said and not a half of it.
    ///
    /// The pairing is the invariant `Facing::Corner` carries; a facing built by
    /// hand with two right-half faces is not a picture anything measured, and
    /// what it must not do is pick one of them for the whole tile.
    #[test]
    fn a_corner_facing_becomes_a_corner_stance() {
        use crate::facing::{Face, Facing};

        let wall = openshard_uofiles::tiledata::StaticTile::default();
        assert_eq!(
            Stance::of(
                &wall,
                Some(Facing::Corner {
                    right: Face::East,
                    left: Face::South
                })
            ),
            Stance::CornerEastSouth,
        );
        assert_eq!(
            Stance::of(
                &wall,
                Some(Facing::Corner {
                    right: Face::South,
                    left: Face::North
                })
            ),
            Stance::Upright,
            "a pairing no half can produce falls back to the whole tile",
        );
    }

    /// The five normals [`crate::facing::Prism::mesh`] can produce all round-trip
    /// back to the real stance that names them, and nothing else does.
    ///
    /// Pinned because the two sides of this mapping — the literals `Prism::mesh`
    /// builds its faces' normals from, and this function's own match — live in
    /// different files with nothing but a comment tying them together; a literal
    /// changed in either one should fail a test rather than silently start
    /// returning `Upright` for a mesh face it used to answer honestly.
    #[test]
    fn of_normal_recovers_every_stance_prism_mesh_can_produce() {
        use crate::facing::Face;

        assert_eq!(Stance::of_normal([0.0, 0.0, 1.0]), Some(Stance::Flat));
        for face in [Face::North, Face::East, Face::South, Face::West] {
            let [ox, oy] = face.outward();
            assert_eq!(
                Stance::of_normal([ox, oy, 0.0]),
                Some(Stance::face(face)),
                "{face:?}'s own outward normal"
            );
        }
        assert_eq!(
            Stance::of_normal([1.0, 1.0, 0.0]),
            None,
            "not a unit vector any face or the flat top produces"
        );
    }

    /// The kinds, one number each. `blit.wgsl` holds the same four and cannot be
    /// checked against this by anything but a person reading both.
    #[test]
    fn the_kinds_are_the_numbers_the_shader_has() {
        assert_eq!(Kind::Land as u32, 1);
        assert_eq!(Kind::Static as u32, 2);
        assert_eq!(Kind::Mobile as u32, 3);
    }

    /// A height packs and unpacks to within half a step of the fraction, and a
    /// stance rides beside it untouched.
    ///
    /// The contract `docs/lighting_height.md` phase 1 rests on, and the one
    /// thing that can be checked on this side: the shader's `pack_place` and
    /// `unpack_place_z` are the same arithmetic in a file no Rust compiler
    /// reads. What this pins is that the round trip is *continuous* — a
    /// quarter-unit step in `z` survives it, which is exactly what rounding to
    /// whole units destroyed — and that the fraction never leaks upwards into
    /// the stance's bits.
    #[test]
    fn a_height_round_trips_through_its_fraction() {
        // A quarter of a unit apart is four steps of the fraction: under
        // rounding these three were all one height, which is the staircase.
        for (z, expect) in [(13.25, 13.25), (13.5, 13.5), (13.75, 13.75)] {
            let packed = packed_height(z, Stance::FaceSouth);
            assert_eq!(unpacked_height(packed), expect, "{z} did not survive packing");
            assert_eq!(
                packed >> STANCE_SHIFT,
                Stance::FaceSouth as u16,
                "{z}'s fraction reached into the stance",
            );
        }
        // Anything between two steps lands on the nearer, never further than
        // half a step away — a thirty-second of a unit, an eighth of a screen
        // pixel at zoom 1.
        for z in [-40.03, -0.7, 0.0, 7.31, 63.99] {
            let error: f32 = unpacked_height(packed_height(z, Stance::Flat)) - z;
            assert!(error.abs() <= 0.5 / Z_FRAC_STEPS, "{z} came back {error} out");
        }
        // A negative height floors into the whole units with the remainder
        // still positive, the way the shader's arithmetic shift does it —
        // -3.5 is -4 and eight sixteenths, not -3 and minus eight.
        let packed = packed_height(-3.5, Stance::Upright);
        assert_eq!(
            packed & 0xFF,
            (-4i32 + 128) as u16,
            "the whole units did not floor"
        );
        assert_eq!(
            (packed >> Z_FRAC_SHIFT) & Z_FRAC_MASK,
            8,
            "and the remainder went negative"
        );
    }

    /// The ends of the range are the ends of what the *two* fields hold, and a
    /// height past either one is pinned there rather than wrapping.
    ///
    /// The clamp is the same one the channel always had; what moved is that its
    /// top end is now `127 + 15/16` rather than `127`. A wrap here would put a
    /// tower's roof under the ground, which is the failure worth a test.
    #[test]
    fn a_height_outside_the_range_is_pinned_to_its_end() {
        assert_eq!(unpacked_height(packed_height(-900.0, Stance::Flat)), -128.0);
        assert_eq!(
            unpacked_height(packed_height(900.0, Stance::Flat)),
            127.0 + 15.0 / Z_FRAC_STEPS,
        );
        // And the stance is undisturbed at both ends: a clamp that overflowed
        // its field would show up here first.
        for z in [-900.0, 900.0] {
            assert_eq!(
                packed_height(z, Stance::FaceEast) >> STANCE_SHIFT,
                Stance::FaceEast as u16,
            );
        }
    }

    /// The lowest and highest `z` a map holds both survive the offset, which is
    /// the whole reason there is one.
    #[test]
    fn the_ends_of_the_z_range_survive_the_offset() {
        assert_eq!(Place::of_static(Point::new(1, 1, -128)).packed()[1] & 0xFF, 0);
        assert_eq!(Place::of_static(Point::new(1, 1, 127)).packed()[1] & 0xFF, 255);
    }
}
