//! What `docs/gbuffer.md` step 4c's render pass draws from and writes: one
//! [`crate::mesh::Face`] at a time, not one sprite.
//!
//! The CPU side, the way [`crate::sprite`] is for the ordinary billboard
//! pipeline — [`crate::renderer::MeshFaceRenderer`] is the GPU half, in
//! `renderer.rs` beside `GroundRenderer`/`SpriteRenderer`, the same split this
//! crate already keeps between a pass's data and its pipeline.

use crate::camera::ViewPoint;
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
    pub tile:   (u16, u16),
    /// This face's real stance — never [`Stance::MeshFace`] itself, which is
    /// only ever the id word's own routing sentinel, not a row's content.
    pub stance: Stance,
    /// **Which solid of the grid this face is the drawn form of**, or
    /// [`SolidId::NOBODY`](crate::occlusion::SolidId::NOBODY) for a face this
    /// frame's grid has no solid for.
    ///
    /// `docs/lighting_rebuild.md` phase 4, and the mesh half of
    /// [`SpriteQuad::owner`](crate::sprite::SpriteQuad::owner) — which is still
    /// an owner, because a sprite instance covers a corner's *two* panels and
    /// only a fragment's own stance says which; a mesh face is one primitive by
    /// construction and can carry the id outright.
    ///
    /// It was that owner here too, and a flight of steps is exactly where one
    /// level of identity is not enough: its treads are a lid and a riser each,
    /// six solids from one `Builder::add`, all wearing one owner. The walk then
    /// needed a height to tell which lid a fragment stood on. This is the number
    /// that replaced it — [`crate::occlusion::Occlusion::id_of`] of the face's
    /// own [`Part`](crate::occlusion::Part), joined where the mesh is built.
    pub solid:  u32,
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
        out.extend_from_slice(&self.solid.to_le_bytes());
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
    pub screen: ViewPoint,
    /// This corner's true world position, carried *in addition to* `screen`
    /// rather than recomputed from it.
    ///
    /// Interpolating this across a triangle gives the fragment shader an
    /// exact per-pixel world position for free — the projection is affine and
    /// every face is planar, so linear interpolation of the vertex's own
    /// world coordinates is exact at every fragment inside it. That is what
    /// replaces both a constant sub-tile-fraction/height approximation and a
    /// second copy of `statics.wgsl`'s per-stance analytic inversion.
    pub world:  [f32; 3],
    /// The enclosing static's own `SpriteQuad::depth`, reused rather than
    /// recomputed — `docs/gbuffer.md` decision 4's "stays depth-consistent
    /// with the visible pass": a second depth formula is a second chance to
    /// disagree with the one that already decided this static's pixels.
    pub depth:  f32,
    /// This face's row index in the frame's `MeshFaceRow` list — identical
    /// for all six vertices [`crate::mesh::Face::fan`] produces for one face.
    pub id:     u32,
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
    pub tile:   [f32; 2],
    /// This face's own unit normal — [`crate::mesh::Face::normal`], carried
    /// whole rather than named by [`MeshFaceRow::stance`] and read back out of
    /// one.
    ///
    /// The two say the same thing today, and only because
    /// [`crate::place::Stance::of_normal`] refuses anything but the five
    /// vectors [`crate::facing::Prism::mesh`] builds — a mesh whose faces are
    /// not axis-aligned would have a normal no stance can name, and this is the
    /// field that would still be right. `docs/lighting_rebuild.md` phase 2 is
    /// the decision to carry it: the G-buffer's normal plane is written from
    /// here, and nothing downstream turns a stance into a direction any more.
    ///
    /// Flat across the face, the same as `id` and `tile` — a [`crate::mesh::Face`]
    /// is planar, so there is nothing for the rasteriser to interpolate.
    pub normal: [f32; 3],
    /// This face's own albedo, **linear** — the same convention every other
    /// surface's is stated in (`oracle::pathtrace::Albedos`'s own doc), and
    /// encoded to sRGB once, in `mesh_face.wgsl`'s fragment stage, the way
    /// every other producer of [`crate::blit::WORLD_FORMAT`] does.
    ///
    /// `docs/lighting_rebuild.md` phase 6d: this pass wrote no colour at all
    /// until then, because the enclosing static's own billboard sprite always
    /// drew one and this pass ran only to correct a normal. Off real statics
    /// now, the four hand-built diagnostic scenes that still use it have no
    /// sprite underneath to fall back on, so a face needs to say what it is
    /// worth. Flat across the face like `id`, `tile` and `normal`.
    pub colour: [f32; 3],
}

impl MeshFaceVertex {
    /// Bytes one vertex occupies: `Float32x2` (screen), `Float32x3` (world),
    /// `Float32` (depth), `Uint32` (id), `Float32x2` (tile), `Float32x3`
    /// (normal) and `Float32x3` (colour), with no padding to match — unlike
    /// `SpriteQuad`, nothing reads this buffer a second time as storage, so
    /// there is no `blit.wgsl`-side mirror whose alignment to round up to.
    pub const STRIDE: u64 = 60;

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
        for value in self
            .tile
            .iter()
            .chain(self.normal.iter())
            .chain(self.colour.iter())
        {
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
            tile:   (0x1234, 0x5678),
            stance: Stance::FaceWest,
            solid:  3,
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
        assert_eq!(&out[8..12], &3u32.to_le_bytes(), "the solid this face is");
    }

    /// The vertex layout is a contract with `mesh_face.wgsl`'s `vs_main`.
    #[test]
    fn a_vertex_writes_its_stride_and_nothing_more() {
        let vertex = MeshFaceVertex {
            screen: ViewPoint::new(1.5, 2.5),
            world:  [100.0, 101.0, 5.0],
            depth:  0.75,
            id:     0xABCD,
            tile:   [100.0, 100.0],
            normal: [0.0, 0.0, 1.0],
            colour: [0.72, 0.70, 0.66],
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
        assert_eq!(&out[36..40], &0.0f32.to_le_bytes(), "normal.x");
        assert_eq!(&out[40..44], &0.0f32.to_le_bytes(), "normal.y");
        assert_eq!(&out[44..48], &1.0f32.to_le_bytes(), "normal.z");
        assert_eq!(&out[48..52], &0.72f32.to_le_bytes(), "colour.r");
        assert_eq!(&out[52..56], &0.70f32.to_le_bytes(), "colour.g");
        assert_eq!(&out[56..60], &0.66f32.to_le_bytes(), "colour.b");
    }
}
