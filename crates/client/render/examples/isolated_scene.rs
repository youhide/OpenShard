//! A picture of one real place, and only what the caller asks to keep in it.
//!
//! The tool `docs/lighting.md`'s backlog wanted while chasing a corner where a
//! staircase's occlusion box met a lamp's soft pool: `OPENSHARD_FRAME_AT`
//! (`tests/cost.rs`) repoints the camera at a real place but still draws the
//! whole neighbourhood around it, and a house standing beside the thing under
//! test is a second variable in every picture. This draws a **synthetic**
//! [`openshard_uofiles::map::Map`] instead — `Map::from_blocks` never carries
//! statics (see `crate::scene`'s own doc) — and puts back only what is asked
//! for: the real map's statics within a stated radius of a stated point,
//! optionally filtered to a list of tile IDs, the real ground under them or
//! none at all, and any hand-named extra items (a live-shard decoration from
//! `openshard.db`, say — that table is not something this reads itself; see
//! `docs/lighting.md`'s DB-lookup recipe for pulling one out by hand). Turn
//! every knob down and what is left is one tile.
//!
//! Real coordinates translate onto a fixed synthetic anchor
//! ([`SYN_ANCHOR`]) the same way `crate::scene::CENTRE` does it for the
//! built-in scenes — far from the synthetic map's own edge, so a camera never
//! runs off it.
//!
//! # Knobs, all environment variables so no edit-run-revert is ever needed
//!
//! - `OPENSHARD_CLIENT` — the client's files. Required, like every tool here.
//! - `OPENSHARD_SCENE_AT=x,y,z` — the real place. Required: it is both the
//!   centre of the pulled radius and, unless `OPENSHARD_SCENE_LOOK` says
//!   otherwise, where the camera looks.
//! - `OPENSHARD_SCENE_LOOK=x,y,z` — where the camera looks, if not `_AT`.
//! - `OPENSHARD_SCENE_RADIUS=n` — pull real statics from the `(2n+1)^2` tiles
//!   around `_AT`. `0` (the default) is the one tile `_AT` stands on.
//! - `OPENSHARD_SCENE_STATICS=0` — skip the real map's statics entirely
//!   (default: pull them).
//! - `OPENSHARD_SCENE_TILES=0x0739,0x0738` — keep only these tile IDs among
//!   whatever the radius pulled. Unset keeps everything pulled.
//! - `OPENSHARD_SCENE_GROUND=0` — draw no land at all: an empty atlas, so the
//!   ground pass still runs (it always clears) but paints nothing. Default is
//!   the real land, read live off the same facet.
//! - `OPENSHARD_SCENE_EXTRA=x,y,z,graphic[,hue];...` — items to add by hand,
//!   semicolon-separated. What a DB-pulled decoration (or anything else not on
//!   the map) comes in as.
//! - `OPENSHARD_SCENE_VIEWPORT=960x720` — must keep `width * 4` a multiple of
//!   256 (`wgpu`'s row-copy alignment) or the readback panics; the default is
//!   already aligned.
//! - `OPENSHARD_SCENE_ZOOM=n` — `n` notches of [`Zoom::scale_up`] past
//!   [`Zoom::ONE`], the live client's own wheel-in ladder (`2:1`, `3:1`,
//!   `4:1`, nearest-sampled so a texel still lands on a whole number of real
//!   pixels — see [`Zoom`]'s own doc for why that matters). Default `0`
//!   (`Zoom::ONE`, one world pixel to one viewport pixel). A crop blown up
//!   afterwards with an image tool is a *different* resample — bilinear or
//!   nearest, neither the client's own rule — and can show banding neither
//!   this tool nor the client actually draws; this is the honest way to look
//!   closer.
//! - `OPENSHARD_FRAME_DUMP=/tmp/x.png` — where to write the picture. Required;
//!   this tool has nothing else to do with a frame once it is drawn.
//! - `OPENSHARD_FRAME_VIEW=n` — index into `debug::View::ALL` (`0` `Lit`, `4`
//!   `Occluders`, `5` `Light`, …). Default `Lit`. Ignored under `_SOLIDS`.
//!
//! # The occlusion grid alone, nothing drawn to cast a shadow of its own
//!
//! `OPENSHARD_SCENE_SOLIDS=white|lit` skips every sprite and every blade of
//! ground and draws only [`solid::standing`]'s boxes on black — the same pass
//! (`solids::SolidsRenderer`) the live client's own F5 overlay uses. Ground and
//! sprites are still built and drawn to their own offscreen textures first —
//! `_GROUND` and `_TILES` still apply to what is *collected into the grid* —
//! they are simply never composited into the picture this mode reads back.
//!
//! - `white` — one flat colour, so the geometry reads on its own with nothing
//!   about the flame in it.
//! - `lit` — each visible face coloured by one real [`light::sample`] taken at
//!   its own centre (the lid at `Surface::Flat`, the two sides at their own
//!   `Surface::Face`) — the flame's own light on the grid's own boxes, with no
//!   sprite's shading to tell it apart from.
//!
//! `OPENSHARD_SCENE_SOLIDS_EDGES=0` drops the black outline stroke, leaving the
//! translucent fills alone — see `SolidsRenderer::render`'s own doc for why
//! that is what shows a face wrongly surviving behind one that should have hidden
//! it. Default `1`, matching the live F5 overlay.
//!
//! `OPENSHARD_SCENE_SOLIDS_OPAQUE=1` swaps translucent fills for a straight
//! overwrite: a later, nearer face then genuinely hides an earlier, farther one
//! (painter's algorithm, on `solid::standing`'s own back-to-front order) rather
//! than tinting through it. Default `0`, matching the live F5 overlay — turn it
//! on to tell "hidden behind something nearer" from "showing through it".
//!
//! # Profiling a segment instead of drawing it
//!
//! Setting `OPENSHARD_SCENE_PROFILE_FACE` switches the tool from a picture to a
//! printed table and skips every GPU pass after the scene's lighting is built —
//! `OPENSHARD_FRAME_DUMP`/`_VIEW` are ignored in this mode. For a question a
//! picture cannot answer on its own: whether a hard edge in the render is the
//! occlusion walk (`Reach::through`) or the face's `faces()` cutoff folded into
//! `Reach::cone` (see `light.rs`'s own doc on the two).
//!
//! - `OPENSHARD_SCENE_PROFILE_FACE=north|east|south|west|flat|upright` — which
//!   [`light::Surface`] to sample. The four faces are `Spot::face(...)`'s
//!   `Face`. Required to enter profile mode.
//! - `OPENSHARD_SCENE_PROFILE_FROM=x,y,z` / `_TO=x,y,z` — the segment to walk,
//!   in real (fractional allowed) map coordinates.
//! - `OPENSHARD_SCENE_PROFILE_STEPS=n` — how many samples along it, both ends
//!   inclusive. Default `40`.
//! - `OPENSHARD_SCENE_PROFILE_LIGHT=n` — print only [`light::Reach`]s for this
//!   light index. Default: every light the scene collected.
//!
//! ```sh
//! OPENSHARD_CLIENT=… \
//!     OPENSHARD_SCENE_AT=1497,1626,10 OPENSHARD_SCENE_TILES=0x0739,0x0738 \
//!     OPENSHARD_SCENE_GROUND=0 OPENSHARD_SCENE_EXTRA=1498,1626,10,2852 \
//!     OPENSHARD_SCENE_PROFILE_FACE=south \
//!     OPENSHARD_SCENE_PROFILE_FROM=1497.0,1627.0,10 OPENSHARD_SCENE_PROFILE_TO=1497.0,1627.0,16 \
//!     cargo run --release -p openshard-client-render --example isolated_scene
//! ```
//!
//! # Example: the one tile that made the corner in the user's screenshot
//!
//! ```sh
//! OPENSHARD_CLIENT=… \
//!     OPENSHARD_SCENE_AT=1497,1626,10 OPENSHARD_SCENE_TILES=0x0739,0x0738 \
//!     OPENSHARD_SCENE_GROUND=0 \
//!     OPENSHARD_SCENE_EXTRA=1498,1626,10,2852 \
//!     OPENSHARD_FRAME_DUMP=/tmp/corner.png OPENSHARD_FRAME_VIEW=5 \
//!     cargo run --release -p openshard-client-render --example isolated_scene
//! ```

use std::path::PathBuf;

use openshard_client_render::animate::StaticAnimations;
use openshard_client_render::atlas::{LandAtlas, StaticAtlas, TexmapAtlas};
use openshard_client_render::blit::{Blit, ViewportRect};
use openshard_client_render::camera::{Camera, Zoom};
use openshard_client_render::cutaway::Cutaway;
use openshard_client_render::geometry::Vec2;
use openshard_client_render::ground;
use openshard_client_render::items::{self, GroundItem};
use openshard_client_render::renderer::{GroundRenderer, MeshFaceRenderer, SpriteRenderer, Target};
use openshard_client_render::{light, renderer};
use openshard_protocol::wire::{Graphic, Hue};
use openshard_protocol::world::Point;
use openshard_uofiles::art::Art;
use openshard_uofiles::map::{LandCell, Map};
use openshard_uofiles::texmaps::TexMaps;
use openshard_uofiles::tiledata::TileData;

/// Where every scene's real anchor lands in the synthetic map — far from its
/// edge, the same convention `crate::scene::CENTRE` uses.
const SYN_ANCHOR: (u16, u16) = (100, 100);

/// A required environment variable, or a clear panic naming which one.
fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is required"))
}

fn env_opt(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

fn env_flag(name: &str, default: bool) -> bool {
    match env_opt(name).as_deref() {
        None => default,
        Some("0") => false,
        Some("1") => true,
        Some(other) => panic!("{name} wants `0` or `1`, got {other:?}"),
    }
}

/// `x,y,z`, the same shape `tests/cost.rs`'s `OPENSHARD_FRAME_AT` reads.
fn parse_point(spec: &str) -> Point {
    let coords: Vec<&str> = spec.split(',').collect();
    let [x, y, z] = coords[..] else {
        panic!("wanted `x,y,z`, got {spec:?}");
    };
    Point::new(
        x.trim().parse().unwrap_or_else(|_| panic!("x: {x:?}")),
        y.trim().parse().unwrap_or_else(|_| panic!("y: {y:?}")),
        z.trim().parse().unwrap_or_else(|_| panic!("z: {z:?}")),
    )
}

/// A tile ID, decimal or `0x`-prefixed hex — the two shapes `tiledata` dumps
/// and Sphere-flavoured docs both use.
fn parse_tile_id(spec: &str) -> u16 {
    let spec = spec.trim();
    match spec.strip_prefix("0x").or_else(|| spec.strip_prefix("0X")) {
        Some(hex) => u16::from_str_radix(hex, 16).unwrap_or_else(|_| panic!("tile id: {spec:?}")),
        None => spec.parse().unwrap_or_else(|_| panic!("tile id: {spec:?}")),
    }
}

/// `x,y,z,graphic[,hue]`, one item.
fn parse_extra_item(spec: &str) -> GroundItem {
    let parts: Vec<&str> = spec.split(',').collect();
    let (x, y, z, graphic, hue) = match parts[..] {
        [x, y, z, graphic] => (x, y, z, graphic, "0"),
        [x, y, z, graphic, hue] => (x, y, z, graphic, hue),
        _ => panic!("OPENSHARD_SCENE_EXTRA item wants `x,y,z,graphic[,hue]`, got {spec:?}"),
    };
    GroundItem {
        at: Point::new(
            x.trim().parse().unwrap_or_else(|_| panic!("x: {x:?}")),
            y.trim().parse().unwrap_or_else(|_| panic!("y: {y:?}")),
            z.trim().parse().unwrap_or_else(|_| panic!("z: {z:?}")),
        ),
        graphic: Graphic(parse_tile_id(graphic)),
        hue: Hue(hue.trim().parse().unwrap_or_else(|_| panic!("hue: {hue:?}"))),
    }
}

/// `x,y,z`, all three fractional — [`parse_point`]'s shape, but for a spot
/// [`run_profile`] wants to place anywhere inside a tile rather than on one.
fn parse_fpoint(spec: &str) -> (f32, f32, f32) {
    let coords: Vec<&str> = spec.split(',').collect();
    let [x, y, z] = coords[..] else {
        panic!("wanted `x,y,z`, got {spec:?}");
    };
    (
        x.trim().parse().unwrap_or_else(|_| panic!("x: {x:?}")),
        y.trim().parse().unwrap_or_else(|_| panic!("y: {y:?}")),
        z.trim().parse().unwrap_or_else(|_| panic!("z: {z:?}")),
    )
}

/// `OPENSHARD_SCENE_PROFILE_FACE`'s value, which names a whole [`light::Surface`]
/// and not just a [`Face`](openshard_client_render::facing::Face) — `flat` and
/// `upright` are the other two the place attachment can carry.
fn parse_surface(spec: &str) -> light::Surface {
    use openshard_client_render::facing::Face;
    match spec.trim().to_ascii_lowercase().as_str() {
        "north" => light::Surface::Face(Face::North),
        "east" => light::Surface::Face(Face::East),
        "south" => light::Surface::Face(Face::South),
        "west" => light::Surface::Face(Face::West),
        "flat" => light::Surface::Flat,
        "upright" => light::Surface::Upright,
        _ => panic!("OPENSHARD_SCENE_PROFILE_FACE wants north/east/south/west/flat/upright, got {spec:?}"),
    }
}

/// [`shift`], but for the fractional coordinates [`run_profile`] samples at
/// rather than the whole tiles the rest of this file moves onto the synthetic
/// map.
fn shift_f(anchor: (u16, u16), x: f32, y: f32) -> (f32, f32) {
    (
        x - f32::from(anchor.0) + SYN_ANCHOR.0 as f32,
        y - f32::from(anchor.1) + SYN_ANCHOR.1 as f32,
    )
}

/// The profile mode: walk [`Spot::face`](light::Spot::face) along a segment and
/// print what [`light::sample`] says at each step, instead of drawing a frame.
///
/// `through` and `cone` are read straight off [`light::Reach`] rather than
/// re-derived here, which is the whole point — they are the production
/// function's own account of "was something in the way" and "was the surface
/// turned towards the flame", already kept apart for exactly this question. For
/// a scene with one point light (no [`crate::light::Beam`]), `cone` *is* the
/// `faces()` term alone.
fn run_profile(anchor: (u16, u16), lighting: &light::Lighting) {
    let face_spec = env("OPENSHARD_SCENE_PROFILE_FACE");
    let surface = parse_surface(&face_spec);
    let (fx, fy, fz) = parse_fpoint(&env("OPENSHARD_SCENE_PROFILE_FROM"));
    let (tx, ty, tz) = parse_fpoint(&env("OPENSHARD_SCENE_PROFILE_TO"));
    let steps: u32 = env_opt("OPENSHARD_SCENE_PROFILE_STEPS")
        .map(|s| {
            s.parse()
                .unwrap_or_else(|_| panic!("OPENSHARD_SCENE_PROFILE_STEPS: {s:?}"))
        })
        .unwrap_or(40);
    let only_light: Option<usize> = env_opt("OPENSHARD_SCENE_PROFILE_LIGHT").map(|s| {
        s.parse()
            .unwrap_or_else(|_| panic!("OPENSHARD_SCENE_PROFILE_LIGHT: {s:?}"))
    });

    println!("profile: surface {face_spec}, {steps} steps from ({fx},{fy},{fz}) to ({tx},{ty},{tz})");
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let (x, y, z) = (fx + (tx - fx) * t, fy + (ty - fy) * t, fz + (tz - fz) * t);
        let (sx, sy) = shift_f(anchor, x, y);
        let spot = light::Spot {
            at: Vec2::new(sx, sy),
            z,
            // A point in the world rather than a fragment of one of its statics:
            // this tool walks a segment through the scene, so it is a point of no
            // occluder and exempt from none. See `light::Spot::owner`.
            owner: openshard_client_render::occlusion::OwnerId::NONE,
            // `floor()`, deliberately: this tool exists to bisect exactly the
            // ambiguity `Spot::tile` closes for a real caller
            // (`docs/lighting_raymarch.md` step 2), so the profile has to keep
            // showing what a naively-derived tile does at a boundary sample,
            // not paper over it with a guessed intent.
            tile: (sx.floor() as i32, sy.floor() as i32),
            surface,
        };
        let sample = light::sample(spot, lighting);
        print!(
            "t={t:.3} ({x:.2}, {y:.2}, z {z:.1}) -> brightness {:.3}",
            sample.brightness()
        );
        for reach in &sample.reaches {
            if only_light.is_some_and(|want| want != reach.light) {
                continue;
            }
            match (reach.within, reach.stopped_by) {
                (false, _) => print!("  | light {}: outside radius", reach.light),
                (true, Some(stopper)) => print!("  | light {}: stopped by {stopper}", reach.light),
                (true, None) => print!(
                    "  | light {}: through {:.3} cone {:.3}",
                    reach.light, reach.through, reach.cone
                ),
            }
        }
        println!();
    }
}

/// One real [`light::sample`] per *corner* of every visible face of a drawn
/// solid — `[Top, East, South]`, [`openshard_client_render::solid::Solid::faces`]'s
/// own order, and within each a
/// [`solids::FaceColours`](openshard_client_render::solids::FaceColours) in
/// that same function's own four-corner ring order — as an RGB colour:
/// [`light::Sample::multiplier`] directly, clamped into `0..=255` rather than
/// reduced to [`light::Sample::brightness`]'s one number, so the flame's own
/// colour survives onto the box.
///
/// Four samples a face rather than one at its centre, so `SolidsRenderer`'s
/// ordinary per-vertex colour interpolation turns them into a gradient across
/// the fill instead of a flat tint with a step at every face's edge — see
/// `FaceColours`'s own doc for what this still is not: the real, non-linear
/// falloff, only four real points of it linearly blended.
fn face_light(
    solid: &openshard_client_render::solid::Solid,
    lighting: &light::Lighting,
) -> [openshard_client_render::solids::FaceColours; 3] {
    use openshard_client_render::facing::Face;
    use openshard_client_render::solids::FaceColours;

    let (lo, hi) = (solid.min, solid.max);
    let colour = |x: f64, y: f64, z: f64, surface: light::Surface| {
        let spot = light::Spot {
            at: Vec2::new(x as f32, y as f32),
            z: z as f32,
            tile: (x.floor() as i32, y.floor() as i32),
            surface,
            // The wireframe's own face colours: a probe of a *place*, not of a
            // drawn fragment, so it names no owner and is exempt from nothing —
            // which is what makes the picture show every solid's real effect,
            // including the one being coloured.
            owner: openshard_client_render::occlusion::OwnerId::NONE,
        };
        let multiplier = light::sample(spot, lighting).multiplier;
        [
            multiplier[0].clamp(0.0, 1.0) * 255.0,
            multiplier[1].clamp(0.0, 1.0) * 255.0,
            multiplier[2].clamp(0.0, 1.0) * 255.0,
        ]
    };
    // The same four corners `Solid::faces` hands `SolidsRenderer`, in the same
    // order, so corner `i`'s sample here is corner `i`'s vertex there.
    let top: FaceColours = [
        colour(lo.x, lo.y, hi.z, light::Surface::Flat),
        colour(hi.x, lo.y, hi.z, light::Surface::Flat),
        colour(hi.x, hi.y, hi.z, light::Surface::Flat),
        colour(lo.x, hi.y, hi.z, light::Surface::Flat),
    ];
    let east: FaceColours = [
        colour(hi.x, lo.y, hi.z, light::Surface::Face(Face::East)),
        colour(hi.x, hi.y, hi.z, light::Surface::Face(Face::East)),
        colour(hi.x, hi.y, lo.z, light::Surface::Face(Face::East)),
        colour(hi.x, lo.y, lo.z, light::Surface::Face(Face::East)),
    ];
    let south: FaceColours = [
        colour(lo.x, hi.y, hi.z, light::Surface::Face(Face::South)),
        colour(hi.x, hi.y, hi.z, light::Surface::Face(Face::South)),
        colour(hi.x, hi.y, lo.z, light::Surface::Face(Face::South)),
        colour(lo.x, hi.y, lo.z, light::Surface::Face(Face::South)),
    ];
    [top, east, south]
}

fn shift(anchor: (u16, u16), real: (u16, u16)) -> (u16, u16) {
    (
        (real.0 as i32 - anchor.0 as i32 + SYN_ANCHOR.0 as i32) as u16,
        (real.1 as i32 - anchor.1 as i32 + SYN_ANCHOR.1 as i32) as u16,
    )
}

fn unshift(anchor: (u16, u16), syn: (u16, u16)) -> (u16, u16) {
    (
        (syn.0 as i32 - SYN_ANCHOR.0 as i32 + anchor.0 as i32) as u16,
        (syn.1 as i32 - SYN_ANCHOR.1 as i32 + anchor.1 as i32) as u16,
    )
}

/// Reads `surface` back to PNG bytes — the one step every mode of this
/// tool ends in, whatever filled the texture before it was called.
fn dump_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    surface: &wgpu::Texture,
    viewport: (u32, u32),
) -> Vec<u8> {
    let (w, h) = viewport;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(w) * u64::from(h) * 4,
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
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |result| {
        result.expect("mapping a buffer this example just wrote");
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("waiting on our own submission");
    let pixels = slice
        .get_mapped_range()
        .expect("the map completed above")
        .to_vec();

    openshard_client_render::png::encode_rgba(w, h, &pixels)
}

fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()
}

/// `960x720`-shaped, defaulting to a size whose row is already 256-byte
/// aligned — `wgpu`'s `COPY_BYTES_PER_ROW_ALIGNMENT`, which the readback at
/// the bottom of this file needs and does not itself enforce.
fn viewport() -> (u32, u32) {
    let Some(spec) = env_opt("OPENSHARD_SCENE_VIEWPORT") else {
        return (960, 720);
    };
    let (w, h) = spec
        .split_once('x')
        .unwrap_or_else(|| panic!("OPENSHARD_SCENE_VIEWPORT: {spec:?}"));
    let (w, h): (u32, u32) = (
        w.trim().parse().unwrap_or_else(|_| panic!("width: {w:?}")),
        h.trim().parse().unwrap_or_else(|_| panic!("height: {h:?}")),
    );
    assert!(
        w * 4 % 256 == 0,
        "OPENSHARD_SCENE_VIEWPORT width {w} isn't 256-byte-row aligned (wants a multiple of 64)"
    );
    (w, h)
}

fn main() {
    let dir = PathBuf::from(env("OPENSHARD_CLIENT"));
    let (device, queue) = gpu().expect("an adapter");

    let real_map = Map::load_facet(&dir, 0).expect("Felucca");
    let art = Art::open(&dir).expect("artLegacyMUL.uop");
    let tiledata = TileData::load(dir.join("tiledata.mul")).expect("tiledata.mul");

    let at = parse_point(&env("OPENSHARD_SCENE_AT"));
    let anchor = (at.x, at.y);
    let look = env_opt("OPENSHARD_SCENE_LOOK")
        .map(|s| parse_point(&s))
        .unwrap_or(at);

    let radius: u16 = env_opt("OPENSHARD_SCENE_RADIUS")
        .map(|s| {
            s.parse()
                .unwrap_or_else(|_| panic!("OPENSHARD_SCENE_RADIUS: {s:?}"))
        })
        .unwrap_or(0);
    let want_statics = env_flag("OPENSHARD_SCENE_STATICS", true);
    let tile_filter: Option<Vec<u16>> =
        env_opt("OPENSHARD_SCENE_TILES").map(|s| s.split(',').map(parse_tile_id).collect());
    let want_ground = env_flag("OPENSHARD_SCENE_GROUND", true);

    // Land, borrowed live from the real facet at every synthetic cell's real
    // coordinate — or nothing, `_GROUND=0`'s whole point. `Map::from_blocks`
    // never carries statics regardless, so a house never comes along by
    // accident; keeping this a live read rather than a stored blob is what
    // lets `_RADIUS` and `_AT` move the scene to any corner of Britain with no
    // second file to keep in step.
    let synthetic = Map::from_blocks(16, 16, |sx, sy| {
        if !want_ground {
            return LandCell { tile: 0, z: at.z };
        }
        let (rx, ry) = unshift(anchor, (sx, sy));
        real_map.land(rx, ry).unwrap_or(LandCell { tile: 0, z: at.z })
    });

    // The real map's own statics within the radius, translated onto the
    // synthetic anchor and filtered to `_TILES` if it named any. This is what
    // makes the tool general: the caller does not hand-transcribe a tile's
    // graphic and `z` the way this file's first draft did — it is read here,
    // the same way `examples/dump_statics.rs` (this session's throwaway) read
    // it by hand.
    let mut items: Vec<GroundItem> = Vec::new();
    if want_statics {
        for x in at.x.saturating_sub(radius)..=at.x.saturating_add(radius) {
            for y in at.y.saturating_sub(radius)..=at.y.saturating_add(radius) {
                for s in real_map.statics_at(x, y) {
                    if tile_filter
                        .as_ref()
                        .is_some_and(|allowed| !allowed.contains(&s.tile))
                    {
                        continue;
                    }
                    let (sx, sy) = shift(anchor, (x, y));
                    items.push(GroundItem {
                        at: Point::new(sx, sy, s.z),
                        graphic: Graphic(s.tile),
                        hue: Hue(s.hue),
                    });
                }
            }
        }
    }

    // Hand-named extras — typically a live-shard decoration or dropped item
    // pulled from `openshard.db` by the recipe in `docs/lighting.md`, since
    // nothing on the map can see those and nothing here reads the DB itself.
    if let Some(spec) = env_opt("OPENSHARD_SCENE_EXTRA") {
        for entry in spec.split(';').filter(|s| !s.trim().is_empty()) {
            let mut item = parse_extra_item(entry);
            let (sx, sy) = shift(anchor, (item.at.x, item.at.y));
            item.at = Point::new(sx, sy, item.at.z);
            items.push(item);
        }
    }
    assert!(
        !items.is_empty(),
        "the scene is empty: turn on OPENSHARD_SCENE_STATICS, widen OPENSHARD_SCENE_RADIUS, \
         loosen OPENSHARD_SCENE_TILES, or add OPENSHARD_SCENE_EXTRA"
    );

    let viewport = viewport();
    let (look_sx, look_sy) = shift(anchor, (look.x, look.y));
    let mut camera = Camera::new(Point::new(look_sx, look_sy, look.z), viewport.0, viewport.1);
    let zoom_notches: u32 = env_opt("OPENSHARD_SCENE_ZOOM")
        .map(|s| {
            s.parse()
                .unwrap_or_else(|_| panic!("OPENSHARD_SCENE_ZOOM: {s:?}"))
        })
        .unwrap_or(0);
    let mut zoom = Zoom::ONE;
    for _ in 0..zoom_notches {
        zoom = zoom.scale_up();
    }
    // About the viewport's own centre, not a corner: `zoom_about` holds
    // whatever world point sits under the given screen point fixed, and the
    // scene is built around `look`, which `Camera::new` already centred.
    // Zooming about `(0, 0)` would hold the *corner* fixed instead and walk
    // the framed static out from under the crop this tool's callers take.
    camera.zoom_about((viewport.0 / 2) as i32, (viewport.1 / 2) as i32, zoom);
    let (width, height) = camera.image_size();

    let land_atlas = if want_ground {
        LandAtlas::build(
            &art,
            ground::visible_graphics(&synthetic, &camera).iter().copied(),
        )
        .expect("land fits")
    } else {
        LandAtlas::build(&art, std::iter::empty()).expect("an empty atlas always fits")
    };
    let texmaps = {
        let texmaps_src = TexMaps::open(&dir).expect("texidx.mul and texmaps.mul");
        let wanted = match want_ground {
            true => ground::visible_graphics(&synthetic, &camera),
            false => Default::default(),
        };
        TexmapAtlas::build(&texmaps_src, &tiledata, wanted).expect("textures fit")
    };
    let ground_quads = match want_ground {
        true => ground::collect(&synthetic, &camera, &land_atlas, &texmaps, &Cutaway::OPEN),
        false => Vec::new(),
    };

    let animations = StaticAnimations::default();
    let needed = items::needed_graphics(&items, &animations);
    let static_atlas = StaticAtlas::build(&art, needed).expect("the scene's own items fit");
    // **The grid before the pictures**, which is `docs/lighting_height.md` phase
    // 3's own ordering and the same one `app::render` keeps: what a drawn row
    // carries beside its picture is the number this grid gave the static it
    // draws, so a tool that collected its items first would stamp numbers from a
    // grid that did not exist yet.
    let mut lighting = light::collect(
        &synthetic,
        &items,
        &camera,
        &tiledata,
        &Cutaway::OPEN,
        light::NIGHT.flattened(),
        0.0,
        Some(&static_atlas),
        None,
    );
    let openshard_client_render::statics::StaticGeometry {
        quads: item_quads,
        mesh_vertices,
        mesh_rows,
    } = items::collect(
        &items,
        &camera,
        &tiledata,
        &animations,
        &static_atlas,
        &Cutaway::OPEN,
        None,
        &lighting.occlusion,
    );

    let format = openshard_client_render::blit::WORLD_FORMAT;
    let world = openshard_client_render::blit::world_texture(&device, width, height);
    let world_view = world.create_view(&wgpu::TextureViewDescriptor::default());
    let place = openshard_client_render::place::texture(&device, width, height);
    let place_view = place.create_view(&wgpu::TextureViewDescriptor::default());
    let depth = renderer::depth_texture(&device, width, height);
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());

    let mut ground_pass = GroundRenderer::new(&device, &queue, format, &land_atlas, &texmaps);
    let mut items_pass = SpriteRenderer::new(
        &device,
        &queue,
        format,
        static_atlas.pixels(),
        &openshard_client_render::hue::HueRamp::build(
            &openshard_uofiles::hues::Hues::load(dir.join("hues.mul")).expect("hues.mul"),
        ),
    );
    let mut mesh_pass = MeshFaceRenderer::new(&device);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let target = Target {
        place: &place_view,
        view: &world_view,
        depth: &depth_view,
        width,
        height,
        projection: camera.projection(),
    };
    // Always run, even with no ground quads: the ground pass is what clears
    // the world/place/depth targets — see its own doc — so skipping it here
    // would leave the items pass drawing over whatever the textures happened
    // to hold.
    ground_pass.render(&device, &queue, &mut encoder, target, &ground_quads);
    items_pass.render(&device, &queue, &mut encoder, target, &item_quads, None);
    // Right after, into the same pixels its own billboard just drew — the same
    // order `lib.rs`'s real frame loop uses and for the same reason
    // (`docs/gbuffer.md` step 4c): without this, a climbable, prism-fit item
    // pulled from the real map never gets its honest per-face normal, and
    // this tool would go on showing the flat corner-stance reading a live
    // frame does not.
    mesh_pass.render(&device, &queue, &mut encoder, target, &mesh_vertices, &mesh_rows);
    queue.submit([encoder.finish()]);

    let surface = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("surface"),
        size: wgpu::Extent3d {
            width: viewport.0,
            height: viewport.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let surface_view = surface.create_view(&wgpu::TextureViewDescriptor::default());

    eprintln!(
        "{} items, {} flames, {} standing cells",
        items.len(),
        lighting.lights.len(),
        lighting.occlusion.boxes().count(),
    );
    // Every solid `_AT`'s own tile holds, in real coordinates — the geometry a
    // profile below is walking across, printed once so the segment it is given
    // does not have to be guessed from the picture.
    let (syn_x, syn_y) = shift(anchor, (at.x, at.y));
    for solid in lighting.occlusion.solids_at(i32::from(syn_x), i32::from(syn_y)) {
        eprintln!(
            "  solid: x {:.3}..{:.3}, y {:.3}..{:.3}, z {:.1}..{:.1}, edges {:#06b}, opacity {}",
            solid.space.min.x,
            solid.space.max.x,
            solid.space.min.y,
            solid.space.max.y,
            solid.space.min.z,
            solid.space.max.z,
            solid.edges,
            solid.opacity,
        );
    }

    // A profile instead of a picture: `light::sample` walked along a segment
    // rather than `Blit` rasterising a frame, for the question a picture cannot
    // answer — whether a hard edge on screen is `Reach::through` (occlusion) or
    // `Reach::cone` (which one of a face's `faces()` and its beam folds into,
    // see `light.rs`) that is doing the cutting. No GPU work past this point.
    if env_opt("OPENSHARD_SCENE_PROFILE_FACE").is_some() {
        run_profile(anchor, &lighting);
        return;
    }

    // The grid alone: every solid `lighting.occlusion` holds, on black, with no
    // sprite and no ground under it to explain a shape away. The same list and
    // the same pass the live client's F5 overlay draws (`solid::standing` into
    // `solids::SolidsRenderer`) — only the colour changes, and nothing else is
    // composited under it, since a ray does not care which kind of thing it hit.
    //
    // - `OPENSHARD_SCENE_SOLIDS=white` — one flat colour, so the geometry reads
    //   on its own with nothing about the flame in it.
    // - `OPENSHARD_SCENE_SOLIDS=lit` — `light::sample` run once per visible
    //   face (`Solid::faces`'s `[Top, East, South]`, the lid at `Surface::Flat`
    //   and the two sides at their own `Surface::Face`), the real multiplier
    //   painted straight on rather than the kind-coloured, [`Side::shade`]d
    //   picture `render` draws — see `solids::SolidsRenderer::render_lit`.
    if let Some(mode) = env_opt("OPENSHARD_SCENE_SOLIDS") {
        let standing = openshard_client_render::solid::standing(
            &lighting.occlusion,
            openshard_client_render::solid::Cut::Nothing,
        );
        let count = standing.len();

        let mut solids_pass = openshard_client_render::solids::SolidsRenderer::new(&device, format);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        // `SolidsRenderer` only ever loads (see its own doc) — this clear is
        // the black the boxes sit on, and the only reason a pass runs first.
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("solids-only clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &surface_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        let rect = ViewportRect {
            x: 0,
            y: 0,
            width: viewport.0,
            height: viewport.1,
        };
        // On by default, like the live F5 overlay — off to see the fills alone,
        // unmixed with a stroke's own colour. See `SolidsRenderer::render`'s own
        // doc: with the strokes gone, a face that something nearer should have
        // hidden and did not reads as a wrongly bright translucent patch rather
        // than a line quietly sitting on top of it.
        // On by default, like the live F5 overlay — off to see the fills alone,
        // unmixed with a stroke's own colour. See `SolidsRenderer::render`'s own
        // doc: with the strokes gone, a face that something nearer should have
        // hidden and did not reads as a wrongly bright translucent patch rather
        // than a line quietly sitting on top of it.
        //
        // `opaque` is off by default too, matching the live overlay's own
        // translucency, and on for the question that overlay was never asked —
        // whether a face is genuinely hidden or only tinted through.
        let style = openshard_client_render::solids::Style {
            edges: env_flag("OPENSHARD_SCENE_SOLIDS_EDGES", true),
            opaque: env_flag("OPENSHARD_SCENE_SOLIDS_OPAQUE", false),
        };
        match mode.as_str() {
            "white" => {
                let coloured: Vec<_> = standing
                    .into_iter()
                    .map(|(solid, _)| (solid, [255.0; 3]))
                    .collect();
                solids_pass.render(
                    &device,
                    &queue,
                    &mut encoder,
                    openshard_client_render::solids::Frame {
                        target: &surface_view,
                        size: viewport,
                        rect,
                    },
                    &camera,
                    &coloured,
                    style,
                );
            }
            "lit" => {
                let coloured: Vec<_> = standing
                    .into_iter()
                    .map(|(solid, _)| (solid, face_light(&solid, &lighting)))
                    .collect();
                solids_pass.render_lit(
                    &device,
                    &queue,
                    &mut encoder,
                    openshard_client_render::solids::Frame {
                        target: &surface_view,
                        size: viewport,
                        rect,
                    },
                    &camera,
                    &coloured,
                    style,
                );
            }
            other => panic!("OPENSHARD_SCENE_SOLIDS wants `white` or `lit`, got {other:?}"),
        }
        queue.submit([encoder.finish()]);

        let png = dump_frame(&device, &queue, &surface, viewport);
        let path = PathBuf::from(env("OPENSHARD_FRAME_DUMP"));
        std::fs::write(&path, png).expect("writing the frame");
        eprintln!(
            "wrote {} ({count} solids, {mode}, edges {}, opaque {})",
            path.display(),
            style.edges,
            style.opaque,
        );
        return;
    }

    let wanted_view = env_opt("OPENSHARD_FRAME_VIEW")
        .and_then(|v| v.parse::<usize>().ok())
        .and_then(|v| openshard_client_render::debug::View::ALL.get(v).copied())
        .unwrap_or_default();
    lighting.view = wanted_view;

    let mut blit = Blit::new(&device, format);
    // No mobile pass in this scene: the dummy stands in for it.
    let dummy_instances = openshard_client_render::blit::dummy_instances(&device);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    blit.render(
        &device,
        &queue,
        &mut encoder,
        openshard_client_render::blit::Frame {
            target: &surface_view,
            world: &world_view,
            place: &place_view,
            face_instances: items_pass.instances_buffer(),
            mobile_instances: &dummy_instances,
            mesh_instances: mesh_pass.rows_buffer(),
            ground_instances: ground_pass.instances_buffer(),
            zoom: Zoom::ONE,
            rect: ViewportRect {
                x: 0,
                y: 0,
                width: viewport.0,
                height: viewport.1,
            },
        },
        &lighting,
    );
    queue.submit([encoder.finish()]);

    let png = dump_frame(&device, &queue, &surface, viewport);
    let path = PathBuf::from(env("OPENSHARD_FRAME_DUMP"));
    std::fs::write(&path, png).expect("writing the frame");
    eprintln!("wrote {} ({:?})", path.display(), wanted_view);
}
