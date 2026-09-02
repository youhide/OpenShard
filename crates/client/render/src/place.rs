//! Which tile a pixel belongs to.
//!
//! The world passes draw pictures; this is the second thing they write while
//! they do it — for every pixel, the tile and height of *the thing drawn there*.
//! [`crate::blit`] reads it and lights the frame in world coordinates, which is
//! the whole of `docs/archive/render/lighting.md`'s first decision.
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
//! # What is left of it, and what took the rest
//!
//! This module was the *format of an attachment* — `Rgba16Uint`, four channels
//! holding an id, a height in whole units and sixteenths, a stance, and seven
//! bits of tile-local `x` and seven of `y`. `docs/render/design_model.md` phase 2
//! took it apart one field at a time, and nothing of the packing is left here:
//!
//! - the **height** and the **fraction** are one point in
//!   [`crate::gbuffer`]'s position plane, unquantised. Every constant on the
//!   height track existed because those two were not exactly known, which is
//!   that plan's second root;
//! - the **normal** a stance stood in for is the plane beside it, written by
//!   the pass that knows the facing rather than recovered from four bits by the
//!   pass that reads it;
//! - the **id**, the **kind** and the **stance** are one `u32` in
//!   [`crate::gbuffer::pack_ids`]'s id plane, which is the whole of what was
//!   left once the other two moved out.
//!
//! So what remains is the *vocabulary*: [`Kind`] and [`Stance`], the two
//! enumerations both sides of the wire name a fragment with, and [`Place`] —
//! the **row**'s own shape, the two words a
//! [`SpriteQuad`](crate::sprite::SpriteQuad) or a
//! [`GroundQuad`](crate::ground::GroundQuad) carries on the GPU, where a tile
//! is still a literal `x`/`y` rather than an id into anything. That row is what
//! `docs/archive/render/gbuffer.md` step 3 moved a static's and a mobile's tile into, and
//! step 7 the ground's; it is read back by `blit.wgsl` as `face_instances[id]`
//! and its neighbours.
//!
//! A stance is still the one thing a position cannot say: a wall's two faces
//! are one tile and one plane, so a pixel of the street side and a pixel of the
//! room side carry the same point. `shaders/place_format.wesl` is this file's
//! twin, and nothing but a person reading both can check that the numbers
//! agree.
//!
//! A fragment a sprite discarded writes nothing anywhere, so what a G-buffer
//! holds is what is *visible*, which is the question lighting asks.

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
/// eleven values need and what [`Place::packed`] reserves — and in four more of
/// the G-buffer's id word, above the kind. See
/// [`crate::gbuffer::IDS_STANCE_SHIFT`]. Four where six values needed three,
/// and neither word had to grow: both have room to spare above it.
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
    /// Not a real stance: a routing sentinel `docs/archive/render/gbuffer.md` step 4c's mesh
    /// pass writes instead of one of the five above.
    ///
    /// `blit.wgsl` reads `face_instances[id]` for every other `Kind::Static`
    /// pixel; seeing this one in the id word's stance bits tells it to read
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
    pub fn of(tile: &openshard_tiles::StaticTile, facing: Option<crate::facing::Facing>) -> Self {
        use crate::facing::{
            Face,
            Facing,
        };

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
            Some(Facing::Corner { right, left }) => {
                match (right, left) {
                    (Face::North, Face::South) => Self::CornerNorthSouth,
                    (Face::North, Face::West) => Self::CornerNorthWest,
                    (Face::East, Face::South) => Self::CornerEastSouth,
                    (Face::East, Face::West) => Self::CornerEastWest,
                    _ => Self::Upright,
                }
            }
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

    /// Which way a surface with this stance looks, in tiles —
    /// [`Stance::of_normal`]'s inverse for the five it recognises.
    ///
    /// **No world pass reads this any more, and its shader twin is deleted.**
    /// `place_format.wesl`'s `outward` was the same table and
    /// `docs/render/design_model.md` phase 6c retired it: a drawn fragment's normal
    /// is the face of the box its own view ray met, measured rather than looked
    /// up. What still asks this is a *hand-built* G-buffer — `plan.rs`'s
    /// diagnostic pictures and the fixtures in `tests/` — which states its own
    /// scene by naming a stance and has no boxes to meet. Kept for that, and for
    /// [`Stance::of_normal`], which is a live reader on the producing side.
    ///
    /// Two of its rows are the *drawn* face rather than the one a camera sees:
    /// `FaceNorth` answers `(0, -1, 0)`, which is the side turned away from the
    /// viewer. That is the defect the impostor closed by construction, and it
    /// survives here because a fixture asking "which way does a north-edge panel
    /// face" is asking about the edge, not about the picture.
    ///
    /// The zero vector for [`Stance::Upright`], for a corner and for
    /// [`Stance::MeshFace`], and each is zero for its own reason. An upright
    /// picture has no side, which is a statement about what is *not* known —
    /// [`crate::light::Surface::normal`] returns `None` for the same thing. A
    /// **corner** never reaches a reader: `statics.wesl` resolves one to the
    /// face of the half the fragment was drawn on before it writes anything. And
    /// [`Stance::MeshFace`] is a routing sentinel rather than a facing at all —
    /// the mesh pass carries [`crate::mesh::Face::normal`] on its own vertices
    /// and never asks this.
    ///
    /// **[`Stance::Flat`] looks up, and land does not.** A wall's top cap is a
    /// flat static and wants the gate — `docs/archive/render/lighting.md`'s decision 27, a lamp
    /// beside a wall lighting its cap as fully as one standing over it. The
    /// ground carries the same stance and must *not* be gated, so what tells the
    /// two apart is the [`Kind`] beside it and never this: see
    /// [`crate::gbuffer::Fragment::normal`], which is where the pair is asked
    /// together.
    pub fn normal(self) -> [f32; 3] {
        use crate::facing::Face;

        let face = match self {
            Self::Flat => return [0.0, 0.0, 1.0],
            Self::FaceNorth => Face::North,
            Self::FaceEast => Face::East,
            Self::FaceSouth => Face::South,
            Self::FaceWest => Face::West,
            Self::Upright
            | Self::CornerNorthSouth
            | Self::CornerNorthWest
            | Self::CornerEastSouth
            | Self::CornerEastWest
            | Self::MeshFace => return [0.0; 3],
        };
        // Off [`crate::facing::Face::outward`] rather than four literals, so
        // that this and [`Stance::of_normal`] are one table read in two
        // directions and cannot drift apart.
        let [x, y] = face.outward();
        [x, y, 0.0]
    }

    /// Which real stance an honest [`crate::mesh::Face`] normal names, or
    /// `None` for a vector nothing here produces.
    ///
    /// [`crate::facing::Prism::mesh`] builds every one of its faces' normals
    /// from exactly two shapes — `[0, 0, 1]` for a top, or
    /// [`crate::facing::Face::outward`] folded into three dimensions for a
    /// riser — so this is a closed set today, not a general vector decoder:
    /// `docs/archive/render/gbuffer.md`'s "Not settled" section leaves the general case
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
    pub x:      u16,
    /// The tile's `y`.
    pub y:      u16,
    /// The height it was drawn at.
    ///
    /// The ground pass ignores this and writes the height its corner
    /// interpolation gives the pixel — a hillside's pixels each carry their own
    /// — so for a [`GroundQuad`](crate::ground::GroundQuad) it is the tile's
    /// base and nothing reads it.
    pub z:      i8,
    /// What drew it.
    pub kind:   Kind,
    /// Which way its picture faces. The ground pass ignores this — a land tile
    /// is a diamond by construction and its shader has always read the position
    /// inside it.
    pub stance: Stance,
}

impl Place {
    /// No place at all: the clear value, and what a pass that draws something
    /// outside the world writes.
    pub const NOWHERE: Self = Self {
        x:      0,
        y:      0,
        z:      0,
        kind:   Kind::Nothing,
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
            x:      at.x,
            y:      at.y,
            z:      at.z,
            kind:   Kind::Static,
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
            x:      at.x,
            y:      at.y,
            z:      at.z,
            kind:   Kind::Mobile,
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

    /// A cleared id texel and a [`Place::NOWHERE`] quad say the same thing, and
    /// it is the *kind* that says it.
    ///
    /// The clear is all zeros; `NOWHERE` writes a `z` of 0 into a row nothing
    /// reads. What they must agree on is the one field every reader branches on
    /// first: [`Kind::Nothing`] is zero on both sides, so a pixel nothing drew
    /// and a pixel a pass stamped as nothing are one word.
    /// [`crate::gbuffer::pack_ids`] is where the other half of that invariant
    /// lives — the kind is at the *bottom* of the word for exactly this.
    #[test]
    fn nothing_drawn_and_nothing_cleared_are_one_kind() {
        assert_eq!(Kind::Nothing as u32, 0, "the clear value's kind");
        assert_eq!(Place::NOWHERE.packed()[0], 0);
        assert_eq!(Place::NOWHERE.packed()[1] >> 8, 0, "and nothing else is read");
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
        use crate::facing::{
            Face,
            Facing,
        };

        let wall = openshard_tiles::StaticTile::default();
        assert_eq!(
            Stance::of(
                &wall,
                Some(Facing::Corner {
                    right: Face::East,
                    left:  Face::South,
                })
            ),
            Stance::CornerEastSouth,
        );
        assert_eq!(
            Stance::of(
                &wall,
                Some(Facing::Corner {
                    right: Face::South,
                    left:  Face::North,
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

    /// Every stance's normal round-trips, and the ones with no facing say so
    /// with a zero rather than with a plausible direction.
    ///
    /// [`Stance::normal`] is now the *producer* side — `statics.wgsl` writes it
    /// into the G-buffer's normal plane, and nothing downstream ever sees a
    /// stance again for the purpose of lighting. So what used to be checkable
    /// only by a person reading `blit.wgsl`'s `outward` beside this file is a
    /// round trip through [`Stance::of_normal`] for the five real facings, and
    /// an explicit zero for the six that have none.
    #[test]
    fn every_stance_s_normal_names_it_back() {
        for stance in [
            Stance::Flat,
            Stance::FaceNorth,
            Stance::FaceEast,
            Stance::FaceSouth,
            Stance::FaceWest,
        ] {
            let normal = stance.normal();
            assert_eq!(Stance::of_normal(normal), Some(stance), "{stance:?}");
            let length: f32 = normal.iter().map(|axis| axis * axis).sum();
            assert_eq!(length, 1.0, "{stance:?}'s normal is not a unit vector");
        }
        // And the six with nothing to say. A corner is here because it is
        // resolved to a face per fragment and must never reach a reader whole;
        // `MeshFace` because it is a routing sentinel and the mesh pass carries
        // its own geometry's normal instead.
        for stance in [
            Stance::Upright,
            Stance::CornerNorthSouth,
            Stance::CornerNorthWest,
            Stance::CornerEastSouth,
            Stance::CornerEastWest,
            Stance::MeshFace,
        ] {
            assert_eq!(stance.normal(), [0.0; 3], "{stance:?}");
        }
    }

    /// The kinds, one number each. `blit.wgsl` holds the same four and cannot be
    /// checked against this by anything but a person reading both.
    #[test]
    fn the_kinds_are_the_numbers_the_shader_has() {
        assert_eq!(Kind::Land as u32, 1);
        assert_eq!(Kind::Static as u32, 2);
        assert_eq!(Kind::Mobile as u32, 3);
    }

    /// The lowest and highest `z` a map holds both survive the offset, which is
    /// the whole reason there is one.
    #[test]
    fn the_ends_of_the_z_range_survive_the_offset() {
        assert_eq!(Place::of_static(Point::new(1, 1, -128)).packed()[1] & 0xFF, 0);
        assert_eq!(Place::of_static(Point::new(1, 1, 127)).packed()[1] & 0xFF, 255);
    }
}
