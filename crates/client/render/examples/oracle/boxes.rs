//! A hand-built box: the one spec a scene's occluder, its visible mesh and the
//! reference tracer's own body are all built from.
//!
//! Here rather than in the tool that draws them because two of them now exist —
//! `examples/boxes.rs`, which is the tool, and `tests/traced.rs`, which is the
//! gate. A second copy of [`box_mesh`] is the shape that drifts quietly: both
//! look right, both are read rather than run, and the day one of them puts a
//! corner a hair off the other, the gate and the tool are measuring two
//! different scenes and neither says so.

use openshard_client_render::camera::WorldSpot;
use openshard_client_render::mesh::{
    Face,
    Mesh,
};
use openshard_client_render::occlusion::Owner;
use openshard_client_render::solid::Solid;
use openshard_protocol::wire::Graphic;

/// One hand-built occluder and its visible box, together: the tile bucket
/// [`openshard_client_render::occlusion::Builder::add_raw`] and
/// `MeshFaceRow::tile` both need, and the exact `min`/`max` corners both
/// [`box_mesh`] and the occluder's own `space` are built from — one spec,
/// so mesh and occlusion cannot disagree about where the box stands.
pub struct BoxSpec {
    pub tile:    (u16, u16),
    pub min:     (f64, f64, f64),
    pub max:     (f64, f64, f64),
    /// Whose box this is — the graphic half of its [`Owner`], stated by the
    /// scene rather than taken from its place in a list.
    ///
    /// **A field since S3b's merge, and the reason is that a list position can
    /// only ever say "everyone is different".** `occlusion::merge` folds
    /// neighbours that share an `Owner`, so a scene where every box is its own
    /// graphic is a scene where nothing merges *ever* — which is what every
    /// scene here was, and it left the reference tracer blind to the merge (the
    /// blind spot `docs/occluders.md` records under § *The merge*). A run of one
    /// wall says so here, in one number, and the scenes that want distinct boxes
    /// say that just as explicitly.
    pub graphic: u16,
}

impl BoxSpec {
    pub fn solid(&self) -> Solid {
        Solid {
            min: WorldSpot {
                x: self.min.0,
                y: self.min.1,
                z: self.min.2,
            },
            max: WorldSpot {
                x: self.max.0,
                y: self.max.1,
                z: self.max.2,
            },
        }
    }
}

/// The key a hand-built scene gives one box, since it has no `tiledata` for the
/// builder to derive one from — `occlusion::Builder::add_raw`'s own doc.
///
/// [`BoxSpec::graphic`] is the whole of what tells two boxes of the `pair` scene
/// apart: they stand at one `z`, on one tile, spanning one height, so nothing
/// about their geometry is a distinguishing key and nothing should be. Identity
/// is stated, not measured — and since it is stated, a scene may also state that
/// two boxes are pieces of **one** static, which is what a run of wall is.
pub fn box_owner(b: &BoxSpec) -> Owner {
    Owner::new(b.min.2.floor() as i8, Graphic(b.graphic))
}

/// The same three-face box `two_cubes.rs` builds by hand — top, `+x`,
/// `+y` — from a `Solid`'s exact corners, so all three faces share
/// bit-identical vertices at the box's own edges instead of the crack two
/// independent `facing::Prism`s left (`two_cubes.rs`'s own doc has the full
/// account of that bug).
///
/// Three faces and not six: an isometric camera never sees the other three.
pub fn box_mesh(solid: Solid) -> Mesh {
    let (lo, hi) = (solid.min, solid.max);
    let mut mesh = Mesh::EMPTY;
    mesh.push(
        Face::new(
            &[
                WorldSpot {
                    x: lo.x,
                    y: lo.y,
                    z: hi.z,
                },
                WorldSpot {
                    x: hi.x,
                    y: lo.y,
                    z: hi.z,
                },
                WorldSpot {
                    x: hi.x,
                    y: hi.y,
                    z: hi.z,
                },
                WorldSpot {
                    x: lo.x,
                    y: hi.y,
                    z: hi.z,
                },
            ],
            [0.0, 0.0, 1.0],
        )
        .expect("a 4-corner ring"),
    );
    mesh.push(
        Face::new(
            &[
                WorldSpot {
                    x: hi.x,
                    y: lo.y,
                    z: hi.z,
                },
                WorldSpot {
                    x: hi.x,
                    y: hi.y,
                    z: hi.z,
                },
                WorldSpot {
                    x: hi.x,
                    y: hi.y,
                    z: lo.z,
                },
                WorldSpot {
                    x: hi.x,
                    y: lo.y,
                    z: lo.z,
                },
            ],
            [1.0, 0.0, 0.0],
        )
        .expect("a 4-corner ring"),
    );
    mesh.push(
        Face::new(
            &[
                WorldSpot {
                    x: lo.x,
                    y: hi.y,
                    z: hi.z,
                },
                WorldSpot {
                    x: hi.x,
                    y: hi.y,
                    z: hi.z,
                },
                WorldSpot {
                    x: hi.x,
                    y: hi.y,
                    z: lo.z,
                },
                WorldSpot {
                    x: lo.x,
                    y: hi.y,
                    z: lo.z,
                },
            ],
            [0.0, 1.0, 0.0],
        )
        .expect("a 4-corner ring"),
    );
    mesh
}
