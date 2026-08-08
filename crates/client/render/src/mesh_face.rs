//! What `docs/gbuffer.md` step 4c's render pass draws from and writes: one
//! [`crate::mesh::Face`] at a time, not one sprite.
//!
//! The CPU side, the way [`crate::sprite`] is for the ordinary billboard
//! pipeline — [`crate::renderer::MeshFaceRenderer`] is the GPU half, in
//! `renderer.rs` beside `GroundRenderer`/`SpriteRenderer`, the same split this
//! crate already keeps between a pass's data and its pipeline.

use crate::geometry::Vec2;
use crate::place::Stance;

/// One [`crate::mesh::Face`]'s own row, addressed by `blit.wgsl`'s
/// `mesh_instances[id]` once a fragment's [`Stance::MeshFace`] sentinel
/// routes it here instead of to the statics pass's `face_instances`.
///
/// Deliberately not [`SpriteQuad`](crate::sprite::SpriteQuad)-shaped: no
/// `rect`/`region`/`hue`, because a mesh face has no picture of its own to
/// share that struct's layout with — just the tile it stands on and which
/// way it really faces, which is everything `blit.wgsl`'s `outward(stance)`
/// needs for a normal.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct MeshFaceRow {
    /// The tile this face's static stands on.
    pub tile: (u16, u16),
    /// This face's real stance — never [`Stance::MeshFace`] itself, which is
    /// only ever the attachment's own routing sentinel, not a row's content.
    pub stance: Stance,
    /// Which occluder of `tile` the static this face belongs to is, or
    /// [`OwnerId::NONE`](crate::occlusion::OwnerId::NONE) for one this frame's
    /// grid has no solid for.
    ///
    /// `docs/lighting_height.md` phase 3, and the mesh half of
    /// [`SpriteQuad::owner`](crate::sprite::SpriteQuad::owner) — see that field
    /// for what `blit.wgsl` asks it. A flight of steps is where the phase's own
    /// first decision shows: its treads are a lid and a riser each, several
    /// solids from one `Builder::add`, and every one of its faces carries the one
    /// owner that call gave them all.
    pub owner: u32,
}

impl MeshFaceRow {
    /// Bytes one row occupies in `blit.wgsl`'s `mesh_instances` storage
    /// buffer: three `u32`s, unlike [`SpriteQuad::STRIDE`](crate::sprite::SpriteQuad::STRIDE)'s
    /// padded 64 — this buffer is never bound as a vertex attribute, only
    /// read as storage, so there is no vertex-buffer stride to match and
    /// WGSL's own natural, unpadded struct size is what both sides use. A
    /// struct of scalars aligns to four bytes, so three of them is twelve and
    /// nothing is rounded up.
    pub const STRIDE: u64 = 12;

    /// Append this row to a storage buffer's upload bytes.
    pub fn write(&self, out: &mut Vec<u8>) {
        let tile = u32::from(self.tile.0) | u32::from(self.tile.1) << 16;
        out.extend_from_slice(&tile.to_le_bytes());
        out.extend_from_slice(&(self.stance as u32).to_le_bytes());
        out.extend_from_slice(&self.owner.to_le_bytes());
    }
}

/// One vertex of the mesh pass's raw, non-instanced vertex buffer.
///
/// Not split into a unit quad plus per-instance data the way
/// [`SpriteQuad`](crate::sprite::SpriteQuad) is: a [`crate::mesh::Face`]'s
/// true screen shape is an arbitrary projected quadrilateral, not an
/// axis-aligned rectangle a shader could reconstruct from an origin and a
/// size, so every corner is a full vertex, fan-triangulated CPU-side by
/// [`crate::mesh::Face::fan`].
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct MeshFaceVertex {
    /// This corner in `Camera::to_view_exact`'s space — the same
    /// pre-viewport-zoom space [`SpriteQuad::rect`](crate::sprite::SpriteQuad::rect)
    /// is measured in. The vertex shader applies the same `Viewport`-uniform
    /// step `statics.wgsl` does to reach real pixels; the projection is not
    /// repeated a second time here or there.
    pub screen: Vec2,
    /// This corner's true world position, carried *in addition to* `screen`
    /// rather than recomputed from it.
    ///
    /// Interpolating this across a triangle gives the fragment shader an
    /// exact per-pixel world position for free — the projection is affine and
    /// every face is planar, so linear interpolation of the vertex's own
    /// world coordinates is exact at every fragment inside it. That is what
    /// replaces both a constant sub-tile-fraction/height approximation and a
    /// second copy of `statics.wgsl`'s per-stance analytic inversion.
    pub world: [f32; 3],
    /// The enclosing static's own `SpriteQuad::depth`, reused rather than
    /// recomputed — `docs/gbuffer.md` decision 4's "stays depth-consistent
    /// with the visible pass": a second depth formula is a second chance to
    /// disagree with the one that already decided this static's pixels.
    pub depth: f32,
    /// This face's row index in the frame's `MeshFaceRow` list — identical
    /// for all six vertices [`crate::mesh::Face::fan`] produces for one face.
    pub id: u32,
    /// The tile this face's static stands on — [`MeshFaceRow::tile`] again,
    /// carried on the vertex too rather than looked up a second time.
    ///
    /// `mesh_face.wgsl`'s fragment stage needs it to place this fragment
    /// inside its own tile: `world`'s `x`/`y` are exact, but a face whose own
    /// edge sits on a whole number — this tread's outer corner, at the
    /// flight's own footprint boundary — has nothing to tell `fract(world.xy)`
    /// which of the two tiles that shared edge belongs to, and it always
    /// answers the one `floor` rounds up to. Subtracting the known tile
    /// instead of flooring the position is the fix: `docs/lighting.md`'s
    /// shadow-raymarch anomaly, an isolated lit pixel on an otherwise
    /// evenly-shadowed flat face, sitting exactly on that edge.
    pub tile: [f32; 2],
}

impl MeshFaceVertex {
    /// Bytes one vertex occupies: `Float32x2` (screen), `Float32x3` (world),
    /// `Float32` (depth), `Uint32` (id) and `Float32x2` (tile), with no
    /// padding to match — unlike `SpriteQuad`, nothing reads this buffer a
    /// second time as storage, so there is no `blit.wgsl`-side mirror whose
    /// alignment to round up to.
    pub const STRIDE: u64 = 36;

    /// Append this vertex to a vertex buffer's upload bytes.
    pub fn write(&self, out: &mut Vec<u8>) {
        for value in [
            self.screen.x,
            self.screen.y,
            self.world[0],
            self.world[1],
            self.world[2],
            self.depth,
        ] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&self.id.to_le_bytes());
        for value in self.tile {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The row's layout is a contract with `blit.wgsl`'s `MeshFaceInstance` —
    /// stated here the same way `SpriteQuad::write`'s own test states its.
    #[test]
    fn a_row_packs_its_tile_and_stance() {
        let row = MeshFaceRow {
            tile: (0x1234, 0x5678),
            stance: Stance::FaceWest,
            owner: 3,
        };
        let mut out = Vec::new();
        row.write(&mut out);
        assert_eq!(out.len() as u64, MeshFaceRow::STRIDE);
        assert_eq!(
            &out[0..4],
            &(0x5678_1234u32).to_le_bytes(),
            "y in the high half, x in the low"
        );
        assert_eq!(&out[4..8], &(Stance::FaceWest as u32).to_le_bytes());
        assert_eq!(&out[8..12], &3u32.to_le_bytes(), "the owner");
    }

    /// The vertex layout is a contract with `mesh_face.wgsl`'s `vs_main`.
    #[test]
    fn a_vertex_writes_its_stride_and_nothing_more() {
        let vertex = MeshFaceVertex {
            screen: Vec2::new(1.5, 2.5),
            world: [100.0, 101.0, 5.0],
            depth: 0.75,
            id: 0xABCD,
            tile: [100.0, 100.0],
        };
        let mut out = Vec::new();
        vertex.write(&mut out);
        assert_eq!(out.len() as u64, MeshFaceVertex::STRIDE);
        assert_eq!(&out[0..4], &1.5f32.to_le_bytes());
        assert_eq!(&out[4..8], &2.5f32.to_le_bytes());
        assert_eq!(&out[8..12], &100.0f32.to_le_bytes());
        assert_eq!(&out[12..16], &101.0f32.to_le_bytes());
        assert_eq!(&out[16..20], &5.0f32.to_le_bytes());
        assert_eq!(&out[20..24], &0.75f32.to_le_bytes(), "depth");
        assert_eq!(&out[24..28], &0xABCDu32.to_le_bytes(), "id");
        assert_eq!(&out[28..32], &100.0f32.to_le_bytes(), "tile.x");
        assert_eq!(&out[32..36], &100.0f32.to_le_bytes(), "tile.y");
    }
}
