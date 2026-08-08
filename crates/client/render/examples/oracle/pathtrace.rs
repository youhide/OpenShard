//! The reference tracer's own side of a scene, and the judging of the two
//! pictures against each other.
//!
//! Shared by `examples/boxes.rs` — the tool a person points at a scene — and
//! `tests/traced.rs`, the gate that runs the same comparison under
//! `cargo test`. **The judging is the part that must not be duplicated.** The
//! pipeline boilerplate around it (build a scene, render it, read it back) is
//! copied all over this crate's GPU tests already and a second copy is a
//! nuisance; a second copy of *what counts as a disagreement* is a second
//! opinion, and the day the two drift, the gate is green about a rule the tool
//! no longer applies.
//!
//! What lives here is therefore: the tracer's view of a scene the renderer just
//! drew ([`Mirror`]), and the pixel-by-pixel verdict ([`compare`]). What does
//! not: anything about how the frame was produced, and anything about what to
//! do with the answer.
//!
//! The reasoning behind each split — why an out-of-reach pixel, a back-facing
//! pixel and a silhouette pixel are three different things and none of them is
//! a shadow — is in `docs/lighting_reference.md`.

use std::collections::BTreeMap;

use openshard_client_pathtrace::aabb as pt_aabb;
use openshard_client_pathtrace::camera as pt_camera;
use openshard_client_pathtrace::light as pt_light;
use openshard_client_pathtrace::scene as pt_scene;
use openshard_client_pathtrace::trace as pt_trace;
use openshard_client_pathtrace::vector as pt_vector;
use openshard_client_render::camera::WorldSpot;
use openshard_client_render::light;
use openshard_client_render::place::Stance;

// `super`, not `crate`: this module is reached from two different crate roots —
// an example and a test — and only a relative path is true in both.
use super::boxes::BoxSpec;
use super::{Drawn, Shade};

/// The same scene, in the tracer's own terms.
///
/// Two things are taken from the renderer to build it, and both arrive as
/// **values rather than as formulas** — see `docs/lighting_reference.md`.
pub struct Mirror {
    pub scene: pt_scene::Scene,
    pub camera: pt_camera::Parallel,
    pub flame: pt_light::Light,
}

impl Mirror {
    /// Build the tracer's view of a scene of boxes lit by one flame.
    ///
    /// `to_pixel` is the *renderer's* own world-to-pixel map, handed over as a
    /// black box for [`pt_camera::Parallel::measure`] to recover. This is the
    /// one thing the tracer takes from the render crate, and taking it as
    /// values rather than as a formula is what stops the reference camera from
    /// drifting into being nobody's camera.
    pub fn of(
        boxes: &[BoxSpec],
        light_at: WorldSpot,
        light_radius: f64,
        to_pixel: &dyn Fn(WorldSpot) -> (f64, f64),
    ) -> Self {
        // **The metric.** The tracer's world is isotropic and this one is not: a
        // step of one in `x` is a tile and a step of one in `z` is a tile's
        // eleventh (`light::Z_PER_TILE`, which the renderer states and uses for
        // exactly this reason — its own falloff needs all three axes in one
        // unit). Read from the engine's constant rather than written down again:
        // a second copy of it here would be a reference measuring a
        // differently-shaped world.
        //
        // Visibility alone would survive getting this wrong — it is invariant
        // under any affine change of coordinates — which is precisely why it has
        // to be right anyway: the soft-shadow and bounce modes are not, and a
        // scale error would show up there as a plausible-looking picture rather
        // than as a failure.
        let z_per_tile = f64::from(light::Z_PER_TILE);
        let isotropic = |x: f64, y: f64, z: f64| pt_vector::Vec3::new(x, y, z / z_per_tile);

        let scene = pt_scene::Scene {
            bodies: boxes
                .iter()
                .map(|b| pt_scene::Body {
                    shape: pt_aabb::Aabb::between(
                        isotropic(b.min.0, b.min.1, b.min.2),
                        isotropic(b.max.0, b.max.1, b.max.2),
                    ),
                    albedo: [0.72, 0.70, 0.66],
                })
                .collect(),
            ground: Some(pt_scene::Ground {
                z: 0.0,
                albedo: [0.42, 0.44, 0.40],
            }),
        };

        // The camera, measured through the renderer's own projection. `about`
        // sits in the scene and the span is wide, both for precision: the map
        // narrows to `f32` at world-pixel magnitudes in the thousands, so a
        // central difference over a wide baseline is what keeps that noise out
        // of the recovered columns. The tolerance is a hundredth of a pixel —
        // far under anything visible, far over the noise.
        let camera = pt_camera::Parallel::measure(
            |at| {
                to_pixel(WorldSpot {
                    x: at.x,
                    y: at.y,
                    z: at.z * z_per_tile,
                })
            },
            isotropic(light_at.x, light_at.y, 0.0),
            32.0,
            1e-2,
        );

        let flame = pt_light::Light {
            at: isotropic(light_at.x, light_at.y, light_at.z),
            emitter: pt_light::Emitter::Point,
            // The renderer's own windowed curve and its own radius, so "outside
            // the torch's reach" means the same thing on both sides of the
            // comparison. A physical inverse square here would darken the far
            // half of the frame and every one of those pixels would read as a
            // disagreement about geometry, which is not what any of them would
            // be about.
            falloff: pt_light::Falloff::Windowed { reach: light_radius },
            colour: [1.0, 1.0, 1.0],
            intensity: 6.0,
        };

        Self { scene, camera, flame }
    }

    /// Render this scene in one light model, deterministically.
    ///
    /// # Panics
    ///
    /// If the render is an estimate rather than an exact answer. Every
    /// comparison below is against a renderer with hard shadows, and a
    /// soft-shadow render disagreeing with a hard-shadow one is not a finding.
    pub fn render(&self, brdf: pt_trace::Brdf, width: u32, height: u32) -> pt_trace::Image {
        let image = pt_trace::render(
            &self.scene,
            &self.camera,
            std::slice::from_ref(&self.flame),
            &pt_trace::Settings {
                brdf,
                ..pt_trace::Settings::degenerate()
            },
            width,
            height,
        );
        assert!(
            image.is_exact(),
            "the gate only means anything against an exact render, and this one is an estimate"
        );
        image
    }
}

/// What the renderer left on the pixels, as the comparison needs it.
pub struct Frame<'a> {
    pub width: u32,
    pub height: u32,
    /// The `place` attachment, decoded — who drew each pixel.
    pub drawn: &'a [Drawn],
    /// The `View::Shadow` picture, `RGBA8`.
    pub shadow: &'a [u8],
    /// Which box and stance each mesh-face instance row is, as the tool that
    /// pushed the rows recorded it rather than as anybody's guess about the
    /// order they went in.
    pub face_rows: &'a [(usize, Stance, u32)],
}

/// Every count the comparison produces, and the two maps it produced them from.
///
/// One struct rather than a printed line, because two callers want different
/// things out of it: the tool prints and draws, the gate asserts.
pub struct Verdict {
    /// Pixels where both sides agree what surface is there and the comparison
    /// therefore had an opinion. Asserted non-trivial by [`compare`]: a
    /// detector that silently compares nothing reads exactly like a detector
    /// that found nothing.
    pub compared: usize,
    /// Of those, the ones that disagree away from any edge. **This is the
    /// number.** Anything else has an explanation that is not the walk.
    pub interior: usize,
    /// And the ones that disagree within a pixel of a shadow's own edge, which
    /// half a pixel of sampling difference explains.
    pub edge: usize,
    /// Pixels the two disagree about *which surface is there*, on a silhouette
    /// — the same half a pixel, deciding which of two surfaces a ray meets.
    pub silhouette: usize,
    /// And the ones that do not sit on one, which nothing sub-pixel explains.
    pub interior_surface: usize,
    /// What those were, by name.
    pub surface_pairs: BTreeMap<String, usize>,
    /// Pixels of a surface whose own normal points away from the flame, and how
    /// many of those the frame draws lit.
    pub back_facing: usize,
    pub back_facing_lit: usize,
    /// How many compared pixels the *choice of light model* decides — the two
    /// models rendered and subtracted, rather than inferred from a back-face
    /// count.
    pub model_decides: usize,
    /// Pixels with nothing the two have a common vocabulary for: the cleared
    /// background, or a stance the tracer's scene has no counterpart for.
    pub nothing_drawn: usize,
    /// The frame's own lit/shadowed decision and the tracer's, one bit a pixel,
    /// `None` where the pixel was not compared. The picture and the comparison
    /// are the same data.
    pub engine_lit: Vec<Option<bool>>,
    pub traced_lit: Vec<Option<bool>>,
    /// A handful of each kind of disagreement, in words, for a person.
    pub examples: Vec<String>,
    pub surface_examples: Vec<String>,
}

impl Verdict {
    /// The whole verdict as the two lines a run prints, plus a line per named
    /// disagreement.
    pub fn report(&self) -> String {
        let mut out = format!(
            "path tracer vs rendered View::Shadow: {} pixels compared, {} disagree in the interior, {} \
             on an edge (expected); {} pixels the two disagree about which surface is there ({} of them \
             on a silhouette, {} not), {} with nothing drawn\n",
            self.compared,
            self.interior,
            self.edge,
            self.silhouette + self.interior_surface,
            self.silhouette,
            self.interior_surface,
            self.nothing_drawn,
        );
        out.push_str(&format!(
            "  compared in the engine's own light model (no cosine, no self-occlusion): {} of those \
             pixels are surfaces facing away from the flame, the frame draws {} of them lit, and the \
             choice of model decides {} pixels of the whole comparison — that last number is what a \
             physical `N·L` would move\n",
            self.back_facing, self.back_facing_lit, self.model_decides,
        ));
        for (pair, count) in &self.surface_pairs {
            out.push_str(&format!(
                "  [different surface, not on a silhouette] {count}: {pair}\n"
            ));
        }
        for example in self.surface_examples.iter().chain(&self.examples) {
            out.push_str(example);
            out.push('\n');
        }
        out
    }
}

/// Which surface of the tracer's own scene the renderer says drew a pixel.
///
/// [`None`] where the two have no common vocabulary for it — the cleared
/// background, or a stance the tracer's scene has no counterpart for. A pixel
/// with no answer here is not compared, and is counted as not compared.
fn traced_surface(texel: &Drawn, face_rows: &[(usize, Stance, u32)]) -> Option<pt_scene::Surface> {
    if texel.kind == openshard_client_render::place::Kind::Land as u32 {
        return Some(pt_scene::Surface::Ground);
    }
    if texel.kind == openshard_client_render::place::Kind::Static as u32
        && texel.stance == Stance::MeshFace as u32
    {
        return face_rows
            .iter()
            .find(|(_, _, id)| *id == texel.id)
            .map(|(box_index, _, _)| pt_scene::Surface::Body(*box_index));
    }
    None
}

/// What the frame says drew a pixel, in words, for a disagreement report.
///
/// Not [`traced_surface`]'s answer: this one keeps the *face* — a box's own east
/// side and its lid are one body to the tracer and two rows to the renderer, and
/// which of them won a pixel is the whole content of a "which surface is there"
/// disagreement.
fn engine_side(texel: &Drawn, face_rows: &[(usize, Stance, u32)]) -> String {
    if texel.kind == openshard_client_render::place::Kind::Land as u32 {
        return "the ground".to_owned();
    }
    match face_rows.iter().find(|(_, _, id)| *id == texel.id) {
        Some((box_index, stance, _)) => format!("box {box_index}'s {stance:?}"),
        None => format!("kind {} stance {} row {}", texel.kind, texel.stance, texel.id),
    }
}

/// Whether the pixel at `(x, y)` has anything but its own answer in its eight-
/// neighbourhood — the boundary of whatever `map` is a map of.
///
/// The two renderers answer about *different points*: the tracer about the world
/// point under a pixel's centre, the shader about the fragment the rasteriser
/// wrote, quantised to a hundred-and-twenty-eighth of a tile. Half a pixel
/// decides the answer exactly at a boundary and nowhere else, so a disagreement
/// on one is not a finding and a disagreement away from one cannot be explained
/// by sub-pixel anything.
///
/// One function over any map, because that argument is not about *what* is being
/// compared: it is as true of "which surface is there" along a silhouette as it
/// is of "is this lit" along a shadow's edge, and a second copy of it for the
/// second map would be a second place for the neighbourhood to change.
fn on_an_edge<T: PartialEq>(map: &[Option<T>], width: u32, height: u32, x: u32, y: u32) -> bool {
    let own = &map[(y * width + x) as usize];
    (-1i32..=1).any(|dy| {
        (-1i32..=1).any(|dx| {
            let (nx, ny) = (x as i32 + dx, y as i32 + dy);
            if nx < 0 || ny < 0 || nx as u32 >= width || ny as u32 >= height {
                return false;
            }
            &map[(ny as u32 * width + nx as u32) as usize] != own
        })
    })
}

/// Lay the frame and the tracer's two renders beside each other and count where
/// they disagree.
///
/// `engine_model` is the render in [`pt_trace::Brdf::Flat`] — the shipped
/// renderer's own light model, and the one the comparison is *about*.
/// `physical` is the same scene in [`pt_trace::Brdf::Lambert`], and is used for
/// exactly one thing: subtracting the two to measure how many pixels the choice
/// of model decides.
///
/// # Panics
///
/// If it compared too few pixels to mean anything.
pub fn compare(engine_model: &pt_trace::Image, physical: &pt_trace::Image, frame: Frame<'_>) -> Verdict {
    let Frame {
        width,
        height,
        drawn,
        shadow,
        face_rows,
    } = frame;

    // Both pictures as one bit a pixel, so the comparison and the image are the
    // same data. `None` where the pixel is not one the two can be compared on.
    let mut engine_lit: Vec<Option<bool>> = vec![None; (width * height) as usize];
    let mut traced_lit: Vec<Option<bool>> = vec![None; (width * height) as usize];
    // And what each side says is *there*, kept as maps for the same reason the
    // lit bits are: whether a surface disagreement sits on a silhouette or in
    // the middle of a face is a question about the pixel's neighbours, and it
    // cannot be asked one pixel at a time.
    let mut engine_surface: Vec<Option<pt_scene::Surface>> = vec![None; (width * height) as usize];
    let mut traced_surfaces: Vec<Option<pt_scene::Surface>> = vec![None; (width * height) as usize];
    let mut verdict = Verdict {
        compared: 0,
        interior: 0,
        edge: 0,
        silhouette: 0,
        interior_surface: 0,
        surface_pairs: BTreeMap::new(),
        back_facing: 0,
        back_facing_lit: 0,
        model_decides: 0,
        nothing_drawn: 0,
        engine_lit: Vec::new(),
        traced_lit: Vec::new(),
        examples: Vec::new(),
        surface_examples: Vec::new(),
    };

    for pixel in 0..(width * height) as usize {
        engine_surface[pixel] = traced_surface(&drawn[pixel], face_rows);
        traced_surfaces[pixel] = engine_model.pixels[pixel].seen.map(|seen| seen.surface);
        if engine_surface[pixel].is_none() {
            verdict.nothing_drawn += 1;
            continue;
        }
        if engine_surface[pixel] != traced_surfaces[pixel] {
            // Not a lighting disagreement, and counting it as one would file a
            // depth-sorting difference under the wrong defect entirely. The
            // isometric painter's order and a ray's own nearest hit are two
            // different answers to "what is in front", and where they differ
            // neither picture is about the other's surface. Counted below,
            // where the neighbourhood is available to say which kind it is.
            continue;
        }
        let frame_lit = Shade::of([shadow[pixel * 4], shadow[pixel * 4 + 1], shadow[pixel * 4 + 2]]).lit();
        let (x, y) = ((pixel as u32) % width, (pixel as u32) / width);
        let visibility = engine_model.visibility(x, y, 0);
        let lit = |seen: pt_trace::Visibility| seen.within_reach && seen.reached > 0.5;
        if !visibility.faces_light {
            verdict.back_facing += 1;
            verdict.back_facing_lit += usize::from(frame_lit);
        }
        // Asked of every pixel and not only of the back-facing ones: "the model
        // decides exactly the pixels facing away" is the expectation, and an
        // expectation a detector only ever measures where it already holds is
        // not one the detector can report on.
        verdict.model_decides += usize::from(lit(visibility) != lit(physical.visibility(x, y, 0)));
        engine_lit[pixel] = Some(frame_lit);
        traced_lit[pixel] = Some(lit(visibility));
    }

    for y in 0..height {
        for x in 0..width {
            let pixel = (y * width + x) as usize;
            if engine_surface[pixel].is_some() && engine_surface[pixel] != traced_surfaces[pixel] {
                let rim = on_an_edge(&engine_surface, width, height, x, y)
                    || on_an_edge(&traced_surfaces, width, height, x, y);
                match rim {
                    true => verdict.silhouette += 1,
                    false => {
                        verdict.interior_surface += 1;
                        let what = format!(
                            "the frame draws {}, the tracer sees {}",
                            engine_side(&drawn[pixel], face_rows),
                            match traced_surfaces[pixel] {
                                Some(surface) => format!("{surface:?}"),
                                None => "nothing at all".to_owned(),
                            }
                        );
                        if verdict.surface_examples.len() < 8 {
                            verdict
                                .surface_examples
                                .push(format!("  [pixel ({x}, {y})] {what}"));
                        }
                        *verdict.surface_pairs.entry(what).or_default() += 1;
                    }
                }
                continue;
            }
            let (Some(engine), Some(traced)) = (engine_lit[pixel], traced_lit[pixel]) else {
                continue;
            };
            verdict.compared += 1;
            if engine == traced {
                continue;
            }
            if on_an_edge(&traced_lit, width, height, x, y) || on_an_edge(&engine_lit, width, height, x, y) {
                verdict.edge += 1;
                continue;
            }
            verdict.interior += 1;
            if verdict.examples.len() < 8 {
                let seen = engine_model.pixels[pixel].seen.expect("compared, so it was seen");
                verdict.examples.push(format!(
                    "  [pixel ({x}, {y})] {:?} at ({:.3}, {:.3}, {:.3} tiles): the tracer says {}, the \
                     frame says {}",
                    seen.surface,
                    seen.at.x,
                    seen.at.y,
                    seen.at.z,
                    if traced { "lit" } else { "shadowed" },
                    if engine { "lit" } else { "shadowed" },
                ));
            }
        }
    }

    assert!(
        verdict.compared > 1000,
        "the path tracer compared only {} pixels against the frame — a detector that compares nothing \
         reads exactly like a detector that found nothing",
        verdict.compared
    );
    verdict.engine_lit = engine_lit;
    verdict.traced_lit = traced_lit;
    verdict
}
