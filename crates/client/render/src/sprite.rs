//! The quad two of the three passes draw: a picture, at its own size, in front
//! of or behind everything else.
//!
//! Ground is a shape — a diamond, or a patch over four heights — and the shader
//! decides which. A static and a mobile are not: each is a rectangle of pixels
//! standing somewhere, and the only thing that separates them is *where* the
//! rectangle goes and which atlas it came from. So they share this type, they
//! share [`SpriteRenderer`](crate::renderer::SpriteRenderer), and they share the
//! shader — what differs is entirely on the CPU, in [`crate::statics`] and
//! [`crate::mobiles`].
//!
//! Mirroring lives here rather than in the shader, and it is one subtraction:
//! a region whose width is negative samples its own texels backwards.

use crate::atlas::Region;
use crate::geometry::Rect;

/// One sprite: where it goes, how big it is, and what to sample.
///
/// The rectangle is the sprite's top-left corner and size in viewport pixels,
/// height already folded in — unlike a [`GroundQuad`](crate::ground::GroundQuad),
/// whose corners the shader lifts individually. A sprite has one `z` and one
/// picture, so there is nothing left for the shader to decide.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SpriteQuad {
    /// Where it goes and how big it is, in viewport pixels. The quad is
    /// exactly this size, so the art is drawn at its own scale and a texel is
    /// a pixel.
    pub rect: Rect,
    /// Where its picture lives in whichever atlas this pass is bound to.
    ///
    /// A negative width is a mirrored sprite — see [`SpriteQuad::mirrored`].
    pub region: Region,
    /// What it hides and what hides it: smaller is nearer. See [`crate::depth`].
    pub depth: f32,
    /// The wire hue to tint this sprite with, or `0` for none.
    ///
    /// Carried raw rather than as [`openshard_protocol::wire::Hue`] — like every
    /// other field here, this is the instance buffer's own layout, and `.0`
    /// belongs at the boundary the value crosses into it. See
    /// [`crate::hue::HueRamp`] for what a nonzero value means to the shader.
    pub hue: u32,
    /// Which tile in the world these pixels are, for the lighting pass.
    ///
    /// [`Place::NOWHERE`](crate::place::Place::NOWHERE) for a sprite that is not
    /// a thing standing in the street — the letters over a speaker's head are
    /// the case, and they are left unlit rather than dimmed by the night. See
    /// [`crate::place`].
    pub place: crate::place::Place,
    /// The row index of this face's other half, or `0` for a row with no
    /// other half.
    ///
    /// A corner static is one picture but two real faces — see
    /// `docs/gbuffer.md` decision 3 and step 4. `statics::collect` gives a
    /// corner's own drawn row a `twin` pointing at a second, undrawn row it
    /// appends for exactly this field; `statics.wgsl`'s fragment shader picks
    /// between the two ids per pixel, the same diagonal test that already
    /// resolves which face's `Stance` a pixel gets. Every other row — a wall,
    /// a floor, a mobile — never reads this field.
    pub twin: u32,
    /// **Which occluder of its own tile the thing drawn here is**, or
    /// [`OwnerId::NONE`](crate::occlusion::OwnerId::NONE) for one that is no
    /// occluder at all.
    ///
    /// `docs/lighting_height.md` phase 3, and the join it pays for: the grid
    /// numbers a cell's occluders, and the pass that draws one has to learn the
    /// number the grid gave it — [`crate::occlusion::Occlusion::owner_at`]. What
    /// `blit.wgsl` does with it is ask, for every solid a ray meets on this
    /// fragment's own cell, whether it is the one the fragment is a point of;
    /// before this it guessed, by asking whether the fragment's own height fell
    /// inside the solid's span.
    ///
    /// A mobile stamps `NONE` and is exempt from nothing — a billboard is no
    /// occluder, and a creature standing on a walled tile is genuinely in that
    /// wall's shadow. So does a static this frame's grid refused (no opacity,
    /// above the draw ceiling, hidden by the cutaway): what the walk cannot
    /// find, a fragment cannot be a point of.
    ///
    /// A `u32` and not a `u8` because this is the instance buffer's own layout
    /// and its unit is a word — the `.raw()` belongs at this boundary, the way
    /// `hue`'s does.
    pub owner: u32,
}

impl SpriteQuad {
    /// Bytes one quad occupies in the instance buffer.
    ///
    /// Nine floats and five `u32`s — position, size, region, depth, hue, the
    /// two words of [`crate::place::Place`], `twin` and `owner` — is 56 bytes on
    /// its own, but the stride is 64: `blit.wgsl`'s `FaceInstance` mirrors this
    /// struct field for field to read it a second time as storage
    /// (`docs/gbuffer.md` step 3), and WGSL's storage-buffer layout rounds a
    /// struct's size up to its own alignment — 16 bytes here, from the two
    /// `vec4<f32>` fields — so the struct is 64 bytes in the shader's own
    /// accounting whether or not this pads to match. Twelve bytes of trailing
    /// zero padding keep the two sides the same width; without them, every
    /// row but the first is `blit.wgsl`'s `face_instances[id]` reading a
    /// stride short of where this buffer actually put it — silently wrong for
    /// any frame with more than one distinct tile, which is every real one.
    /// Written by hand for the same reason
    /// [`GroundQuad::STRIDE`](crate::ground::GroundQuad::STRIDE)
    /// is — `bytemuck`'s derive emits `unsafe impl` and this workspace denies
    /// `unsafe_code`.
    pub const STRIDE: u64 = 16 * 4;

    /// The same region, sampled right to left.
    ///
    /// UO stores five of a mobile's eight facings and mirrors the rest, so this
    /// is not an effect: it is half of every creature that ever faces west. The
    /// flip is a region starting at the far edge with a negative width, which
    /// costs the shader nothing — it already interpolates `u + corner * du`.
    pub fn mirrored(region: Region) -> Region {
        Region {
            u: region.u + region.du,
            v: region.v,
            du: -region.du,
            dv: region.dv,
        }
    }

    /// Append this quad to an instance buffer.
    pub fn write(&self, out: &mut Vec<u8>) {
        for value in [
            self.rect.x,
            self.rect.y,
            self.rect.width,
            self.rect.height,
            self.region.u,
            self.region.v,
            self.region.du,
            self.region.dv,
            self.depth,
        ] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&self.hue.to_le_bytes());
        for word in self.place.packed() {
            out.extend_from_slice(&word.to_le_bytes());
        }
        out.extend_from_slice(&self.twin.to_le_bytes());
        out.extend_from_slice(&self.owner.to_le_bytes());
        // See `STRIDE`'s own doc: `blit.wgsl`'s mirror of this struct rounds
        // up to 64 bytes on its own, so this has to as well.
        out.extend_from_slice(&[0u8; 8]);
    }
}

/// The rows an instance buffer actually uploads, and how many of them the
/// pass draws.
///
/// [`split_corners`] appends a row past `drawn` for every corner static, so
/// the two are always the same slice — `rows[..drawn]` is the picture,
/// `rows[drawn..]` exists only so `SpriteRenderer::instances_buffer` can be
/// addressed by an id that names one of a corner's two real faces.
#[derive(Debug)]
pub struct InstanceRows {
    /// What the pass uploads to `SpriteRenderer::instances_buffer`.
    pub rows: Vec<SpriteQuad>,
    /// How many of `rows`, from the front, the draw call actually asks for —
    /// see `SpriteRenderer::render`'s own `drawn` parameter.
    pub drawn: u32,
}

/// Give a corner static's two faces their own row, instead of one row both
/// share.
///
/// `statics.wgsl`'s fragment shader already resolves, per pixel, which of a
/// corner's two faces (`crate::place::Stance::CornerNorthSouth` and its
/// siblings) it is drawing — see the shader's own `across > 0.0` test — and
/// writes the right [`crate::place::Stance`] for whichever half a pixel
/// lands on. What it could not do until this row existed is tell the two
/// halves apart by id: both wrote `@builtin(instance_index)` unchanged, so
/// nothing downstream of the `place` attachment could address "the corner's
/// north-ish half" on its own. See `docs/gbuffer.md` decision 3 and step 4.
///
/// `rows` is the pass's full instance list — map statics and items alike,
/// since both can carry a corner `Stance` (both go through
/// [`crate::statics::place`]) — after sorting, so the ids this assigns are
/// exactly the ones the vertex shader's `@builtin(instance_index)` will
/// hand out for the drawn half. One shadow row per corner is appended past
/// every drawn row, in a distinct, index-addressable range; the corner's own
/// row (still drawn, still the id the diagonal test's `across > 0.0` side
/// keeps) is left with `twin` pointing at its shadow.
pub fn split_corners(mut rows: Vec<SpriteQuad>) -> InstanceRows {
    let drawn = rows.len() as u32;
    let mut shadows = Vec::new();
    for row in &mut rows {
        if row.place.stance as u8 >= crate::place::STANCE_CORNER {
            let mut shadow = *row;
            shadow.twin = 0;
            row.twin = drawn + shadows.len() as u32;
            shadows.push(shadow);
        }
    }
    rows.extend(shadows);
    InstanceRows { rows, drawn }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The instance layout is a contract with a shader the compiler never sees.
    #[test]
    fn a_quad_writes_its_stride_and_nothing_more() {
        let quad = SpriteQuad {
            rect: Rect {
                x: 1.0,
                y: 2.0,
                width: 44.0,
                height: 88.0,
            },
            region: Region {
                u: 0.25,
                v: 0.5,
                du: 0.125,
                dv: 0.25,
            },
            depth: 0.75,
            hue: 0x8021,
            place: crate::place::Place::of_static(openshard_protocol::world::Point::new(7, 9, -2)),
            twin: 0xABCD,
            owner: 5,
        };
        let mut out = Vec::new();
        quad.write(&mut out);
        assert_eq!(out.len() as u64, SpriteQuad::STRIDE);
        assert_eq!(&out[40..44], &(7u32 | 9 << 16).to_le_bytes(), "the tile");
        assert_eq!(&out[44..48], &(126u32 | 2 << 8).to_le_bytes(), "z, then the kind");
        assert_eq!(&out[..4], &1.0f32.to_le_bytes());
        assert_eq!(&out[8..12], &44.0f32.to_le_bytes());
        assert_eq!(&out[16..20], &0.25f32.to_le_bytes());
        assert_eq!(&out[32..36], &0.75f32.to_le_bytes(), "depth");
        assert_eq!(&out[36..40], &0x8021u32.to_le_bytes(), "hue");
        assert_eq!(&out[48..52], &0xABCDu32.to_le_bytes(), "twin");
        assert_eq!(&out[52..56], &5u32.to_le_bytes(), "owner");
        assert_eq!(&out[56..64], &[0u8; 8], "padding out to the 64-byte stride");
    }

    /// A corner gets a shadow row its twin points at; a plain wall gets none.
    #[test]
    fn split_corners_gives_a_corner_a_shadow_row_and_leaves_a_wall_alone() {
        let at = openshard_protocol::world::Point::new(3, 4, 0);
        let wall = SpriteQuad {
            rect: Rect {
                x: 0.0,
                y: 0.0,
                width: 44.0,
                height: 44.0,
            },
            region: Region {
                u: 0.0,
                v: 0.0,
                du: 1.0,
                dv: 1.0,
            },
            depth: 0.5,
            hue: 0,
            place: crate::place::Place {
                stance: crate::place::Stance::FaceNorth,
                ..crate::place::Place::of_static(at)
            },
            twin: 0,
            owner: 0,
        };
        let corner = SpriteQuad {
            place: crate::place::Place {
                stance: crate::place::Stance::CornerEastSouth,
                ..crate::place::Place::of_static(at)
            },
            ..wall
        };
        let split = super::split_corners(vec![wall, corner]);

        assert_eq!(split.drawn, 2, "both rows are still drawn");
        assert_eq!(split.rows.len(), 3, "the corner gained one shadow row");
        assert_eq!(split.rows[0].twin, 0, "a wall has no twin");
        assert_eq!(split.rows[1].twin, 2, "the corner's twin is the shadow row");
        assert_eq!(
            split.rows[2].place, split.rows[1].place,
            "a corner's shadow shares its tile with the drawn half",
        );
        assert_eq!(split.rows[2].twin, 0, "a shadow row does not itself have a twin");
    }

    /// A mirrored region covers the same texels and starts at the other end.
    ///
    /// Both halves matter: the same *span*, or the sprite is drawn from
    /// somebody else's pixels, and the opposite *direction*, or nothing is
    /// mirrored at all. The second is what a single sign error leaves intact.
    #[test]
    fn a_mirrored_region_samples_the_same_texels_backwards() {
        let region = Region {
            u: 0.25,
            v: 0.5,
            du: 0.125,
            dv: 0.25,
        };
        let flipped = SpriteQuad::mirrored(region);
        assert_eq!(flipped.u, 0.375, "starts at the region's far edge");
        assert_eq!(flipped.u + flipped.du, region.u, "ends at its near one");
        assert_eq!((flipped.v, flipped.dv), (region.v, region.dv), "and only in x");
        // Twice is the original, which is the property a sign error breaks
        // without moving the sprite anywhere visible.
        let back = SpriteQuad::mirrored(flipped);
        assert_eq!((back.u, back.du), (region.u, region.du));
    }
}
