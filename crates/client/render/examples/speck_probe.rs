//! Sweep a body's top face and count the pixels `impostor::meets` calls a wall.
//!
//! The instrument for one class of defect: a fragment standing *on* a box comes
//! back with a side face's normal, which reads as a dot of wall shading in the
//! middle of a floor. `docs/render/design_model.md` records it twice — once as
//! fourteen dots on a lattice of exactly one tile over a roof, once as the
//! specks a person reported at Britain's `(1501, 1659)`.
//!
//! [`impostor::meets`] already refuses a side face to a **lid** — `hi.z > lo.z`
//! guards both of them at once, since a lid's side faces are lines. This asks
//! the question that guard does not answer: what a box with real height does on
//! its own top face, where the two exits are equal at a corner and one rounding
//! apart beside it.
//!
//! No GPU and no client files. The sweep is over the face's own plane, at the
//! sub-pixel step the isometric projection actually puts fragments on, so a
//! sample here is a pixel there.
//!
//! ```sh
//! cargo run --release -p openshard-client-render --example speck_probe
//! ```

use openshard_client_render::impostor::{
    self,
    VIEW,
};
use openshard_client_render::light::WorldVec;

/// Which axis a meeting's normal names.
fn axis_of(normal: [f32; 3]) -> usize {
    normal
        .iter()
        .position(|n| *n != 0.0)
        .expect("a normal names an axis")
}

/// Every sample over one box's top face, and how many of them came back as a
/// side face rather than the lid they stand on.
///
/// `steps` samples a side, both edges inclusive, so the corners are hit exactly
/// — which is the case the tie rule is written for — and the interior is walked
/// at a step no coarser than a pixel.
fn sweep(lo: WorldVec, hi: WorldVec, steps: u32) -> (u32, u32, Vec<(f32, f32, usize)>) {
    let mut total = 0;
    let mut sides = 0;
    let mut where_ = Vec::new();
    for ix in 0..=steps {
        for iy in 0..=steps {
            let fx = ix as f32 / steps as f32;
            let fy = iy as f32 / steps as f32;
            let x = lo.x + (hi.x - lo.x) * fx;
            let y = lo.y + (hi.y - lo.y) * fy;
            // The ray through this point of the top face, taken from a point
            // back along `VIEW` — which is where a fragment's own position is
            // reconstructed from, so this is the same ray the shader has.
            let back = 4.0;
            let from = WorldVec::new(x - back * VIEW.x, y - back * VIEW.y, hi.z - back * VIEW.z);
            let met = impostor::meets(from, lo, hi);
            total += 1;
            let axis = axis_of(met.normal.array());
            if axis != 2 {
                sides += 1;
                if where_.len() < 12 {
                    where_.push((x - lo.x, y - lo.y, axis));
                }
            }
        }
    }
    (total, sides, where_)
}

fn main() {
    // A body with real height standing on one tile, at Britain's own
    // coordinates — the magnitude matters, because every one of these numbers
    // reaches the shader in `f32` and an ulp at 1500 is sixteen times an ulp at
    // 100. A counter (`0x0B40`, height 6) is the piece the report stands on.
    for (label, lo, hi) in [
        (
            "a counter at Britain's coordinates",
            WorldVec::new(1501.0, 1659.0, 27.0),
            WorldVec::new(1502.0, 1660.0, 33.0),
        ),
        (
            "the same body near the origin",
            WorldVec::new(101.0, 59.0, 27.0),
            WorldVec::new(102.0, 60.0, 33.0),
        ),
        (
            "a lid, which the `hi.z > lo.z` guard already covers",
            WorldVec::new(1501.0, 1659.0, 27.0),
            WorldVec::new(1502.0, 1660.0, 27.0),
        ),
    ] {
        let (total, sides, where_) = sweep(lo, hi, 88);
        println!("{label}: {sides} of {total} samples on the top face came back a side face");
        for (dx, dy, axis) in &where_ {
            let name = ["x", "y", "z"][*axis];
            println!("    at +{dx:.4}, +{dy:.4} into the face: {name}");
        }
    }
}
