//! What every scene tool in this directory has to do the same way to be
//! comparable: read the renderer's own answer about a pixel back off the GPU,
//! decode the one debug view an oracle can be checked against, and ask an
//! independent question about a segment and a box.
//!
//! Not a library — a module the scene tools share (`mod oracle;`), because the
//! alternative is two copies of a slab test. Two copies of a *geometric
//! primitive* is the shape that drifts: both look right, both are read rather
//! than run, and the day one of them grows a tolerance the other does not, two
//! oracles disagree about a scene neither of them is wrong about.
//!
//! It cannot be a library and it is not one by oversight: [`pathtrace`] uses
//! `openshard-client-pathtrace`, which is a **dev-dependency** of this crate
//! precisely so the reference cannot be reached from the shipped renderer. Code
//! that names it can therefore live in `examples/` or in `tests/` and nowhere
//! else — so `tests/traced.rs` reaches this module by `#[path]` rather than by
//! a second copy. See that file's own doc for the argument.
//!
//! Everything here is deliberately free of `light.rs` and `blit.wesl`
//! arithmetic. That is the whole claim an oracle makes — reusing the thing
//! under test to check the thing under test proves the two agree with each
//! other and nothing else — so the only engine code this module touches is the
//! G-buffer's own *format* (`crate::gbuffer`), which is a wire, not an answer.

pub mod boxes;
pub mod pathtrace;

/// One pixel of the G-buffer, decoded: **who drew this pixel, and where that
/// fragment is**.
///
/// The renderer's own answer to a question every oracle here has to ask and
/// used to answer by reconstructing the picture on the CPU — which surface owns
/// the pixel a world point projects to. A sample point is a point of a
/// *surface*, and the pixel under it belongs to whatever the depth test left
/// there: the ground behind a box, a nearer box's face, a box's own top.
/// Comparing a rendered pixel that some other surface drew against an oracle's
/// answer about this one is not a measurement of anything.
///
/// See [`openshard_client_render::gbuffer`] for both formats.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Drawn {
    /// [`openshard_client_render::place::Kind`]'s own two bits.
    pub kind: u32,
    /// The stance, which for a mesh face is the
    /// [`Stance::MeshFace`](openshard_client_render::place::Stance::MeshFace)
    /// routing sentinel rather than the face's real one.
    pub stance: u32,
    /// The instance row this fragment's picture came from — a `MeshFaceRow` for
    /// a mesh face, a ground quad for land.
    pub id: u32,
    /// And **where in the world the fragment itself is**, which is not the world
    /// point that projected onto it: the pixel's own fragment sits at the
    /// pixel's centre. That is the point the shader lit, so it is the point an
    /// oracle has to ask about; anything else compares the picture against a
    /// fragment the rasteriser could not produce.
    ///
    /// The number itself, off the position plane. This used to be a tile-local
    /// fraction in hundred-and-twenty-sevenths and a height in sixteenths, which
    /// every caller then added to a tile it looked up — three fields, one of
    /// them quantised twice on the way. `docs/lighting_rebuild.md` phase 2.
    pub at: (f64, f64, f64),
}

/// The G-buffer read back, one [`Drawn`] a pixel, row-major.
///
/// Two planes and one struct, because no caller has ever wanted one without the
/// other: what drew a pixel and where that fragment is are the two halves of
/// "which surface owns this pixel", and reading them apart is two readbacks a
/// caller has to keep in step by hand. `crate::gbuffer`'s planes ask for
/// `COPY_SRC` exactly so this is possible, and their own docs say so.
pub fn read_gbuffer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    gbuffer: &openshard_client_render::gbuffer::Gbuffer,
    width: u32,
    height: u32,
) -> Vec<Drawn> {
    let ids = read_plane(device, queue, gbuffer.ids(), width, height, 4);
    let positions = read_plane(device, queue, gbuffer.position(), width, height, 16);
    ids.as_chunks::<4>()
        .0
        .iter()
        .zip(positions.as_chunks::<16>().0)
        .map(|(word, point)| {
            let word = u32::from_le_bytes([word[0], word[1], word[2], word[3]]);
            let axis = |i: usize| {
                f64::from(f32::from_le_bytes([
                    point[i * 4],
                    point[i * 4 + 1],
                    point[i * 4 + 2],
                    point[i * 4 + 3],
                ]))
            };
            Drawn {
                // Through `gbuffer`'s own three readers rather than three shifts
                // spelled here: the layout is a contract with
                // `place_format.wesl` and an oracle restating it is a fourth
                // copy of it, in the one file whose whole claim is that it
                // shares no arithmetic with what it measures. A *format* is a
                // wire and reading it through the engine's own accessor is what
                // keeps this module honest about the difference.
                kind: word & 3,
                stance: openshard_client_render::gbuffer::ids_stance(word),
                id: openshard_client_render::gbuffer::ids_id(word),
                at: (axis(0), axis(1), axis(2)),
            }
        })
        .collect()
}

/// What the ground reflects, in linear units, **read off the frame the world
/// passes drew**.
///
/// The albedo half of a shaded comparison, and it lives here because both
/// callers of the reference tracer need exactly it: the tool that draws the two
/// pictures and the gate that asserts on them. Written twice, it would be two
/// decodes of one art — and the whole claim of a brightness comparison is that
/// the colour is *the same on both sides*, which two spellings cannot make true.
///
/// Not the land art looked up a second time: what is wanted is what the lighting
/// pass will multiply, which is what the world texture holds at a pixel the id
/// plane says is land. Taking it from the picture makes "the same albedo" a
/// measurement rather than two authors agreeing to write the same constant down.
///
/// # Panics
///
/// If the ground is not one flat colour, or if there is none. Both are the same
/// failure of a scene to be what a brightness comparison needs: the reference
/// has a single albedo for its whole ground plane, so a textured floor would be
/// compared against an average of itself and every pixel would differ by the
/// texture rather than by the light.
pub fn ground_albedo(drawn: &[Drawn], world: &[u8]) -> [f64; 3] {
    let land = openshard_client_render::place::Kind::Land as u32;
    let mut found: Option<[u8; 3]> = None;
    for (pixel, texel) in drawn.iter().enumerate() {
        if texel.kind != land {
            continue;
        }
        let art = [world[pixel * 4], world[pixel * 4 + 1], world[pixel * 4 + 2]];
        assert_eq!(
            *found.get_or_insert(art),
            art,
            "the ground is not one flat colour — pixel {pixel} is {art:?}, and a reference with a \
             single ground albedo cannot be laid beside a textured floor"
        );
    }
    let stored = found.expect("the frame drew no ground at all, so there is nothing to compare");
    stored.map(|channel| {
        f64::from(openshard_client_render::tonemap::srgb_to_linear(
            f32::from(channel) / 255.0,
        ))
    })
}

/// What a box's faces reflect, in linear units, **read off the frame the mesh
/// pass drew** — [`ground_albedo`]'s own argument, for the other surface a
/// shaded comparison needs.
///
/// `docs/lighting_rebuild.md` phase 6d gave the mesh pass a colour target;
/// before it, `oracle::pathtrace::Albedos::body` was a stand-in because there
/// was nothing on the engine's side to measure. A mesh face's own routing
/// sentinel (`Stance::MeshFace`) is what separates it from a ground pixel here,
/// the same way [`Drawn::stance`]'s own doc says a reader has to.
///
/// # Panics
///
/// If the frame drew no box face at all, or if the faces it drew are not one
/// flat colour — a scene with boxes must give every one of them the same
/// albedo, or a comparison against one reference number is comparing an
/// average of several to a number that was never any of them.
pub fn body_albedo(drawn: &[Drawn], world: &[u8]) -> [f64; 3] {
    let static_kind = openshard_client_render::place::Kind::Static as u32;
    let mesh_face = openshard_client_render::place::Stance::MeshFace as u32;
    let mut found: Option<[u8; 3]> = None;
    for (pixel, texel) in drawn.iter().enumerate() {
        if texel.kind != static_kind || texel.stance != mesh_face {
            continue;
        }
        let art = [world[pixel * 4], world[pixel * 4 + 1], world[pixel * 4 + 2]];
        assert_eq!(
            *found.get_or_insert(art),
            art,
            "a box's faces are not one flat colour — pixel {pixel} is {art:?}, and a reference with a \
             single body albedo cannot be laid beside a scene that gives its boxes more than one"
        );
    }
    let stored = found.expect("the frame drew no box face at all, so there is nothing to compare");
    stored.map(|channel| {
        f64::from(openshard_client_render::tonemap::srgb_to_linear(
            f32::from(channel) / 255.0,
        ))
    })
}

/// One plane copied back to the CPU, whole, at `stride` bytes a texel.
///
/// The row pitch is rounded up to `COPY_BYTES_PER_ROW_ALIGNMENT` and the
/// padding cut off again at the end, rather than assuming a width that happens
/// to divide it. The id plane is four bytes a texel where the `place`
/// attachment it replaced was eight, so every width that used to be aligned
/// stopped being — a scene 32 pixels across was exactly on the boundary and is
/// now half of one, which is a validation error naming a buffer layout and not
/// the plane that shrank.
fn read_plane(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
    stride: u32,
) -> Vec<u8> {
    let row = width * stride;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded = row.div_ceil(align) * align;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("g-buffer readback"),
        size: u64::from(padded) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |result| {
        result.expect("mapping a buffer this example just wrote")
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("waiting on our own submission");
    let bytes = slice.get_mapped_range().expect("the map completed above");
    bytes
        .chunks_exact(padded as usize)
        .flat_map(|padded_row| &padded_row[..row as usize])
        .copied()
        .collect()
}

/// One pixel of a rendered [`View::Shadow`](openshard_client_render::debug::View::Shadow)
/// frame, decoded into the **three** answers `blit.wesl` writes there.
///
/// A threshold on the red channel collapses the first two, and they are
/// opposite facts: a fragment no flame reaches is a statement about the
/// *radius*, and a fragment a flame reaches and the walk stopped is a statement
/// about the *geometry*. Only the second is a thing a visibility oracle has an
/// opinion about — measuring the first against one counts a torch's own range
/// as a disagreement — which is why the shader spends a colour telling them
/// apart (`blit.wesl`'s own comment at `VIEW_SHADOW`) and why reading it back
/// as a number throws that away.
///
/// The caller filters `Kind::Nothing` pixels out first: the shader returns the
/// cleared background for those before the view is ever asked, so they are not
/// one of these three.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Shade {
    /// Blue, `(0, 0, 0.35)`: no flame's radius covers this fragment. The walk
    /// never ran, so nothing here is about occlusion.
    Unreached,
    /// Dark red, `(0.2, 0, 0)`: a flame reaches and the walk let through at most
    /// `RAY_CUTOFF` of it. A hard stop, said in a colour rather than in black,
    /// so it cannot read as the background.
    Blocked,
    /// Grey: how much of the nearest reaching flame survived, as the channel the
    /// shader wrote it in.
    Through(u8),
}

impl Shade {
    /// Decode one pixel's `rgb`.
    pub fn of(rgb: [u8; 3]) -> Self {
        match (rgb[0], rgb[1], rgb[2]) {
            (0, 0, b) if b > 0 => Self::Unreached,
            (_, 0, 0) => Self::Blocked,
            (r, _, _) => Self::Through(r),
        }
    }

    /// Whether the picture calls this fragment lit — the same half-channel test
    /// every reader of this frame has used, stated once.
    ///
    /// [`Shade::Unreached`] is `false` here because a fragment outside every
    /// pool *is* dark; a caller that must not count it as a disagreement has to
    /// match on the variant instead, which is the point of there being one.
    pub fn lit(self) -> bool {
        matches!(self, Self::Through(value) if value > 128)
    }
}

/// A point-light-vs-axis-aligned-box visibility test, the textbook slab method,
/// written fresh here rather than calling the engine's own private
/// `light::ray_vs_solid` — reusing the thing under test to check the thing under
/// test would only prove the two agree with each other, not that either is
/// right. `true` when the open segment `from..to` (both ends excluded by a small
/// margin, so touching the box's own surface at either end does not count as a
/// hit) never enters `min..max`.
///
/// **Degenerate boxes are the ordinary case here, not an edge case.** A tread's
/// top is a plane (`min.z == max.z`) and a riser is a plane on the climb axis,
/// which is what `occlusion::Builder::add` pushes for a climbable static. The
/// slab arithmetic already answers for them where the ray meets the plane at an
/// angle: a plane's two slab bounds coincide, so `t0 == t1` at the crossing and
/// the interval is empty only if that crossing falls outside the segment or
/// outside another axis's span.
///
/// **A ray running *along* a plane is the case it does not answer for**, and it
/// is not exotic: a flame at exactly a tread's own height puts every ray from
/// that tread level, in the plane of every other tread at the same height. The
/// slab loop's parallel-axis arm asks "is the origin inside this pair of
/// bounds", which for a plane is "is the ray in it" — and answers `false`,
/// leaving the two remaining axes to decide, so a ray that travels the whole
/// length of a plane without ever passing through it is reported as blocked by
/// it. A plane is **crossed**, never travelled through; `light.rs`'s `crosses`
/// is strict for the same reason and says so in the same words. On a box with
/// extent on every axis this arm is untouched — being inside a real pair of
/// bounds is not a crossing question at all — so this is a statement about
/// degenerate boxes only.
pub fn segment_clear_of_box(
    from: (f64, f64, f64),
    to: (f64, f64, f64),
    min: (f64, f64, f64),
    max: (f64, f64, f64),
) -> bool {
    let d = (to.0 - from.0, to.1 - from.1, to.2 - from.2);
    let (mut t0, mut t1): (f64, f64) = (1e-4, 1.0 - 1e-4);
    for (o, d, lo, hi) in [
        (from.0, d.0, min.0, max.0),
        (from.1, d.1, min.1, max.1),
        (from.2, d.2, min.2, max.2),
    ] {
        if d.abs() < 1e-12 {
            if o < lo || o > hi || lo == hi {
                return true;
            }
            continue;
        }
        let (mut near, mut far) = ((lo - o) / d, (hi - o) / d);
        if near > far {
            std::mem::swap(&mut near, &mut far);
        }
        t0 = t0.max(near);
        t1 = t1.min(far);
        if t0 > t1 {
            return true;
        }
    }
    false
}

/// A lime crosshair on a black backing plate, marking where the flame itself
/// projects to.
///
/// A number in a log line does not answer "is the light behind the stair or in
/// front of it" nearly as fast as a mark on the frame does. Thick enough (a
/// 41-pixel arm on a 512-pixel frame) to find at a glance, and the black backing
/// is what keeps it visible crossing an already-white pixel.
fn mark_crosshair(pixels: &mut [u8], width: u32, height: u32, at: (i32, i32)) {
    let mut mark = |dx: i32, dy: i32, colour: [u8; 3]| {
        let (px, py) = (at.0 + dx, at.1 + dy);
        if px < 0 || py < 0 || px as u32 >= width || py as u32 >= height {
            return;
        }
        let offset = ((py as u32 * width + px as u32) * 4) as usize;
        pixels[offset] = colour[0];
        pixels[offset + 1] = colour[1];
        pixels[offset + 2] = colour[2];
        pixels[offset + 3] = 255;
    };
    for dy in -20..=20i32 {
        for dx in -20..=20i32 {
            if dx.abs() > 2 && dy.abs() > 2 {
                continue;
            }
            mark(dx, dy, [0, 0, 0]);
        }
    }
    for dy in -18..=18i32 {
        for dx in -18..=18i32 {
            if dx.abs() > 1 && dy.abs() > 1 {
                continue;
            }
            mark(dx, dy, [80, 255, 0]);
        }
    }
}

/// One rendered frame's pixels, `RGBA8`, row-major.
///
/// Split out of [`dump`] because the gate under `tests/` wants the frame and
/// not a file: a test that writes a picture every green run is a test that
/// fills a directory to say nothing.
pub fn read_surface(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    surface: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Vec<u8> {
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(width) * u64::from(height) * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: surface,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |result| {
        result.expect("mapping a buffer this example just wrote")
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("waiting on our own submission");
    slice
        .get_mapped_range()
        .expect("the map completed above")
        .to_vec()
}

/// [`read_surface`], marked with a crosshair and written out as a PNG.
///
/// Returns the *unmarked* pixels: the crosshair is for a person looking at the
/// picture, and an oracle reading the mark back as a lit pixel would be an
/// oracle measuring its own annotation.
pub fn dump(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    surface: &wgpu::Texture,
    width: u32,
    height: u32,
    path: &std::path::Path,
    mark: Option<(i32, i32)>,
) -> Vec<u8> {
    let pixels = read_surface(device, queue, surface, width, height);
    let mut marked = pixels.clone();
    if let Some(at) = mark {
        mark_crosshair(&mut marked, width, height, at);
    }
    std::fs::write(
        path,
        openshard_client_render::png::encode_rgba(width, height, &marked),
    )
    .expect("writing the frame");
    eprintln!("wrote {}", path.display());
    pixels
}
