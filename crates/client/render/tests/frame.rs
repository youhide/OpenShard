//! Frames, rendered on a real GPU and read back pixel by pixel.
//!
//! A renderer's usual problem is that it has no oracle: the output is looked at,
//! and "looks right" survives an off-by-one in the projection, a swapped colour
//! channel and a sprite sampled one texel over. Rendering to a texture instead
//! of a window removes that excuse — the frame is bytes, and bytes can be
//! compared with the art the frame was built from.
//!
//! Two things gate these tests, and both are honest skips rather than failures:
//! `OPENSHARD_CLIENT`, because no client files live in this repository, and the
//! presence of an adapter, because CI machines do not always have one.

use std::path::PathBuf;

use openshard_client_render::animate::StaticAnimations;
use openshard_client_render::atlas::FrameKey;
use openshard_client_render::atlas::{AnimAtlas, LandAtlas, StaticAtlas, TexmapAtlas};
use openshard_client_render::blit::{Blit, ViewportRect};
use openshard_client_render::camera::{Camera, Projection, WorldPoint, Zoom};
use openshard_client_render::cutaway::Cutaway;
use openshard_client_render::debug::View;
use openshard_client_render::geometry::{Rect, Vec2};
use openshard_client_render::ground::{self, GroundQuad};
use openshard_client_render::hue::HueRamp;
use openshard_client_render::light::{Light, Lighting, Surface};

/// The reach the lighting tests give their flame, in tiles.
///
/// Chosen here rather than read from `light::TORCH`: these tests are about the
/// shader's falloff and its shadow walk, not about which flame a graphic gets.
/// Three and not a torch's six: at 44 pixels a tile, a 256-pixel frame holds
/// five tiles of one row, and a pool that reached six of them would have no
/// "outside" to compare against inside the picture.
const TORCH_TILES: f32 = 3.0;
use openshard_client_render::camera::TileBounds;
use openshard_client_render::mobiles::{self, Mobile};
use openshard_client_render::occlusion::{Builder, Occlusion, OwnerId, Shape};
use openshard_client_render::outline::{self, Outline, Ring};
use openshard_client_render::place::Place;
use openshard_client_render::renderer::{self, GroundRenderer, SpriteRenderer, Target};
use openshard_client_render::sprite::SpriteQuad;
use openshard_client_render::statics;
use openshard_protocol::direction::Direction;
use openshard_protocol::wire::Graphic;
use openshard_protocol::world::Point;
use openshard_uofiles::anim::{Anim, AnimFrame};
use openshard_uofiles::art::{Art, LAND_TILE_SIZE, land_row};
use openshard_uofiles::color::Color16;
use openshard_uofiles::equipconv::EquipConv;
use openshard_uofiles::hues::Hues;
use openshard_uofiles::image::Image;
use openshard_uofiles::map::Map;
use openshard_uofiles::texmaps::TexMaps;
use openshard_uofiles::tiledata::{StaticTile, TileData, TileFlags};

/// The client's files, or `None` when the environment does not point at any.
fn client_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("OPENSHARD_CLIENT")?))
}

/// The texture atlas for a set of land graphics, read from a real install.
///
/// Two files rather than one: the textures themselves, and the `tiledata` that
/// says which of them a land graphic uses.
fn texmap_atlas(dir: &std::path::Path, wanted: impl IntoIterator<Item = Graphic>) -> TexmapAtlas {
    let texmaps = TexMaps::open(dir).expect("texidx.mul and texmaps.mul");
    let tiledata = TileData::load(dir.join("tiledata.mul")).expect("tiledata.mul");
    TexmapAtlas::build(&texmaps, &tiledata, wanted).expect("a screen of textures fits")
}

/// A GPU to draw with, or `None` where there is none.
fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    // The defaults are WebGL2's limits in wgpu's downlevel form, which is the
    // point: a pipeline that needs more than this would not run in a browser,
    // and finding that out here is cheaper than finding it out in one.
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()
}

/// A rendered frame, as RGBA8 rows.
struct Frame {
    width: u32,
    pixels: Vec<u8>,
}

impl Frame {
    fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * self.width + x) * 4) as usize;
        [
            self.pixels[at],
            self.pixels[at + 1],
            self.pixels[at + 2],
            self.pixels[at + 3],
        ]
    }

    /// How many pixels the ground pass wrote. Anything drawn is opaque and the
    /// clear is fully transparent, so this counts exactly, with no threshold.
    fn drawn(&self) -> usize {
        self.pixels.chunks_exact(4).filter(|p| p[3] == u8::MAX).count()
    }
}

/// Draw ground into a `width` x `height` texture and read the result back.
///
/// The common case, and the one every test written before statics existed used.
fn render(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    atlas: &LandAtlas,
    texmaps: &TexmapAtlas,
    quads: &[GroundQuad],
    width: u32,
    height: u32,
) -> Frame {
    let empty = StaticAtlas::pack([]).expect("nothing always fits");
    let no_mobiles = AnimAtlas::pack([]).expect("nothing always fits");
    render_both(
        device,
        queue,
        atlas,
        texmaps,
        quads,
        &empty,
        &[],
        (no_mobiles.pixels(), &[]),
        width,
        height,
        Projection::one_to_one(width, height),
    )
}

/// Draw ground through a camera's own projection, rather than 1:1.
///
/// The magnified path, which [`render`] cannot reach: it is the same ground pass
/// and the same quads, and what differs is the two numbers the vertex shader
/// ends on.
#[allow(clippy::too_many_arguments)]
fn render_projected(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    atlas: &LandAtlas,
    texmaps: &TexmapAtlas,
    quads: &[GroundQuad],
    width: u32,
    height: u32,
    camera: Camera,
) -> Frame {
    let empty = StaticAtlas::pack([]).expect("nothing always fits");
    let no_mobiles = AnimAtlas::pack([]).expect("nothing always fits");
    render_both(
        device,
        queue,
        atlas,
        texmaps,
        quads,
        &empty,
        &[],
        (no_mobiles.pixels(), &[]),
        width,
        height,
        camera.projection(),
    )
}

/// Draw both passes into a `width` x `height` texture and read the result back.
///
/// `width * 4` must be a multiple of 256: that is the row alignment a buffer
/// copy demands, and padding it here would only hide the constraint from the
/// callers, which choose their own sizes.
#[allow(clippy::too_many_arguments)]
fn render_both(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    atlas: &LandAtlas,
    texmaps: &TexmapAtlas,
    quads: &[GroundQuad],
    static_atlas: &StaticAtlas,
    static_quads: &[SpriteQuad],
    mobiles: (&[u8], &[SpriteQuad]),
    width: u32,
    height: u32,
    projection: Projection,
) -> Frame {
    assert_eq!(width * 4 % 256, 0, "a row copy has to be 256-byte aligned");

    let format = wgpu::TextureFormat::Rgba8Unorm;
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("frame"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());

    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(width) * u64::from(height) * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    // The depth buffer both passes share. Created here rather than inside the
    // renderer because a test that could not hand the two passes the same one
    // would not be testing the thing that makes them agree.
    let depth = renderer::depth_texture(device, width, height);
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
    let place = openshard_client_render::place::texture(device, width, height);
    let place_view = place.create_view(&wgpu::TextureViewDescriptor::default());

    // None of these frames ask for a hue — every quad built below carries
    // `hue: 0` — so an empty ramp is a real texture the shader can bind rather
    // than a special case: it is never indexed because nothing here sets the
    // bit that would make the shader look.
    let hue_ramp = HueRamp::build(&Hues::parse(&[0u8; 708]).expect("one empty group"));

    let mut renderer = GroundRenderer::new(device, queue, format, atlas, texmaps);
    let mut statics = SpriteRenderer::new(device, queue, format, static_atlas.pixels(), &hue_ramp);
    // The mobiles are the same pass again with another atlas bound, which is
    // the whole of the difference between a static and a creature on the GPU.
    let mut people = SpriteRenderer::new(device, queue, format, mobiles.0, &hue_ramp);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let target_view = Target {
        place: &place_view,
        view: &view,
        depth: &depth_view,
        width,
        height,
        projection,
    };
    renderer.render(device, queue, &mut encoder, target_view, quads);
    statics.render(device, queue, &mut encoder, target_view, static_quads, None);
    people.render(device, queue, &mut encoder, target_view, mobiles.1, None);
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
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
        result.expect("mapping a buffer this test just wrote");
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("waiting on our own submission");
    let pixels = slice
        .get_mapped_range()
        .expect("the map completed above")
        .to_vec();
    readback.unmap();

    Frame { width, pixels }
}

/// One sprite, drawn alone, compared to the art texel for texel.
///
/// This is the test that ties the three layers together: the atlas packed the
/// sprite somewhere, the instance carried texture coordinates, and the shader
/// sampled them. Any of the three being off by a texel moves the diamond, and
/// nothing else in the suite would notice — a whole frame of ground still looks
/// like ground when every tile samples its neighbour.
#[test]
fn a_lone_sprite_matches_the_art_it_came_from() {
    let (Some(dir), Some((device, queue))) = (client_dir(), gpu()) else {
        return;
    };
    let art = Art::open(&dir).expect("artLegacyMUL.uop");

    // The first land graphic the client actually ships. Which one it is does not
    // matter; that it is real art with a real shape does.
    let (graphic, image) = (0..0x4000u16)
        .map(Graphic)
        .find_map(|g| art.land(g).expect("reading land art").map(|image| (g, image)))
        .expect("a modern client ships thousands of land tiles");

    let atlas = LandAtlas::build(&art, [graphic]).expect("one graphic fits");
    let region = atlas.region(graphic).expect("just packed");

    // Level, and centred so its bounding square starts at the viewport's origin:
    // viewport coordinates are then the sprite's own. A tile whose four corners
    // share a height is drawn as the art's square, which is what makes this
    // comparison texel for texel possible at all — see `ground.wgsl`.
    let quads = [GroundQuad {
        x: f32::from(LAND_TILE_SIZE) / 2.0,
        y: f32::from(LAND_TILE_SIZE) / 2.0,
        corners: [0.0; 4],
        region,
        texmap: None,
        // Anything inside clip space: this frame holds one quad, so there is
        // nothing for the depth test to decide.
        depth: 0.5,
        place: Place::land(0, 0),
    }];
    let side = u32::from(LAND_TILE_SIZE);
    let empty = TexmapAtlas::pack([]).expect("nothing always fits");
    let frame = render(&device, &queue, &atlas, &empty, &quads, 64, 64);

    let mut compared = 0;
    for y in 0..side {
        let row = land_row(y as u16);
        for x in 0..side {
            let got = frame.pixel(x, y);
            if !row.contains(&(x as u16)) {
                assert_eq!(got[3], 0, "({x}, {y}) is outside the diamond but was drawn");
                continue;
            }
            // Inside the diamond every pixel is drawn, black ones included:
            // ground has no transparency, and a tile that loses its zero pixels
            // is a tile with pinholes in it.
            let (r, g, b) = image.pixel(x as u16, y as u16).expect("inside the sprite").rgb8();
            assert_eq!(
                got,
                [r, g, b, u8::MAX],
                "({x}, {y}) does not match the art: the sprite is sampled from the wrong place",
            );
            compared += 1;
        }
    }

    // The diamond is 1,012 of the square's 1,936 pixels. Without this the loop
    // above would pass on a sprite that decoded to nothing at all.
    assert_eq!(compared, 1012, "the diamond should be 1,012 drawn pixels");

    // And nothing outside the sprite's own square was touched.
    for y in 0..64 {
        for x in 0..64 {
            if x < side && y < side {
                continue;
            }
            assert_eq!(frame.pixel(x, y)[3], 0, "({x}, {y}) is outside the quad");
        }
    }
}

/// Level ground tiles the viewport exactly, with no pixel left over.
///
/// This is the assertion the projection lives or dies by: the diamonds only
/// meet if a step is exactly 22 pixels on each axis and the sprite is exactly
/// 44 across. Any other numbers leave a lattice of gaps, and a lattice of gaps
/// against a black background is close to invisible on a screenshot.
///
/// It is deliberately *level* ground. Flat diamonds are only the whole truth
/// where the four corners of a tile share a height, and the sea is the largest
/// place that is true — see the sibling test for what happens on a hillside.
#[test]
fn level_ground_covers_every_pixel() {
    let (Some(dir), Some((device, queue))) = (client_dir(), gpu()) else {
        return;
    };
    let map = Map::load_facet(&dir, 0).expect("Felucca");
    let art = Art::open(&dir).expect("artLegacyMUL.uop");

    // Open sea off the north-west corner: 80 tiles square at a single height.
    let camera = Camera::new(Point::new(200, 200, -5), 768, 512);

    // The premise, checked rather than assumed. If this patch of Felucca ever
    // stopped being level the coverage assertion below would start measuring
    // the terrain instead of the projection, and would still be green.
    for y in 160..240u16 {
        for x in 160..240u16 {
            assert_eq!(
                map.land(x, y).map(|cell| cell.z),
                Some(-5),
                "({x}, {y}) is not at sea level; this test needs level ground",
            );
        }
    }

    let wanted = ground::visible_graphics(&map, &camera);
    let atlas = LandAtlas::build(&art, wanted.iter().copied()).expect("fits");
    let texmaps = texmap_atlas(&dir, wanted);
    let quads = ground::collect(&map, &camera, &atlas, &texmaps, &Cutaway::OPEN);
    assert!(!quads.is_empty(), "the sea is made of land tiles too");

    let frame = render(
        &device,
        &queue,
        &atlas,
        &texmaps,
        &quads,
        camera.width,
        camera.height,
    );
    let total = (camera.width * camera.height) as usize;
    assert_eq!(
        frame.drawn(),
        total,
        "level ground left holes: the diamonds do not meet",
    );

    // One flat colour would satisfy everything above, and is also what a broken
    // atlas produces.
    let first = frame.pixel(0, 0);
    assert!(
        (0..camera.height).any(|y| (0..camera.width).any(|x| frame.pixel(x, y) != first)),
        "the whole frame is one colour",
    );
}

/// A screen of Britain: hilly ground covers the viewport as completely as the
/// sea does.
///
/// This is the assertion stretched ground exists for. Flat 44x44 diamonds drawn
/// at different heights pull apart along a slope and leave a lattice of seams —
/// which is what this test used to pin, at 97.7% of the viewport. A tile
/// stretched over its four corner heights cannot do that: neighbours are built
/// from *the same* corners, so the mesh is watertight by construction rather
/// than by the projection's arithmetic coming out even.
///
/// Real terrain is also the only place the two shapes meet, so this covers the
/// join between them: a flat tile beside a sloped one, and no gap at the seam.
#[test]
fn hilly_ground_covers_every_pixel() {
    let (Some(dir), Some((device, queue))) = (client_dir(), gpu()) else {
        return;
    };
    let map = Map::load_facet(&dir, 0).expect("Felucca");
    let art = Art::open(&dir).expect("artLegacyMUL.uop");

    // Britain, near the bank: the ground here runs from z = -15 to z = 25.
    let camera = Camera::new(Point::new(1495, 1629, 0), 768, 512);
    let wanted = ground::visible_graphics(&map, &camera);
    let atlas = LandAtlas::build(&art, wanted.iter().copied()).expect("fits");
    let texmaps = texmap_atlas(&dir, wanted);
    let quads = ground::collect(&map, &camera, &atlas, &texmaps, &Cutaway::OPEN);

    // Every cell the camera can see became a quad: the client ships art for all
    // of them, so a missing quad would mean the atlas or the lookup lost one.
    let bounds = camera.visible_tiles();
    let cells = (bounds.min_y.max(0)..=bounds.max_y)
        .flat_map(|y| (bounds.min_x.max(0)..=bounds.max_x).map(move |x| (x, y)))
        .filter(|&(x, y)| map.land(x as u16, y as u16).is_some())
        .count();
    assert_eq!(quads.len(), cells, "a visible tile was dropped");

    // The premise: this camera has to be looking at a hillside, or the test is
    // the level-ground one again under another name and would stay green
    // through the loss of everything it is here to protect.
    let sloped = quads
        .iter()
        .filter(|quad| quad.corners.iter().any(|z| *z != quad.corners[0]))
        .count();
    assert!(sloped > 100, "only {sloped} of {} quads slope", quads.len());

    // And most of those slopes are textured rather than falling back to the
    // stretched art, or the texture path is being exercised by nothing.
    let textured = quads
        .iter()
        .filter(|quad| quad.corners.iter().any(|z| *z != quad.corners[0]) && quad.texmap.is_some())
        .count();
    assert!(
        textured * 2 > sloped,
        "only {textured} of {sloped} sloped tiles have a texture map",
    );

    let frame = render(
        &device,
        &queue,
        &atlas,
        &texmaps,
        &quads,
        camera.width,
        camera.height,
    );
    let total = (camera.width * camera.height) as usize;
    assert_eq!(
        frame.drawn(),
        total,
        "hilly ground left holes: the corner heights do not meet",
    );
}

/// A sloped tile is drawn from its texture map, and a level one from its art.
///
/// The one assertion that says the branch in `ground.wgsl` is on the *heights*
/// and reads the *right* atlas. Both pictures are made here rather than read
/// from a client, and they are told apart by colour alone: green art, red
/// texture. Nothing subtler is needed and nothing subtler would survive a
/// reader's understanding writing both sides of the comparison — which is
/// exactly the trap `uofiles` fell into.
#[test]
fn a_sloped_tile_is_drawn_from_its_texture_and_a_level_one_from_its_art() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const GRAPHIC: Graphic = Graphic(1);
    let green = Color16(0b0_00000_11111_00000);
    let red = Color16(0b0_11111_00000_00000);

    let side = usize::from(LAND_TILE_SIZE);
    let art = Image::new(LAND_TILE_SIZE, LAND_TILE_SIZE, vec![green; side * side]);
    let texture = Image::new(64, 64, vec![red; 64 * 64]);
    let atlas = LandAtlas::pack([(GRAPHIC, art)]).expect("one sprite fits");
    let texmaps = TexmapAtlas::pack([(GRAPHIC, texture)]).expect("one texture fits");
    let region = atlas.region(GRAPHIC).expect("packed");
    let texmap = texmaps.region(GRAPHIC).expect("packed");

    // Two tiles in one frame, far enough apart not to touch: one level, one
    // over four different corner heights. Same graphic, same regions — only the
    // heights differ, which is the whole claim.
    let quads = [
        GroundQuad {
            x: 64.0,
            y: 128.0,
            corners: [0.0; 4],
            region,
            texmap: Some(texmap),
            depth: 0.5,
            place: Place::land(1, 1),
        },
        GroundQuad {
            // Its own corner raised and its neighbours level: a hillock, and
            // the one direction that makes the quad *bigger* than the diamond
            // rather than shearing it into something smaller.
            x: 192.0,
            y: 128.0,
            corners: [4.0, 0.0, 0.0, 0.0],
            region,
            texmap: Some(texmap),
            depth: 0.5,
            place: Place::land(2, 2),
        },
    ];
    let frame = render(&device, &queue, &atlas, &texmaps, &quads, 256, 256);

    let (mut art_pixels, mut textured_pixels) = (0, 0);
    for y in 0..256 {
        for x in 0..256 {
            let pixel = frame.pixel(x, y);
            if pixel[3] == 0 {
                continue;
            }
            // The left half is the level tile and the right half the slope, so
            // a colour on the wrong side is a tile drawn from the wrong atlas.
            let expected = if x < 128 { green } else { red };
            let (r, g, b) = expected.rgb8();
            assert_eq!(
                pixel,
                [r, g, b, u8::MAX],
                "({x}, {y}) was drawn from the wrong atlas",
            );
            if x < 128 {
                art_pixels += 1;
            } else {
                textured_pixels += 1;
            }
        }
    }

    // A level tile is the art's diamond, exactly as the lone-sprite test pins
    // it. Anything else here and the comparison above was made against an empty
    // frame.
    assert_eq!(art_pixels, 1012, "the level tile is not the art's diamond");
    assert!(
        textured_pixels > 1012,
        "the sloped tile covered only {textured_pixels} pixels; it should be a stretched diamond",
    );
}

/// Read an RGBA8 texture back into a [`Frame`].
///
/// `width * 4` must be a multiple of 256, the row alignment a buffer copy
/// demands. Split out of [`render_both`] because the blit test reads two
/// textures — the world image and the surface it was blitted onto — and
/// comparing them is the whole assertion.
fn read_back(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Frame {
    let (width, height) = (texture.width(), texture.height());
    assert_eq!(width * 4 % 256, 0, "a row copy has to be 256-byte aligned");
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(width) * u64::from(height) * 4,
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
        result.expect("mapping a buffer this test just wrote");
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("waiting on our own submission");
    let pixels = slice
        .get_mapped_range()
        .expect("the map completed above")
        .to_vec();
    readback.unmap();
    Frame { width, pixels }
}

/// At zoom 1 the blit moves no pixel: every texel of the surface is the world
/// image's own texel, put through the colour pipeline and nothing else.
///
/// The property every pixel-exact assertion in this file depends on now that the
/// world is drawn offscreen and stretched onto the surface: if the blit is not
/// the identity at 1:1, then every other test here is measuring an image the
/// screen never shows. A half-texel of sampling error, a flipped vertical axis
/// or a filter left on all read as "slightly soft" on a screenshot and are exact
/// here.
///
/// It used to assert a byte-for-byte copy, and could while lighting was a
/// multiplication of stored bytes by `1.0`. `docs/lighting_rebuild.md`'s phase 1
/// decodes the art out of sRGB, multiplies in linear light and curves the
/// result, and a curve that left `1.0` alone would not be a curve. So the
/// prediction goes through `tonemap::shade_u8` — which is a **stronger**
/// assertion than the copy was: it catches a blit that shifts by a texel and a
/// colour pipeline that has drifted from its own CPU twin, and only the first of
/// those was ever covered here.
///
/// No client files: the scene is two coloured diamonds made in memory, which is
/// enough to have edges — a flat field of one colour would survive any of those
/// mistakes.
#[test]
fn the_blit_at_zoom_one_is_the_world_image_texel_for_texel() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const GRAPHIC: Graphic = Graphic(1);
    let side = usize::from(LAND_TILE_SIZE);
    let art = Image::new(
        LAND_TILE_SIZE,
        LAND_TILE_SIZE,
        (0..side * side)
            // A gradient rather than a wash: a filter left on averages
            // neighbours, and neighbours that differ are what makes that
            // visible.
            .map(|at| Color16(((at % 31) as u16) << 10 | ((at / 31 % 31) as u16) << 5 | 1))
            .collect(),
    );
    let atlas = LandAtlas::pack([(GRAPHIC, art)]).expect("one sprite fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let region = atlas.region(GRAPHIC).expect("packed");
    let quads: Vec<GroundQuad> = [(40.0, 40.0), (150.0, 96.0)]
        .into_iter()
        .map(|(x, y)| GroundQuad {
            x,
            y,
            corners: [0.0; 4],
            region,
            texmap: None,
            depth: 0.5,
            place: Place::land(1, 1),
        })
        .collect();

    let (width, height) = (256, 256);
    let world = openshard_client_render::blit::world_texture(&device, width, height);
    let world_view = world.create_view(&wgpu::TextureViewDescriptor::default());
    let depth = renderer::depth_texture(&device, width, height);
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
    let place = openshard_client_render::place::texture(&device, width, height);
    let place_view = place.create_view(&wgpu::TextureViewDescriptor::default());
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut ground_pass = GroundRenderer::new(&device, &queue, format, &atlas, &texmaps);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    ground_pass.render(
        &device,
        &queue,
        &mut encoder,
        Target::whole(&world_view, &depth_view, &place_view, width, height),
        &quads,
    );
    queue.submit([encoder.finish()]);

    // The surface stands in for a window: same size, so a zoom of 1 asks the
    // blit for exactly the identity.
    let surface = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("surface"),
        size: wgpu::Extent3d {
            width,
            height,
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
    let mut blit = Blit::new(&device, format);
    let dummy_instances = openshard_client_render::blit::dummy_instances(&device);
    let dummy_mesh_instances = openshard_client_render::blit::dummy_mesh_instances(&device);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    blit.render(
        &device,
        &queue,
        &mut encoder,
        // Qualified: this file has a `Frame` of its own, which is a read-back
        // picture rather than a blit's arguments.
        openshard_client_render::blit::Frame {
            target: &surface_view,
            world: &world_view,
            place: &place_view,
            // Ground only: nothing here ever indexes either static/mobile
            // buffer, but the ground quads drawn above are real, so their id
            // has to resolve through the real buffer and not a dummy.
            face_instances: &dummy_instances,
            mobile_instances: &dummy_instances,
            mesh_instances: &dummy_mesh_instances,
            ground_instances: ground_pass.instances_buffer(),
            zoom: Zoom::ONE,
            rect: ViewportRect {
                x: 0,
                y: 0,
                width,
                height,
            },
        },
        // The identity: this test is about the blit being a copy, and lighting
        // is a multiplication by one for it.
        &Lighting::NONE,
    );
    queue.submit([encoder.finish()]);

    let drawn = read_back(&device, &queue, &world);
    let blitted = read_back(&device, &queue, &surface);

    // The scene has to be worth comparing. Two diamonds of a gradient cover a
    // couple of thousand pixels of a 65,536-pixel frame, and an empty frame
    // would compare equal to another empty frame.
    assert!(
        drawn.drawn() > 2000,
        "the world image holds only {} drawn pixels",
        drawn.drawn(),
    );
    for y in 0..height {
        for x in 0..width {
            let world = drawn.pixel(x, y);
            let got = blitted.pixel(x, y);
            // `Lighting::NONE` is a white ambient, so the light is `1.0` on
            // every channel and what is left is the pipeline itself.
            let rgb = openshard_client_render::tonemap::shade_u8([world[0], world[1], world[2]], [1.0; 3]);
            let want = [rgb[0], rgb[1], rgb[2], world[3]];
            // One step, for the reason the parity sweep allows one: the GPU and
            // the CPU evaluate the same `pow` on different hardware.
            for channel in 0..4 {
                let apart = i32::from(want[channel]) - i32::from(got[channel]);
                assert!(
                    apart.abs() <= 1,
                    "({x}, {y}) channel {channel}: the blit drew {got:?}, the pipeline says \
                     {want:?} for a world texel of {world:?}",
                );
            }
        }
    }
}

/// A light brightens the pixels under it and nothing else, and the ambient
/// darkens everything the light does not reach.
///
/// The only oracle the lighting shader has: everything else about it is CPU
/// arithmetic with tests of its own in `light.rs`, and the part that is neither
/// — the falloff, the multiply, the loop bound by a count in a uniform — exists
/// only as WGSL and can only be read back off a GPU.
///
/// The scene is a flat grey world image, deliberately: this test is about what
/// the *lighting* did to a pixel, and a gradient underneath would make every
/// comparison between two pixels a statement about the art as well.
#[test]
fn a_light_brightens_its_own_pool_and_the_ambient_darkens_the_rest() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    let (width, height) = (256, 256);
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let world = openshard_client_render::blit::world_texture(&device, width, height);
    let world_view = world.create_view(&wgpu::TextureViewDescriptor::default());
    let depth = renderer::depth_texture(&device, width, height);
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
    let place = openshard_client_render::place::texture(&device, width, height);
    let place_view = place.create_view(&wgpu::TextureViewDescriptor::default());

    // A flat grey field: one land graphic whose art is a single value, drawn
    // over the whole frame. Mid-grey and not white, so that "brighter" is
    // expressible in both directions.
    const GRAPHIC: Graphic = Graphic(1);
    let side = usize::from(LAND_TILE_SIZE);
    let grey = Color16(15 << 10 | 15 << 5 | 15);
    let art = Image::new(LAND_TILE_SIZE, LAND_TILE_SIZE, vec![grey; side * side]);
    let atlas = LandAtlas::pack([(GRAPHIC, art)]).expect("one sprite fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let region = atlas.region(GRAPHIC).expect("packed");
    // The field, as a lattice of tiles: each quad carries a tile of its own in
    // the place channel, because the lighting is computed in tiles and a frame
    // whose every pixel named one tile would be lit as a single flat thing.
    // `spots` remembers where each of them landed, which is how this test asks
    // "how bright is tile (i, j)" of a picture.
    let mut quads = Vec::new();
    let mut spots = Vec::new();
    for (row, y) in (0..height as i32 + 44).step_by(22).enumerate() {
        for (column, x) in (-44..width as i32 + 44).step_by(44).enumerate() {
            let at = ((x + (y / 22 % 2) * 22) as f32, y as f32);
            let tile = (100 + column as u16, 100 + row as u16);
            quads.push(GroundQuad {
                x: at.0,
                y: at.1,
                corners: [0.0; 4],
                region,
                texmap: None,
                depth: 0.5,
                place: Place::land(tile.0, tile.1),
            });
            spots.push((tile, at));
        }
    }
    let mut ground_pass = GroundRenderer::new(&device, &queue, format, &atlas, &texmaps);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    ground_pass.render(
        &device,
        &queue,
        &mut encoder,
        Target::whole(&world_view, &depth_view, &place_view, width, height),
        &quads,
    );
    queue.submit([encoder.finish()]);

    let surface = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("surface"),
        size: wgpu::Extent3d {
            width,
            height,
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
    let mut blit = Blit::new(&device, format);

    // Three tiles of one row, all of them well inside the picture: the one the
    // flame stands on, one half way out of its reach, and one outside it
    // altogether. Near the left edge, because the row has to be long enough to
    // hold all three at 44 pixels a tile.
    let inside = |at: &(f32, f32)| (10.0..230.0).contains(&at.0) && (30.0..200.0).contains(&at.1);
    let (burning, burning_at) = *spots
        .iter()
        .find(|(_, at)| inside(at) && at.0 < 40.0 && at.1 > 100.0)
        .expect("the lattice covers the left of the frame");
    let find = |dx: u16| {
        *spots
            .iter()
            .find(|((x, y), at)| *x == burning.0 + dx && *y == burning.1 && inside(at))
            .unwrap_or_else(|| panic!("no tile {dx} east of the flame is on screen"))
    };
    let (_, half_way_at) = find(1);
    let (_, outside_at) = find(TORCH_TILES as u16 + 1);

    // One flame, on its own tile, reaching six tiles of ground.
    let lighting = Lighting {
        ambient: openshard_client_render::light::NIGHT,
        lights: vec![Light {
            at: Vec2::new(f32::from(burning.0), f32::from(burning.1)),
            z: 0.0,
            radius: TORCH_TILES,
            color: [1.0, 0.7, 0.35],
            intensity: 1.0,
            // A fire on the ground, lighting every direction: what a beam does
            // is `a_carried_beam_lights_the_way_it_is_pointed`'s claim.
            beam: None,
        }],
        // Nothing stands in the way: what a wall does is
        // `a_wall_stops_the_light_behind_it`'s claim, not this one's.
        occlusion: Occlusion::EMPTY,
        sun: None,
        view: View::Lit,
    };
    let dummy_instances = openshard_client_render::blit::dummy_instances(&device);
    let dummy_mesh_instances = openshard_client_render::blit::dummy_mesh_instances(&device);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    blit.render(
        &device,
        &queue,
        &mut encoder,
        openshard_client_render::blit::Frame {
            target: &surface_view,
            world: &world_view,
            place: &place_view,
            // Ground only, and the ground quads drawn above are real, so
            // their id has to resolve through the real buffer.
            face_instances: &dummy_instances,
            mobile_instances: &dummy_instances,
            mesh_instances: &dummy_mesh_instances,
            ground_instances: ground_pass.instances_buffer(),
            zoom: Zoom::ONE,
            rect: ViewportRect {
                x: 0,
                y: 0,
                width,
                height,
            },
        },
        &lighting,
    );
    queue.submit([encoder.finish()]);

    let drawn = read_back(&device, &queue, &world);
    let lit = read_back(&device, &queue, &surface);
    // The scene has to have something in it, or every comparison below is
    // between two black pixels and holds for any shader at all.
    assert!(drawn.drawn() > 60_000, "the world image is mostly empty");

    // **In linear light, not in stored bytes.** Every comparison below is a
    // ratio — "twice as bright", "brighter than" — and a ratio of sRGB bytes is
    // a statement about a transfer function rather than about the light. The
    // frame is `encode(tonemap(radiance))`, so decoding gives back the tonemapped
    // radiance: monotone in the light, which is all a ratio here needs.
    let luma = |pixel: [u8; 4]| {
        let channel = |value: u8| openshard_client_render::tonemap::srgb_to_linear(f32::from(value) / 255.0);
        // Scaled to the same `0..765` the byte sum used, so the numbers in these
        // messages stay the size a reader of this test is used to.
        ((channel(pixel[0]) + channel(pixel[1]) + channel(pixel[2])) * 255.0) as u32
    };
    // A tile's own middle: `GroundQuad::x`/`y` is the diamond's centre in
    // viewport pixels, which is the one pixel of a tile that is certainly the
    // tile's own and not its neighbour's.
    let sample = |at: (f32, f32)| (at.0 as u32, at.1 as u32);
    let (cx, cy) = sample(burning_at);
    let centre = luma(lit.pixel(cx, cy));
    let (hx, hy) = sample(half_way_at);
    let edge_of_pool = luma(lit.pixel(hx, hy));
    let (fx, fy) = sample(outside_at);
    let far = luma(lit.pixel(fx, fy));
    let unlit = luma(drawn.pixel(fx, fy));

    assert!(
        far < unlit,
        "the ambient did not darken the frame: {far} against {unlit}"
    );
    assert!(
        centre > far * 2,
        "the pool is not brighter than the dark around it: {centre} against {far}",
    );
    assert!(
        edge_of_pool > far && edge_of_pool < centre,
        "the falloff is not monotonic: centre {centre}, edge {edge_of_pool}, outside {far}",
    );
    // The pool is warm, not white: a light whose colour was dropped would pass
    // every brightness assertion above.
    let middle = lit.pixel(cx, cy);
    assert!(
        middle[0] > middle[2],
        "the light's colour was ignored: {middle:?}",
    );
    // And nothing outside the radius is touched by the light at all — the
    // ambient alone accounts for it, which is what makes the pool a shape.
    // The grid is empty, so every tile sees the whole sky and the ambient here is
    // the night's two terms summed — see `light::Ambient`.
    let night = openshard_client_render::light::NIGHT.at(openshard_client_render::occlusion::SKY_OPEN);
    let outside = lit.pixel(fx, fy);
    for (channel, (got, (drawn, ambient))) in outside
        .iter()
        .zip(drawn.pixel(fx, fy).iter().zip(night))
        .take(3)
        .enumerate()
    {
        // Through the pipeline rather than by multiplying the stored byte —
        // `docs/lighting_rebuild.md` phase 1, and the same correction the wall
        // test below carries.
        let expected = (openshard_client_render::tonemap::shade(f32::from(*drawn) / 255.0, ambient) * 255.0)
            .round() as i32;
        assert!(
            (i32::from(*got) - expected).abs() <= 1,
            "outside the pool, channel {channel} is {got} against the ambient's {expected}",
        );
    }
}

/// A wall stops the light behind it.
///
/// The claim `docs/lighting.md` exists for: a torch inside a house must not
/// light the street. `docs/lighting.md`'s own second half of that
/// claim — a wall's own face stays the brightest thing near the flame — is
/// deliberately not this fixture's to make: it stands the occluder in as a
/// *ground* quad rather than a real wall sprite ("what occludes is the grid,
/// and a picture of a wall would only make the frame prettier", below), and
/// a ground pixel is `Stance::Flat`, not the `Stance::Face*` a real wall's
/// own visible surface carries. Those are exempted from a same-tile body's
/// own shadow by a different mechanism (`own_run`'s run-of-wall check) that
/// this fixture never exercises — `two_faces_sharing_an_edge_agree_with_
/// light_sample`/`the_shader_and_light_sample_agree_about_which_side_a_wall_
/// is_on` cover that half instead, on a real face. A `Flat` fragment gets no
/// such exemption at all, by design (`light::exemption`'s own doc: a body a
/// second, taller body stands on needs the precision), so ground standing at
/// a whole-tile body's own base is genuinely inside that body looking out —
/// `docs/lighting_raymarch.md`'s ground-stance entry found this fixture had
/// been passing for the wrong reason, propped up by a land pixel that never
/// named a stance at all and so read as `Upright`, not `Flat`, to the very
/// check this fixture meant to exercise.
#[test]
fn a_wall_stops_the_light_behind_it() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    let (width, height) = (256, 256);
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let world = openshard_client_render::blit::world_texture(&device, width, height);
    let world_view = world.create_view(&wgpu::TextureViewDescriptor::default());
    let depth = renderer::depth_texture(&device, width, height);
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
    let place = openshard_client_render::place::texture(&device, width, height);
    let place_view = place.create_view(&wgpu::TextureViewDescriptor::default());

    // Four tiles of one row, drawn as four flat squares side by side: the flame
    // stands on the first, the wall is on the second, and the two after it are
    // what the wall's shadow falls on. Drawn as ground rather than as a wall
    // sprite deliberately — what occludes is the *grid*, and a picture of a wall
    // would only make the frame prettier.
    const GRAPHIC: Graphic = Graphic(1);
    let side = usize::from(LAND_TILE_SIZE);
    let grey = Color16(15 << 10 | 15 << 5 | 15);
    let art = Image::new(LAND_TILE_SIZE, LAND_TILE_SIZE, vec![grey; side * side]);
    let atlas = LandAtlas::pack([(GRAPHIC, art)]).expect("one sprite fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let region = atlas.region(GRAPHIC).expect("packed");

    const ROW: u16 = 100;
    const FIRST: u16 = 100;
    let centre_of = |tile: u16| (40.0 + f32::from(tile - FIRST) * 44.0, 128.0);
    let quads: Vec<GroundQuad> = (0..4u16)
        .map(|step| {
            let at = centre_of(FIRST + step);
            GroundQuad {
                x: at.0,
                y: at.1,
                corners: [0.0; 4],
                region,
                texmap: None,
                depth: 0.5,
                place: Place::land(FIRST + step, ROW),
            }
        })
        .collect();

    let mut ground_pass = GroundRenderer::new(&device, &queue, format, &atlas, &texmaps);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    ground_pass.render(
        &device,
        &queue,
        &mut encoder,
        Target::whole(&world_view, &depth_view, &place_view, width, height),
        &quads,
    );
    queue.submit([encoder.finish()]);

    let surface = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("surface"),
        size: wgpu::Extent3d {
            width,
            height,
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
    let mut blit = Blit::new(&device, format);

    let flame = Light {
        at: Vec2::new(f32::from(FIRST), f32::from(ROW)),
        z: 0.0,
        // Four tiles, so that the far side of the wall is inside the pool and
        // dark only because the wall is there — a radius that fell short would
        // pass this test for the wrong reason.
        radius: 4.0,
        color: [1.0, 1.0, 1.0],
        intensity: 1.0,
        beam: None,
    };
    let bounds = TileBounds {
        min_x: 90,
        max_x: 110,
        min_y: 90,
        max_y: 110,
    };

    let dummy_instances = openshard_client_render::blit::dummy_instances(&device);
    let dummy_mesh_instances = openshard_client_render::blit::dummy_mesh_instances(&device);
    let read = |blit: &mut Blit, occlusion: Occlusion| -> Frame {
        let lighting = Lighting {
            ambient: openshard_client_render::light::NIGHT,
            lights: vec![flame],
            occlusion,
            sun: None,
            view: View::Lit,
        };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        blit.render(
            &device,
            &queue,
            &mut encoder,
            openshard_client_render::blit::Frame {
                target: &surface_view,
                world: &world_view,
                place: &place_view,
                // Ground only, and the ground quads drawn above are real, so
                // their id has to resolve through the real buffer.
                face_instances: &dummy_instances,
                mobile_instances: &dummy_instances,
                mesh_instances: &dummy_mesh_instances,
                ground_instances: ground_pass.instances_buffer(),
                zoom: Zoom::ONE,
                rect: ViewportRect {
                    x: 0,
                    y: 0,
                    width,
                    height,
                },
            },
            &lighting,
        );
        queue.submit([encoder.finish()]);
        read_back(&device, &queue, &surface)
    };

    let luma = |pixel: [u8; 4]| u32::from(pixel[0]) + u32::from(pixel[1]) + u32::from(pixel[2]);
    let at = |frame: &Frame, tile: u16| {
        let (x, y) = centre_of(tile);
        luma(frame.pixel(x as u32, y as u32))
    };

    // With nothing in the way, every tile of the row is lit.
    let open = read(&mut blit, Builder::new(bounds).finish(&Cutaway::OPEN));
    let (open_wall, open_behind, open_far) = (at(&open, 101), at(&open, 102), at(&open, 103));

    // And with a wall on the second tile, the two behind it are not — while the
    // wall's own tile is exactly as bright as it was.
    let mut occlusion = Builder::new(bounds);
    occlusion.add(
        101,
        ROW,
        0,
        // A plain wall graphic: `occlusion::opacity` asks `doors` first, and a
        // door would be exempt for a reason this test is not about.
        Graphic(0x0100),
        &StaticTile {
            flags: TileFlags::new(TileFlags::NO_SHOOT),
            height: 20,
            ..StaticTile::default()
        },
        // No face: the whole-tile occluder this test has always been about. A
        // named edge would let the ray past on the sides it does not cross,
        // which is a different test — see `occlusion`'s own.
        Shape::UNREAD,
    );
    let walled = read(&mut blit, occlusion.finish(&Cutaway::OPEN));
    let (wall, behind, far) = (at(&walled, 101), at(&walled, 102), at(&walled, 103));

    // The ground *at* the wall's own base is genuinely inside its body looking
    // out at the flame — see this test's own doc comment — and the query
    // point sits at the tile's own *centre*, deep enough inside the body's
    // whole-tile footprint that the ray crosses well past `RAY_CUTOFF`
    // before it ever reaches open air: as dark as `behind`, not a hair off it.
    assert_eq!(
        wall, behind,
        "the wall's own tile did not read as fully inside its own body: {wall}, open {open_wall}, behind {behind}",
    );
    assert!(
        wall < open_wall,
        "the wall's own tile was not darkened by its own body: {wall}, open {open_wall}",
    );
    assert!(
        behind < open_behind && far < open_far,
        "the wall did not stop the light: {behind} of {open_behind}, {far} of {open_far}",
    );
    // Not merely dimmer: as dark as the ambient alone, which is what "stops"
    // means and what a falloff that happened to be steep would not reproduce.
    let world_pixel = read_back(&device, &queue, &world).pixel(centre_of(102).0 as u32, 128);
    // Nothing was ever shaded into this grid — the wall went in through
    // `Occlusion::add`, which is about rays and not about the sky — so every tile
    // still sees the whole of it and the ambient is the night's two terms summed.
    let night = openshard_client_render::light::NIGHT.at(openshard_client_render::occlusion::SKY_OPEN);
    // Through the colour pipeline, not by multiplying the stored bytes: an
    // unlit byte times an ambient is not the byte an ambient produces, and
    // `docs/lighting_rebuild.md` phase 1 is that sentence.
    let ambient_pixel =
        openshard_client_render::tonemap::shade_u8([world_pixel[0], world_pixel[1], world_pixel[2]], night);
    let expected: u32 = ambient_pixel.iter().copied().map(u32::from).sum();
    assert!(
        behind <= expected + 3,
        "the shadow is a dimming rather than a shadow: {behind} against the ambient's {expected}",
    );
}

/// The world passes draw into the world texture on a surface that is not
/// `Rgba8Unorm`.
///
/// The surface's format and the world texture's are two different values, and
/// the frame here is the arrangement that makes them differ: an HDR display
/// offers `Rgba16Float` first among its non-sRGB formats, and a world pipeline
/// built from the surface's format instead of `blit::WORLD_FORMAT` fails
/// validation at `set_pipeline` — the whole client dies on the first frame with
/// nothing drawn. Nothing is read back: the assertion is that the submission
/// validates at all, and a mismatch panics inside `wgpu` before it returns.
#[test]
fn the_world_passes_are_built_for_the_world_texture_not_the_surface() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    let (width, height) = (64, 64);
    let world = openshard_client_render::blit::world_texture(&device, width, height);
    let world_view = world.create_view(&wgpu::TextureViewDescriptor::default());
    let depth = renderer::depth_texture(&device, width, height);
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
    let place = openshard_client_render::place::texture(&device, width, height);
    let place_view = place.create_view(&wgpu::TextureViewDescriptor::default());

    // A sprite made here rather than read from a client, and one real quad: a
    // pass handed nothing returns before it binds its pipeline, which is the
    // one step this test is about.
    const GRAPHIC: Graphic = Graphic(1);
    let art = Image::new(8, 8, vec![Color16(0b0_00000_11111_00000); 64]);
    let atlas = StaticAtlas::pack([(GRAPHIC, art)]).expect("one sprite fits");
    let sprite = atlas.sprite(GRAPHIC).expect("packed");
    let quads = [SpriteQuad {
        rect: Rect {
            x: 4.0,
            y: 4.0,
            width: f32::from(sprite.width),
            height: f32::from(sprite.height),
        },
        region: sprite.region,
        depth: 0.5,
        hue: 0,
        place: Place::NOWHERE,
        twin: 0,
        owner: 0,
    }];
    let hue_ramp = HueRamp::build(&Hues::parse(&[0u8; 708]).expect("one empty group"));
    let mut sprites = SpriteRenderer::new(
        &device,
        &queue,
        openshard_client_render::blit::WORLD_FORMAT,
        atlas.pixels(),
        &hue_ramp,
    );
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    sprites.render(
        &device,
        &queue,
        &mut encoder,
        Target::whole(&world_view, &depth_view, &place_view, width, height),
        &quads,
        None,
    );

    // The stand-in for the HDR surface, in the format the blit and the HUD —
    // and only they — are built for.
    let surface_format = wgpu::TextureFormat::Rgba16Float;
    let surface = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("surface"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: surface_format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let surface_view = surface.create_view(&wgpu::TextureViewDescriptor::default());
    let mut blit = Blit::new(&device, surface_format);
    // No mobile pass in this test: the dummy stands in for it. No ground pass
    // either — this test is about the sprite pass's own target format — so
    // the dummy stands in for that too.
    let dummy_instances = openshard_client_render::blit::dummy_instances(&device);
    let dummy_mesh_instances = openshard_client_render::blit::dummy_mesh_instances(&device);
    let dummy_ground_instances = openshard_client_render::blit::dummy_ground_instances(&device);
    blit.render(
        &device,
        &queue,
        &mut encoder,
        // Qualified: this file has a `Frame` of its own, which is a read-back
        // picture rather than a blit's arguments.
        openshard_client_render::blit::Frame {
            target: &surface_view,
            world: &world_view,
            place: &place_view,
            face_instances: sprites.instances_buffer(),
            mobile_instances: &dummy_instances,
            mesh_instances: &dummy_mesh_instances,
            ground_instances: &dummy_ground_instances,
            zoom: Zoom::ONE,
            rect: ViewportRect {
                x: 0,
                y: 0,
                width,
                height,
            },
        },
        // The identity: this test is about the blit being a copy, and lighting
        // is a multiplication by one for it.
        &Lighting::NONE,
    );
    queue.submit([encoder.finish()]);
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("waiting on our own submission");
}

/// A static sprite is drawn at its own size, in its own place, and its
/// transparent pixels are not drawn at all.
///
/// The statics counterpart of the lone-sprite test, and it needs no client: the
/// picture is made here, so the frame can be compared against it exactly. What
/// it pins is the whole chain — the shelf packer put the sprite somewhere, the
/// instance carried a rectangle and a region, and the shader sampled one texel
/// per pixel. A sprite drawn at the wrong scale still looks like a sprite.
#[test]
fn a_static_sprite_is_drawn_texel_for_texel_with_its_shape_intact() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const GRAPHIC: Graphic = Graphic(1);
    let (width, height) = (17u16, 23u16);

    // A picture with a hole in it: the middle column is absent, which is what
    // a sprite's shape is made of and what the pass has to discard rather than
    // draw black.
    let mut pixels = vec![Color16(0b0_00000_11111_00000); usize::from(width) * usize::from(height)];
    for row in 0..usize::from(height) {
        pixels[row * usize::from(width) + 8] = Color16::TRANSPARENT;
    }
    let art = Image::new(width, height, pixels.clone());
    let atlas = StaticAtlas::pack([(GRAPHIC, art)]).expect("one sprite fits");
    let sprite = atlas.sprite(GRAPHIC).expect("packed");

    let quads = [SpriteQuad {
        rect: Rect {
            x: 10.0,
            y: 20.0,
            width: f32::from(sprite.width),
            height: f32::from(sprite.height),
        },
        region: sprite.region,
        depth: 0.5,
        hue: 0,
        place: Place::NOWHERE,
        twin: 0,
        owner: 0,
    }];
    let land = LandAtlas::pack([]).expect("nothing always fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let none = AnimAtlas::pack([]).expect("nothing always fits");
    let frame = render_both(
        &device,
        &queue,
        &land,
        &texmaps,
        &[],
        &atlas,
        &quads,
        (none.pixels(), &[]),
        128,
        128,
        Projection::one_to_one(128, 128),
    );

    let (green_r, green_g, green_b) = Color16(0b0_00000_11111_00000).rgb8();
    let mut drawn = 0;
    for y in 0..128u32 {
        for x in 0..128u32 {
            let got = frame.pixel(x, y);
            let inside =
                (10..10 + u32::from(width)).contains(&x) && (20..20 + u32::from(height)).contains(&y);
            let transparent = inside && x - 10 == 8;
            if !inside || transparent {
                assert_eq!(got[3], 0, "({x}, {y}) should not have been drawn");
                continue;
            }
            assert_eq!(
                got,
                [green_r, green_g, green_b, u8::MAX],
                "({x}, {y}) is not the sprite's own pixel",
            );
            drawn += 1;
        }
    }
    // Every pixel of the rectangle except the absent column, and nothing else:
    // a sprite drawn at the wrong scale fails this even when every pixel it did
    // draw was the right colour.
    assert_eq!(drawn, usize::from(width - 1) * usize::from(height));
}

/// One `hues.mul` group, `Hue(1)`'s ramp set to `colors` and the other seven
/// entries left at zero — the same construction [`crate`]'s own unit tests
/// cannot reuse across crates, so this test file builds its own bytes from the
/// documented layout rather than from a private helper.
fn one_hue_group(colors: [Color16; 32]) -> Hues {
    const ENTRY_BYTES: usize = 32 * 2 + 2 + 2 + 20;
    let mut bytes = vec![0u8; 4 + 8 * ENTRY_BYTES];
    for (index, color) in colors.iter().enumerate() {
        let at = 4 + index * 2;
        bytes[at..at + 2].copy_from_slice(&color.0.to_le_bytes());
    }
    Hues::parse(&bytes).expect("one whole group")
}

/// The art is not tinted, it is replaced: a full hue looks a pixel up by its
/// own red channel and draws whatever `hues.mul` says, discarding the pixel's
/// original colour entirely — even a pixel that was never grey.
///
/// Both texels below carry the same 5-bit red value and different green and
/// blue, so a shader that multiplied by a tint would leave them visibly
/// different; one that replaces them by index draws them identically.
#[test]
fn a_full_hue_replaces_the_pixel_by_its_red_channel_regardless_of_its_own_colour() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const GRAPHIC: Graphic = Graphic(1);
    const INDEX: u8 = 10;

    // Genuinely grey — all three channels equal `INDEX` — against a texel
    // whose red channel is the same `INDEX` but whose green and blue are not:
    // "partial" is decided by the *pixel*, not by the index alone, so the two
    // have to share an index and differ in colour for the test to mean anything.
    let index = u16::from(INDEX);
    let grey = Color16((index << 10) | (index << 5) | index);
    let coloured = Color16((index << 10) | 0b0_00000_00000_11111);
    assert_ne!(
        grey, coloured,
        "the two texels have to differ for this to test anything"
    );

    let art = Image::new(2, 1, vec![grey, coloured]);
    let atlas = StaticAtlas::pack([(GRAPHIC, art)]).expect("one sprite fits");
    let sprite = atlas.sprite(GRAPHIC).expect("packed");

    let mut ramp_colors = [Color16::TRANSPARENT; 32];
    ramp_colors[usize::from(INDEX)] = Color16(0b0_00000_00000_11111); // pure blue
    let hues = one_hue_group(ramp_colors);
    let hue_ramp = HueRamp::build(&hues);

    let land = LandAtlas::pack([]).expect("nothing always fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");

    let quad = |hue: u32| SpriteQuad {
        rect: Rect {
            x: 0.0,
            y: 0.0,
            width: f32::from(sprite.width),
            height: f32::from(sprite.height),
        },
        region: sprite.region,
        depth: 0.5,
        hue,
        place: Place::NOWHERE,
        twin: 0,
        owner: 0,
    };

    let format = wgpu::TextureFormat::Rgba8Unorm;
    let render_with_ramp = |hue: u32| -> Frame {
        let quads = [quad(hue)];
        render_hued(
            &device, &queue, &land, &texmaps, &atlas, &quads, &hue_ramp, format,
        )
    };

    let (blue_r, blue_g, blue_b) = Color16(0b0_00000_00000_11111).rgb8();
    let (grey_r, grey_g, grey_b) = grey.rgb8();
    let (coloured_r, coloured_g, coloured_b) = coloured.rgb8();

    // Hue 1, no partial flag: both texels come back as the ramp's own colour,
    // not as anything blended with what was there.
    let full = render_with_ramp(1);
    assert_eq!(
        full.pixel(0, 0),
        [blue_r, blue_g, blue_b, u8::MAX],
        "the grey texel"
    );
    assert_eq!(
        full.pixel(1, 0),
        [blue_r, blue_g, blue_b, u8::MAX],
        "the coloured texel too — a full hue does not ask what a pixel looked like",
    );

    // The same hue, partial: only the grey texel is grey enough to tint: the
    // coloured one is left exactly as the art drew it.
    let partial = render_with_ramp(1 | 0x8000);
    assert_eq!(
        partial.pixel(0, 0),
        [blue_r, blue_g, blue_b, u8::MAX],
        "partial still tints a genuinely grey pixel",
    );
    assert_eq!(
        partial.pixel(1, 0),
        [coloured_r, coloured_g, coloured_b, u8::MAX],
        "partial leaves an already-coloured pixel alone",
    );

    // And hue 0 is "no hue": the ramp exists but nothing here samples it.
    let none_hue = render_with_ramp(0);
    assert_eq!(none_hue.pixel(0, 0), [grey_r, grey_g, grey_b, u8::MAX]);
    assert_eq!(
        none_hue.pixel(1, 0),
        [coloured_r, coloured_g, coloured_b, u8::MAX]
    );
}

/// Draw one static pass with a real hue ramp bound, and read the result back.
///
/// [`render_both`] always binds an empty ramp — nothing it draws asks for a
/// hue — so a test that needs a real one calls this instead rather than
/// growing every caller of `render_both` an argument none of them use.
#[allow(clippy::too_many_arguments)]
fn render_hued(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    land: &LandAtlas,
    texmaps: &TexmapAtlas,
    static_atlas: &StaticAtlas,
    quads: &[SpriteQuad],
    hue_ramp: &HueRamp,
    format: wgpu::TextureFormat,
) -> Frame {
    let (width, height) = (64u32, 64u32);
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("hued frame"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hued readback"),
        size: u64::from(width) * u64::from(height) * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let depth = renderer::depth_texture(device, width, height);
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
    let place = openshard_client_render::place::texture(device, width, height);
    let place_view = place.create_view(&wgpu::TextureViewDescriptor::default());

    let mut ground = GroundRenderer::new(device, queue, format, land, texmaps);
    let mut statics = SpriteRenderer::new(device, queue, format, static_atlas.pixels(), hue_ramp);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let target_view = Target::whole(&view, &depth_view, &place_view, width, height);
    ground.render(device, queue, &mut encoder, target_view, &[]);
    statics.render(device, queue, &mut encoder, target_view, quads, None);
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
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
        result.expect("mapping a buffer this test just wrote");
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("waiting on our own submission");
    let pixels = slice
        .get_mapped_range()
        .expect("the map completed above")
        .to_vec();
    readback.unmap();

    Frame { width, pixels }
}

/// Every pixel says which tile it came from, and a wall's pixels say the
/// wall's tile rather than the ground's.
///
/// The attachment `docs/lighting.md` turns on, and the claim that makes it worth
/// having: a wall's picture stands 44 pixels above the tile it is on, so the
/// ground behind it and the wall itself are neighbouring pixels of one image
/// that belong to different tiles at different heights. Everything the lighting
/// pass does rests on being able to tell those two apart, and nothing else in
/// this suite would notice if the channel held the ground's tile everywhere.
#[test]
fn every_pixel_names_the_tile_it_came_from() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const GRAPHIC: Graphic = Graphic(1);
    let green = Color16(0b0_00000_11111_00000);
    let red = Color16(0b0_11111_00000_00000);

    let side = usize::from(LAND_TILE_SIZE);
    let land = LandAtlas::pack([(
        GRAPHIC,
        Image::new(LAND_TILE_SIZE, LAND_TILE_SIZE, vec![green; side * side]),
    )])
    .expect("one sprite fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let statics = StaticAtlas::pack([(GRAPHIC, Image::new(20, 20, vec![red; 20 * 20]))]).expect("fits");
    let region = land.region(GRAPHIC).expect("packed");
    let sprite = statics.sprite(GRAPHIC).expect("packed");

    // The ground tile fills the middle of the image; the wall stands on the
    // *next* tile at a height of its own and is drawn over part of it, which is
    // exactly the pair of pixels this is about.
    let ground = [GroundQuad {
        x: 64.0,
        y: 64.0,
        corners: [0.0; 4],
        region,
        texmap: None,
        depth: 0.6,
        place: Place::land(300, 400),
    }];
    let wall = [SpriteQuad {
        rect: Rect {
            x: 60.0,
            y: 60.0,
            width: f32::from(sprite.width),
            height: f32::from(sprite.height),
        },
        region: sprite.region,
        depth: 0.4,
        hue: 0,
        place: Place::of_static(Point::new(301, 400, 15)),
        twin: 0,
        owner: 0,
    }];

    let places = render_places(
        &device,
        &queue,
        &land,
        &texmaps,
        &ground,
        &statics,
        &wall,
        &[],
        &[],
        128,
    );

    // A pixel of the wall: an id naming the one static this frame drew — its
    // own tile is `docs/gbuffer.md` step 3's `instances[id]` row now, not a
    // number this attachment carries directly — and the static's kind in the
    // low bits of the fourth channel.
    let wall_pixel = places.at(64, 64);
    assert_eq!(
        [wall_pixel[0], wall_pixel[1]],
        [0, 0],
        "a wall's pixel did not name the only static drawn this frame, by id",
    );
    assert_eq!(wall_pixel[3] & 3, 2, "and another kind");
    // Its height is the pixel's own, not the sprite's base: four pixels up the
    // picture is one unit of `z`, which is what gives a wall a gradient down its
    // face instead of one flat brightness.
    //
    // And two pixels up is **half** a unit, which the channel can now say:
    // `docs/lighting_height.md` phase 1 put a fraction under the whole units,
    // and before it this same pair of pixels could only differ by a whole one
    // or by none at all — which is the staircase that phase is about. Decoded
    // through `place::unpacked_height` rather than subtracted raw, because the
    // channel's low eight bits are no longer the whole of the height.
    let higher = places.at(64, 62);
    let height = openshard_client_render::place::unpacked_height;
    assert_eq!(
        height(higher[2]) - height(wall_pixel[2]),
        0.5,
        "two pixels up the wall is not half a unit of height: {higher:?} against {wall_pixel:?}",
    );
    // A pixel of the ground beside it: an id naming the one ground quad this
    // frame drew — its own tile is `docs/gbuffer.md` step 7's
    // `ground_instances[id]` row now, not a number this attachment carries
    // directly, the same move step 3 made for the wall above — at the height
    // the corners gave it, and the land kind.
    let ground_pixel = places.at(64, 84);
    let ground_id = u32::from(ground_pixel[0]) | (u32::from(ground_pixel[1]) << 16);
    assert_eq!(
        ground[ground_id as usize].place,
        Place::land(300, 400),
        "the ground beside the wall named something else",
    );
    // `ground.wgsl` stamps its own stance alongside the height, the same way
    // `statics.wgsl` always has — see `docs/lighting_raymarch.md`'s
    // ground-stance entry for why a land pixel that never named one read as
    // `Stance::Upright` to `blit.wgsl`'s own exemption logic instead. Said
    // through `place::packed_height` rather than as the literal this was
    // (`384`, which was `128 | STANCE_FLAT << 8`): a literal here pins the
    // *layout* as much as the value, and the layout has since moved once, for
    // `docs/lighting_height.md` phase 1's fraction.
    assert_eq!(
        ground_pixel[2],
        openshard_client_render::place::packed_height(0.0, openshard_client_render::place::Stance::Flat),
        "and another height",
    );
    assert_eq!(ground_pixel[3] & 3, 1, "and another kind");
    // And the ground's fraction of its tile moves with the pixel, which is what
    // the lighting reads to make a pool a gradient rather than a set of flat
    // tiles. Two pixels apart on the screen are two different places in a tile.
    let beside = places.at(70, 84);
    assert_ne!(
        beside[3] >> 2,
        ground_pixel[3] >> 2,
        "a pixel six across is at the same place in its tile",
    );
    // And a corner nothing was drawn on stays the clear value, whose kind is
    // `Nothing` — a background the lighting must leave alone.
    assert_eq!(places.at(2, 2)[3], 0, "an untouched pixel claimed a tile");
}

/// A floor's pixels are spread across its tile; a wall's run up it.
///
/// The two stances of `place::Stance`, and the reason there are two: a floor
/// static is a picture *of* the tile's diamond, so its fraction has to move in
/// both directions and its height must not move at all, while a wall is a
/// billboard whose picture is height and whose only horizontal information is
/// how far across the column a pixel is. Written because a floor lit as one flat
/// value per tile — which is what a constant fraction gives — is the blockiness
/// this stance exists to remove, and no other test here would see it.
#[test]
fn a_floor_spreads_across_its_tile_and_a_wall_stands_up_it() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const GRAPHIC: Graphic = Graphic(1);
    const SIDE: u16 = 44;
    const ORIGIN: f32 = 40.0;
    let red = Color16(0b0_11111_00000_00000);

    let land = LandAtlas::pack([]).expect("nothing always fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let statics = StaticAtlas::pack([(
        GRAPHIC,
        Image::new(SIDE, SIDE, vec![red; usize::from(SIDE) * usize::from(SIDE)]),
    )])
    .expect("fits");
    let sprite = statics.sprite(GRAPHIC).expect("packed");
    let quad = |place| SpriteQuad {
        rect: Rect {
            x: ORIGIN,
            y: ORIGIN,
            width: f32::from(sprite.width),
            height: f32::from(sprite.height),
        },
        region: sprite.region,
        depth: 0.4,
        hue: 0,
        place,
        twin: 0,
        owner: 0,
    };
    // The fraction of a tile a place holds, as the shaders pack it: seven bits
    // each, above the two the kind takes.
    let sub = |place: [u16; 4]| ((place[3] >> 2) & 127, (place[3] >> 9) & 127);
    // The third channel is a height *and* a stance — `crate::place::STANCE_SHIFT`
    // — so a test about heights has to say which part it means. The height is
    // both of its own fields together, which is what `unpacked_height` is for.
    let height = |place: [u16; 4]| openshard_client_render::place::unpacked_height(place[2]);
    let stance = |place: [u16; 4]| place[2] >> openshard_client_render::place::STANCE_SHIFT;

    let at = Point::new(301, 400, 15);
    let places = render_places(
        &device,
        &queue,
        &land,
        &texmaps,
        &[],
        &statics,
        &[quad(Place::of_floor(at))],
        &[],
        &[],
        128,
    );
    // The middle of the sprite is the middle of the tile, and the four
    // directions off it are the four world directions — a step right is further
    // along `x` and less along `y`, a step down is further along both.
    let middle = places.at(62, 62);
    let (mid_x, mid_y) = sub(middle);
    let (right_x, right_y) = sub(places.at(72, 62));
    let (below_x, below_y) = sub(places.at(62, 72));
    assert!(
        right_x > mid_x && right_y < mid_y,
        "right of the middle: {right_x} {right_y}"
    );
    assert!(
        below_x > mid_x && below_y > mid_y,
        "below the middle: {below_x} {below_y}"
    );
    // And a floor is at one height everywhere: what runs down its picture is the
    // tile, which the fraction has already spent.
    assert_eq!(
        [height(middle), height(places.at(62, 72))],
        [f32::from(at.z); 2],
        "a floor's pixels stand at different heights",
    );
    // And it says it is a floor, in the bits above the height. That the channel
    // carries a stance at all is what lets the lighting refuse to light a wall
    // from behind — see `crate::place::Stance` — and a floor's own value is what
    // says the bits are being written rather than left at zero.
    assert_eq!(
        stance(middle),
        openshard_client_render::place::Stance::Flat as u16,
        "a floor's pixel does not carry its stance",
    );

    let places = render_places(
        &device,
        &queue,
        &land,
        &texmaps,
        &[],
        &statics,
        &[quad(Place::of_static(at))],
        &[],
        &[],
        128,
    );
    // A wall claims the middle of its tile at every pixel, and deliberately: what
    // its picture runs along is the world axis the wall is built on, which in
    // this projection is a screen diagonal, and nothing in the tiledata says
    // which of the two axes it is. Spreading its pixels along the horizontal —
    // `x - y`, the one direction no wall runs — is the shape this asserts is not
    // being written.
    let (mid_x, mid_y) = sub(places.at(62, 62));
    assert_eq!(
        (mid_x, mid_y),
        (64, 64),
        "a wall's fraction is not its tile's middle"
    );
    assert_eq!(
        sub(places.at(72, 62)),
        (mid_x, mid_y),
        "it moved across the picture"
    );
    assert_eq!(
        sub(places.at(62, 72)),
        (mid_x, mid_y),
        "it moved down the picture"
    );
    // And its height is the picture's, which is the half of this the older test
    // covers — asserted here too so that the two stances are one comparison.
    assert!(
        places.at(62, 62)[2] > places.at(62, 72)[2],
        "a wall is not taller further up its picture",
    );
}

/// A ground pixel decodes to `Stance::Flat`, read the same direct way the two
/// sibling tests above read a floor static's and a corner's.
///
/// `every_pixel_names_the_tile_it_came_from` already pins a land pixel's third
/// channel to the literal `384` — which is `128 | (STANCE_FLAT << 8)`, but
/// nothing at that call site says so, so a reader (or a future stance value
/// shifting the packing) cannot tell height and stance apart in it without doing
/// the arithmetic by hand. `docs/lighting_raymarch.md`'s backlog calls this out
/// by name: the fixture where session 23's bug — `ground.wgsl` never stamping a
/// stance at all — shipped unnoticed, because no test decoded the stance bits on
/// their own and compared them against the enum. This is that test, using the
/// same `place::STANCE_SHIFT` decode as `a_floor_spreads_across_its_tile_and_a_
/// wall_stands_up_it`.
#[test]
fn a_ground_pixel_carries_its_own_stance() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const GRAPHIC: Graphic = Graphic(1);
    let green = Color16(0b0_00000_11111_00000);

    let side = usize::from(LAND_TILE_SIZE);
    let land = LandAtlas::pack([(
        GRAPHIC,
        Image::new(LAND_TILE_SIZE, LAND_TILE_SIZE, vec![green; side * side]),
    )])
    .expect("one sprite fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let statics = StaticAtlas::pack([]).expect("nothing always fits");
    let region = land.region(GRAPHIC).expect("packed");

    let ground = [GroundQuad {
        x: 64.0,
        y: 64.0,
        corners: [0.0; 4],
        region,
        texmap: None,
        depth: 0.6,
        place: Place::land(300, 400),
    }];
    let places = render_places(
        &device,
        &queue,
        &land,
        &texmaps,
        &ground,
        &statics,
        &[],
        &[],
        &[],
        128,
    );

    let stance = |x: u32, y: u32| {
        let place = places.at(x, y);
        assert_eq!(place[3] & 3, 1, "nothing was drawn at ({x}, {y})");
        place[2] >> openshard_client_render::place::STANCE_SHIFT
    };
    assert_eq!(
        stance(64, 64),
        openshard_client_render::place::Stance::Flat as u16,
        "a ground pixel does not carry its stance",
    );
}

/// Two wall tiles in a row are one surface, not two sprites.
///
/// The seam is the whole reason a wall's face is measured out of its art. Before
/// it, every pixel of a wall tile claimed the tile's middle: a row of walls came
/// out as flat 44-pixel bands with a step at each boundary, which is what a torch
/// against a wall actually looked like. With it, the fraction runs from 0 to 1
/// along the edge and the next tile picks up where this one stopped, so the world
/// coordinate a pixel names is *continuous across the join*.
///
/// Stated as a difference across the boundary and not as an absolute, because
/// that is the property that fails: a face read but mapped backwards, or a run
/// that saturates halfway, both put a wall in the right tiles and still break the
/// join. The pair of assertions below is what separates them — one that the step
/// across the seam is small, one that a whole tile is actually traversed, and a
/// mapping that returns a constant fails the second while passing the first.
#[test]
fn two_wall_tiles_in_a_row_name_one_continuous_surface() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const GRAPHIC: Graphic = Graphic(1);
    const HEIGHT: u16 = 60;
    // The south face runs along `+x`, so its neighbour is the tile at `x + 1` —
    // which in this projection is 22 pixels right and 22 down.
    const FACE: openshard_client_render::facing::Face = openshard_client_render::facing::Face::South;
    const ORIGIN: f32 = 20.0;

    let land = LandAtlas::pack([]).expect("nothing always fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let statics = StaticAtlas::pack([(GRAPHIC, openshard_client_render::facing::silhouette(FACE, HEIGHT))])
        .expect("fits");
    let sprite = statics.sprite(GRAPHIC).expect("packed");
    // The atlas is what measures the face, and if it did not this test would go
    // on to assert about two `Upright` sprites and pass for the wrong reason —
    // their fractions are equal, so the step across the seam would be zero.
    assert_eq!(
        sprite.facing,
        Some(openshard_client_render::facing::Facing::One(FACE)),
        "the atlas did not read the fixture",
    );

    let tile = |x: u16| Point::new(x, 400, 0);
    let quad = |at: Point, dx: f32, dy: f32| SpriteQuad {
        rect: Rect {
            x: ORIGIN + dx,
            y: ORIGIN + dy,
            width: f32::from(sprite.width),
            height: f32::from(sprite.height),
        },
        region: sprite.region,
        depth: 0.4,
        hue: 0,
        place: Place {
            stance: openshard_client_render::place::Stance::FaceSouth,
            ..Place::of_static(at)
        },
        twin: 0,
        owner: 0,
    };
    let places = render_places(
        &device,
        &queue,
        &land,
        &texmaps,
        &[],
        &statics,
        &[quad(tile(300), 0.0, 0.0), quad(tile(301), 22.0, 22.0)],
        &[],
        &[],
        256,
    );

    // Where in the world a pixel says it is: the tile plus the fraction, in
    // tiles. This is exactly what `blit.wgsl` computes to measure a distance,
    // which is why the assertions below are about it rather than about the bits.
    let world_x = |x: u32, y: u32| {
        let place = places.at(x, y);
        assert_eq!(place[3] & 3, 2, "nothing was drawn at ({x}, {y})");
        f32::from(place[0]) + f32::from((place[3] >> 2) & 127) / 127.0
    };

    // A row that crosses the join. The first sprite's face occupies its left
    // half — columns 0..=21 of a 44-wide picture — and the second sprite starts
    // 22 pixels right of it, so the two are edge to edge with no gap and no
    // overlap. A row a third of the way up the wall is drawn in every one of
    // those columns.
    let row = ORIGIN as u32 + u32::from(HEIGHT);
    let left = ORIGIN as u32;
    let last_of_the_first = world_x(left + 21, row);
    let first_of_the_second = world_x(left + 22, row + 1);
    assert!(
        (first_of_the_second - last_of_the_first).abs() < 0.15,
        "the seam steps: {last_of_the_first} then {first_of_the_second}",
    );
    // And a whole tile is crossed getting there, which is what says the fraction
    // is a mapping and not a constant somewhere near the middle.
    let first_of_the_first = world_x(left, row - 21);
    assert!(
        (last_of_the_first - first_of_the_first) > 0.85,
        "one tile of wall spans {} of a tile",
        last_of_the_first - first_of_the_first,
    );
    // The fixed coordinate is the edge, one step of the fraction short of it —
    // and both halves of that are load-bearing.
    //
    // *The edge*, because a south face lies on `y + 1`: a fraction that drifted
    // off it would put the lit surface inside the tile rather than on its
    // boundary, which the two assertions above cannot see because both only ever
    // look at `x`.
    //
    // *One step short*, because `blit.wgsl` finds a fragment's cell with
    // `floor(tile + fraction)` and exempts that cell from shadowing it. A clean
    // `127` names the tile **beyond** the wall, so the wall's own tile stops
    // being exempt and the wall is shadowed by itself — measured on Britain, a
    // run of lit wall at 249 dropping to the ambient 65. `statics.wgsl`'s
    // `INSIDE` is the step, and this is the number it produces.
    // Stated as the two facts rather than as the byte, so that a change to how
    // many bits the fraction has does not silently retire either of them.
    for (x, y) in [(left, row - 21), (left + 21, row), (left + 22, row + 1)] {
        let place = places.at(x, y);
        let (sub_x, sub_y) = ((place[3] >> 2) & 127, (place[3] >> 9) & 127);
        assert!(
            sub_y > 120,
            "the south face left its own edge at ({x}, {y}): {sub_y}"
        );
        // The cell `blit.wgsl` will call this fragment's own, computed its way.
        for (tile, sub, axis) in [(place[0], sub_x, 'x'), (place[1], sub_y, 'y')] {
            let cell = (f32::from(tile) + f32::from(sub) / 127.0).floor() as u16;
            assert_eq!(
                cell, tile,
                "at ({x}, {y}) the walk puts this pixel in the {axis} neighbour, so the wall it is \
                 the face of is no longer exempt from shadowing it",
            );
        }
    }
}

/// A corner's two faces are two halves of one picture, and a pixel belongs to
/// the one it is drawn on.
///
/// The whole of decision 25 on the GPU side: `statics.wgsl` resolves a corner per
/// fragment, so what reaches the attachment is a single face with a single
/// outward normal and `blit.wgsl` never learns that corners exist. Before it, a
/// corner was `Upright` — every pixel of it claiming the middle of its tile, a
/// flat 44-pixel band between two continuous runs of wall, lit identically on the
/// side turned towards the flame and the side turned away.
///
/// Asserted as the *two halves disagreeing*, which is the property that fails:
/// a corner resolved to one face for the whole tile draws a wall along an axis
/// half of it does not run on, and every pixel would still carry a face and a
/// fraction that moves. Only comparing the halves says which face is where.
#[test]
fn a_corner_s_pixel_carries_the_face_of_the_half_it_is_drawn_on() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    use openshard_client_render::facing::{Face, Facing};
    use openshard_client_render::place::Stance;

    const GRAPHIC: Graphic = Graphic(1);
    const HEIGHT: u16 = 60;
    const ORIGIN: f32 = 20.0;
    // The pair a camera can see, which is what every corner the client ships is:
    // the east face on the right half of the picture, the south face on the left.
    const RIGHT: Face = Face::East;
    const LEFT: Face = Face::South;

    let land = LandAtlas::pack([]).expect("nothing always fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let statics = StaticAtlas::pack([(
        GRAPHIC,
        openshard_client_render::facing::corner_silhouette(RIGHT, LEFT, HEIGHT),
    )])
    .expect("fits");
    let sprite = statics.sprite(GRAPHIC).expect("packed");
    // The atlas is what measures it, and without this the test would go on to
    // assert about an `Upright` sprite whose halves agree — and pass by drawing
    // exactly the artefact it is here to catch.
    assert_eq!(
        sprite.facing,
        Some(Facing::Corner {
            right: RIGHT,
            left: LEFT
        }),
        "the atlas did not read the fixture as a corner",
    );

    let at = Point::new(300, 400, 0);
    let places = render_places(
        &device,
        &queue,
        &land,
        &texmaps,
        &[],
        &statics,
        &[SpriteQuad {
            rect: Rect {
                x: ORIGIN,
                y: ORIGIN,
                width: f32::from(sprite.width),
                height: f32::from(sprite.height),
            },
            region: sprite.region,
            depth: 0.4,
            hue: 0,
            place: Place {
                stance: Stance::of(&openshard_uofiles::tiledata::StaticTile::default(), sprite.facing),
                ..Place::of_static(at)
            },
            twin: 0,
            owner: 0,
        }],
        &[],
        &[],
        256,
    );

    // A row a third of the way up the picture, one pixel either side of the
    // column the sprite is centred on: the two halves of the same corner, as
    // close together as the picture allows.
    let row = ORIGIN as u32 + u32::from(HEIGHT);
    let middle = ORIGIN as u32 + 22;
    let stance = |x: u32, y: u32| {
        let place = places.at(x, y);
        assert_eq!(place[3] & 3, 2, "nothing was drawn at ({x}, {y})");
        place[2] >> openshard_client_render::place::STANCE_SHIFT
    };
    assert_eq!(
        stance(middle + 4, row),
        Stance::FaceEast as u16,
        "the right half of the corner is not its east face",
    );
    assert_eq!(
        stance(middle - 4, row),
        Stance::FaceSouth as u16,
        "the left half of the corner is not its south face",
    );

    // And the fraction each half carries is its own face's: an east face lies on
    // `x + 1` and a south face on `y + 1`, so the two halves are two different
    // surfaces of one tile and not one surface read twice. Compared as the
    // *fixed* coordinate of each, because that is what a face is — the run along
    // the edge moves in both.
    let sub = |x: u32, y: u32| {
        let place = places.at(x, y);
        ((place[3] >> 2) & 127, (place[3] >> 9) & 127)
    };
    let (right_x, _) = sub(middle + 4, row);
    let (_, left_y) = sub(middle - 4, row);
    assert!(right_x > 120, "the east half left its own edge: {right_x}");
    assert!(left_y > 120, "the south half left its own edge: {left_y}");

    // And the two halves are two different rows, not one instance's id read
    // twice — `docs/gbuffer.md` step 4. This frame drew exactly one corner,
    // so `split_corners` gives it id `0` and a shadow row at id `1`: the
    // right half (its own instance) keeps `0`, the left half (the diagonal
    // test's other side) takes the shadow's `1`.
    let id = |x: u32, y: u32| {
        let place = places.at(x, y);
        u32::from(place[0]) | (u32::from(place[1]) << 16)
    };
    assert_eq!(id(middle + 4, row), 0, "the right half is not the drawn instance");
    assert_eq!(id(middle - 4, row), 1, "the left half is not its shadow row");
}

/// A mesh-face fragment's own `place.z` carries the routing sentinel,
/// `Stance::MeshFace`, decoded the same direct way as the two sibling tests
/// above — not the real face this fragment stands on, which
/// `MeshFaceRow::stance` carries instead, read back through `blit.wgsl`'s
/// `mesh_instances` storage buffer, a different consumer than this
/// attachment.
///
/// `docs/lighting_raymarch.md`'s backlog names this the second of two
/// producers with no direct pixel-decode coverage, and the one that needed
/// real plumbing: unlike ground and statics, `render_places` never drove the
/// mesh-face pass before this — it does now, wired in right after the statics
/// pass, the same order `crates/client/app/src/lib.rs`'s real frame runs it
/// in.
#[test]
fn a_mesh_face_pixel_carries_the_mesh_face_sentinel() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    use openshard_client_render::mesh_face::{MeshFaceRow, MeshFaceVertex};
    use openshard_client_render::place::Stance;

    let land = LandAtlas::pack([]).expect("nothing always fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let statics = StaticAtlas::pack([]).expect("nothing always fits");

    // One flat quad, two triangles, standing over a tile at height 15 — the
    // exact position within the tile is not what this test is about, only
    // that a fragment lands and carries the sentinel.
    let tile = [300.0, 400.0];
    let world = [tile[0] + 0.5, tile[1] + 0.5, 15.0];
    let corner = |x: f32, y: f32| MeshFaceVertex {
        screen: Vec2::new(x, y),
        world,
        depth: 0.4,
        id: 0,
        tile,
    };
    let vertices = [
        corner(54.0, 54.0),
        corner(74.0, 54.0),
        corner(74.0, 74.0),
        corner(54.0, 54.0),
        corner(74.0, 74.0),
        corner(54.0, 74.0),
    ];
    let rows = [MeshFaceRow {
        tile: (300, 400),
        stance: Stance::FaceEast,
        owner: 0,
    }];

    let places = render_places(
        &device,
        &queue,
        &land,
        &texmaps,
        &[],
        &statics,
        &[],
        &vertices,
        &rows,
        128,
    );
    let place = places.at(64, 64);
    assert_eq!(place[3] & 3, 2, "nothing was drawn at (64, 64)");
    assert_eq!(
        place[2] >> openshard_client_render::place::STANCE_SHIFT,
        Stance::MeshFace as u16,
        "a mesh-face pixel does not carry the mesh-face sentinel",
    );
}

/// Draw ground, statics and any mesh faces standing on them, and read back the
/// *place* attachment rather than the picture. `size * 8` must be a multiple
/// of 256, as every readback here.
#[allow(clippy::too_many_arguments)]
fn render_places(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    atlas: &LandAtlas,
    texmaps: &TexmapAtlas,
    quads: &[GroundQuad],
    static_atlas: &StaticAtlas,
    static_quads: &[SpriteQuad],
    mesh_vertices: &[openshard_client_render::mesh_face::MeshFaceVertex],
    mesh_rows: &[openshard_client_render::mesh_face::MeshFaceRow],
    size: u32,
) -> Places {
    assert_eq!(size * 8 % 256, 0, "a row copy has to be 256-byte aligned");
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let world = openshard_client_render::blit::world_texture(device, size, size);
    let world_view = world.create_view(&wgpu::TextureViewDescriptor::default());
    let depth = renderer::depth_texture(device, size, size);
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
    let place = openshard_client_render::place::texture(device, size, size);
    let place_view = place.create_view(&wgpu::TextureViewDescriptor::default());
    let hue_ramp = HueRamp::build(&Hues::parse(&[0u8; 708]).expect("one empty group"));

    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("places"),
        size: u64::from(size) * u64::from(size) * 8,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut ground_pass = GroundRenderer::new(device, queue, format, atlas, texmaps);
    let mut sprite_pass = SpriteRenderer::new(device, queue, format, static_atlas.pixels(), &hue_ramp);
    let mut mesh_pass = renderer::MeshFaceRenderer::new(device);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let target = Target::whole(&world_view, &depth_view, &place_view, size, size);
    ground_pass.render(device, queue, &mut encoder, target, quads);
    // A corner static's two faces get their own id here too, the same as the
    // real pass — see `sprite::split_corners`'s own doc and
    // `docs/gbuffer.md` step 4.
    let instances = openshard_client_render::sprite::split_corners(static_quads.to_vec());
    sprite_pass.render(
        device,
        queue,
        &mut encoder,
        target,
        &instances.rows,
        Some(instances.drawn),
    );
    // Right after statics, into the same static's own pixels — the real
    // renderer's own order (`docs/gbuffer.md` step 4c), so depth and place
    // only ever tie or improve on what the billboard sprite just wrote.
    mesh_pass.render(device, queue, &mut encoder, target, mesh_vertices, mesh_rows);
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &place,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size * 8),
                rows_per_image: Some(size),
            },
        },
        wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);

    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |result| {
        result.expect("mapping a buffer this test just wrote");
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("waiting on our own submission");
    let bytes = slice
        .get_mapped_range()
        .expect("the map completed above")
        .to_vec();
    readback.unmap();
    Places { width: size, bytes }
}

/// The place attachment read back: four `u16` channels a texel.
struct Places {
    width: u32,
    bytes: Vec<u8>,
}

impl Places {
    /// `(x, y, z + 128, kind)` at one pixel.
    fn at(&self, x: u32, y: u32) -> [u16; 4] {
        let start = ((y * self.width + x) * 8) as usize;
        let mut out = [0u16; 4];
        for (channel, slot) in out.iter_mut().enumerate() {
            let at = start + channel * 2;
            *slot = u16::from_le_bytes([self.bytes[at], self.bytes[at + 1]]);
        }
        out
    }
}

/// A hill in front hides a wall behind it.
///
/// The assertion the depth buffer exists for, and the one no pass order can
/// satisfy: all the ground is drawn before any static, so without a shared
/// depth every static would be in front of every tile. Both quads are built
/// here rather than read from a map, because what is being checked is the
/// *ordering*, and a real hillside would decide the geometry as well.
#[test]
fn ground_in_front_hides_a_static_behind_it() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const GRAPHIC: Graphic = Graphic(1);
    let green = Color16(0b0_00000_11111_00000);
    let red = Color16(0b0_11111_00000_00000);

    let side = usize::from(LAND_TILE_SIZE);
    let land = LandAtlas::pack([(
        GRAPHIC,
        Image::new(LAND_TILE_SIZE, LAND_TILE_SIZE, vec![green; side * side]),
    )])
    .expect("one sprite fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let statics = StaticAtlas::pack([(GRAPHIC, Image::new(60, 60, vec![red; 60 * 60]))]).expect("fits");
    let region = land.region(GRAPHIC).expect("packed");
    let sprite = statics.sprite(GRAPHIC).expect("packed");

    // Same pixels on screen, and the ground is nearer. A wall standing behind
    // a hill: the sprite's rectangle covers the tile's diamond entirely, so
    // every pixel of the diamond is a pixel both quads want.
    let ground = [GroundQuad {
        x: 64.0,
        y: 64.0,
        corners: [0.0; 4],
        region,
        texmap: None,
        depth: 0.4,
        place: Place::land(1, 1),
    }];
    let wall = [SpriteQuad {
        rect: Rect {
            x: 64.0 - 30.0,
            y: 64.0 - 30.0,
            width: f32::from(sprite.width),
            height: f32::from(sprite.height),
        },
        region: sprite.region,
        depth: 0.6,
        hue: 0,
        place: Place::NOWHERE,
        twin: 0,
        owner: 0,
    }];
    let none = AnimAtlas::pack([]).expect("nothing always fits");
    let frame = render_both(
        &device,
        &queue,
        &land,
        &texmaps,
        &ground,
        &statics,
        &wall,
        (none.pixels(), &[]),
        128,
        128,
        Projection::one_to_one(128, 128),
    );

    let (green_r, green_g, green_b) = green.rgb8();
    let mut ground_pixels = 0;
    for y in 0..128u32 {
        for x in 0..128u32 {
            if frame.pixel(x, y) == [green_r, green_g, green_b, u8::MAX] {
                ground_pixels += 1;
            }
        }
    }
    // The diamond, whole: not one of its pixels was overwritten by the static
    // that came after it.
    assert_eq!(ground_pixels, 1012, "the wall drew over the hill in front of it");

    // And the reverse ordering does the opposite, or the assertion above is
    // satisfied by a statics pass that draws nothing at all.
    let front = [SpriteQuad {
        depth: 0.2,
        ..wall[0]
    }];
    let frame = render_both(
        &device,
        &queue,
        &land,
        &texmaps,
        &ground,
        &statics,
        &front,
        (none.pixels(), &[]),
        128,
        128,
        Projection::one_to_one(128, 128),
    );
    let covered = (0..128u32)
        .flat_map(|y| (0..128u32).map(move |x| (x, y)))
        .filter(|&(x, y)| frame.pixel(x, y) == [green_r, green_g, green_b, u8::MAX])
        .count();
    assert_eq!(covered, 0, "a static in front left ground showing through");
}

/// Two things at one depth are decided by which is drawn later, and that is the
/// client's own tie-break rather than an accident of the pass order.
///
/// `Chunk.AddGameObject` inserts by `PriorityZ` and, on a tie, puts the land
/// tile *first* in the per-tile list — so the flagstone lying at exactly the
/// height of the ground under it is drawn second, and covers it. Here that is
/// `LessEqual` in `renderer::depth_state` plus the order the passes already
/// run in: the ground pass, then the statics, then the mobiles.
///
/// It needs a frame because the depth *state* is what is being asserted. Every
/// number this crate computes can be right and this still be backwards, and
/// under `Less` it was: the depths agreed with the client and the first writer
/// kept the pixel, so the ground won every tie it should have lost.
#[test]
fn at_one_depth_the_later_pass_wins() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const GRAPHIC: Graphic = Graphic(1);
    let green = Color16(0b0_00000_11111_00000);
    let red = Color16(0b0_11111_00000_00000);

    let side = usize::from(LAND_TILE_SIZE);
    let land = LandAtlas::pack([(
        GRAPHIC,
        Image::new(LAND_TILE_SIZE, LAND_TILE_SIZE, vec![green; side * side]),
    )])
    .expect("one sprite fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let statics = StaticAtlas::pack([(GRAPHIC, Image::new(60, 60, vec![red; 60 * 60]))]).expect("fits");
    let region = land.region(GRAPHIC).expect("packed");
    let sprite = statics.sprite(GRAPHIC).expect("packed");

    // The same depth, to the bit: not "very close", which the test would pass
    // under either comparison.
    const TIED: f32 = 0.5;
    let ground = [GroundQuad {
        x: 64.0,
        y: 64.0,
        corners: [0.0; 4],
        region,
        texmap: None,
        depth: TIED,
        place: Place::land(1, 1),
    }];
    let flagstone = [SpriteQuad {
        rect: Rect {
            x: 64.0 - 30.0,
            y: 64.0 - 30.0,
            width: f32::from(sprite.width),
            height: f32::from(sprite.height),
        },
        region: sprite.region,
        depth: TIED,
        hue: 0,
        place: Place::NOWHERE,
        twin: 0,
        owner: 0,
    }];
    let none = AnimAtlas::pack([]).expect("nothing always fits");
    let frame = render_both(
        &device,
        &queue,
        &land,
        &texmaps,
        &ground,
        &statics,
        &flagstone,
        (none.pixels(), &[]),
        128,
        128,
        Projection::one_to_one(128, 128),
    );

    let (green_r, green_g, green_b) = green.rgb8();
    let showing = (0..128u32)
        .flat_map(|y| (0..128u32).map(move |x| (x, y)))
        .filter(|&(x, y)| frame.pixel(x, y) == [green_r, green_g, green_b, u8::MAX])
        .count();
    assert_eq!(showing, 0, "the ground kept a pixel from the static tied with it");

    // And the static really covered those pixels rather than the frame being
    // empty: the sprite's whole rectangle is its own colour.
    let (red_r, red_g, red_b) = red.rgb8();
    let covered = (0..128u32)
        .flat_map(|y| (0..128u32).map(move |x| (x, y)))
        .filter(|&(x, y)| frame.pixel(x, y) == [red_r, red_g, red_b, u8::MAX])
        .count();
    assert_eq!(covered, 60 * 60, "the static did not draw its whole rectangle");
}

/// A mobile is drawn from its own atlas, in front of the ground it stands on,
/// and a mirrored facing is the same picture backwards.
///
/// The mirror is what needs a frame rather than an assertion on numbers: the
/// region arithmetic is checked in `sprite`, but whether a *negative* region
/// width actually samples backwards is the GPU's answer and not ours. A shader
/// that clamped it instead would leave every west-facing creature looking east,
/// which is a bug a screenshot of one direction cannot show.
#[test]
fn a_mobile_is_drawn_over_the_ground_and_mirrors_with_its_facing() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const BODY: u16 = 400;
    let green = Color16(0b0_00000_11111_00000);
    let red = Color16(0b0_11111_00000_00000);

    // A two-pixel-wide frame: red on the left, green on the right. Mirrored,
    // the two swap, and nothing else about the quad changes.
    let frame = AnimFrame {
        center_x: 1,
        center_y: 0,
        image: Image::new(2, 1, vec![red, green]),
    };
    let atlas = AnimAtlas::pack([(
        FrameKey {
            body: BODY,
            group: 4,
            direction: 1,
            frame: 0,
        },
        frame,
    )])
    .expect("one frame fits");

    // Ground under it, at the same tile: the mobile has to win, and the ground
    // is what makes that a claim rather than a drawing on an empty frame.
    let side = usize::from(LAND_TILE_SIZE);
    let blue = Color16(0b0_00000_00000_11111);
    let land = LandAtlas::pack([(
        Graphic(1),
        Image::new(LAND_TILE_SIZE, LAND_TILE_SIZE, vec![blue; side * side]),
    )])
    .expect("one sprite fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let statics = StaticAtlas::pack([]).expect("nothing always fits");
    let centre = Point::new(100, 100, 0);
    let camera = Camera::new(centre, 256, 256);

    // The ground quad is built here rather than collected: `Map` cannot be
    // constructed in memory — see the backlog in docs/client.md — and what this
    // test needs is one tile under the mobile's feet at the depth `depth` would
    // have given it.
    let at = camera.to_screen(centre);
    let ground = [GroundQuad {
        x: at.x as f32,
        y: at.y as f32,
        corners: [0.0; 4],
        region: land.region(Graphic(1)).expect("packed"),
        texmap: None,
        depth: openshard_client_render::depth::Order {
            tile: 200,
            priority_z: openshard_client_render::depth::land_priority_z([0; 4]),
        }
        .to_depth(openshard_client_render::depth::base_for(100, 100)),
        place: Place::land(100, 100),
    }];

    let colours = |facing| {
        let quads = mobiles::collect(
            &[Mobile {
                at: centre,
                body: BODY,
                group: 4,
                facing,
                frame: 0,
                from: None,
                hue: openshard_protocol::wire::Hue::NONE,
                drawn: openshard_client_render::follow::Gaze::on(centre),
                equipment: Vec::new(),
            }],
            &camera,
            &atlas,
            &Cutaway::OPEN,
            &EquipConv::default(),
            None,
        );
        assert_eq!(quads.len(), 1, "the frame is packed, so it draws");
        let frame = render_both(
            &device,
            &queue,
            &land,
            &texmaps,
            &ground,
            &statics,
            &[],
            (atlas.pixels(), &quads),
            256,
            256,
            Projection::one_to_one(256, 256),
        );
        // The two pixels the sprite covers, left and right.
        let x = quads[0].rect.x as u32;
        let y = quads[0].rect.y as u32;
        (frame.pixel(x, y), frame.pixel(x + 1, y))
    };

    let (red_r, red_g, red_b) = red.rgb8();
    let (green_r, green_g, green_b) = green.rgb8();
    // South is stored direction 1 unflipped, East is the same picture mirrored.
    assert_eq!(
        colours(Direction::South),
        (
            [red_r, red_g, red_b, u8::MAX],
            [green_r, green_g, green_b, u8::MAX]
        ),
        "the mobile is not drawn over the ground, or not from its own atlas",
    );
    assert_eq!(
        colours(Direction::East),
        (
            [green_r, green_g, green_b, u8::MAX],
            [red_r, red_g, red_b, u8::MAX]
        ),
        "a mirrored facing drew the picture the same way round",
    );
}

/// The same camera twice is the same bytes.
///
/// Determinism is not a nicety here: it is what makes every other assertion in
/// this file reproducible, and the ordering it depends on — the sort in
/// `ground::collect`, the `BTreeSet` in the atlas — is easy to lose to a
/// `HashMap` in a later change that looks harmless.
#[test]
fn the_same_camera_renders_the_same_frame() {
    let (Some(dir), Some((device, queue))) = (client_dir(), gpu()) else {
        return;
    };
    let map = Map::load_facet(&dir, 0).expect("Felucca");
    let art = Art::open(&dir).expect("artLegacyMUL.uop");
    let camera = Camera::new(Point::new(1495, 1629, 0), 768, 512);

    let mut frames = Vec::new();
    for _ in 0..2 {
        let wanted = ground::visible_graphics(&map, &camera);
        let atlas = LandAtlas::build(&art, wanted.iter().copied()).expect("fits");
        let texmaps = texmap_atlas(&dir, wanted);
        let quads = ground::collect(&map, &camera, &atlas, &texmaps, &Cutaway::OPEN);
        frames.push(
            render(
                &device,
                &queue,
                &atlas,
                &texmaps,
                &quads,
                camera.width,
                camera.height,
            )
            .pixels,
        );
    }
    assert_eq!(frames[0], frames[1]);

    // And a different camera is a different frame — otherwise the assertion
    // above is satisfied by a renderer that draws nothing at all.
    let moved = Camera::new(Point::new(1497, 1629, 0), 768, 512);
    let wanted = ground::visible_graphics(&map, &moved);
    let atlas = LandAtlas::build(&art, wanted.iter().copied()).expect("fits");
    let texmaps = texmap_atlas(&dir, wanted);
    let quads = ground::collect(&map, &moved, &atlas, &texmaps, &Cutaway::OPEN);
    let other = render(
        &device,
        &queue,
        &atlas,
        &texmaps,
        &quads,
        moved.width,
        moved.height,
    );
    assert_ne!(frames[0], other.pixels, "moving the camera changed nothing");
}

/// The gate `docs/camera.md` D11 asks for, on the GPU: magnified, moving the
/// eye by `1/zoom` of a virtual pixel moves the picture by exactly one real one.
///
/// Everything else about D11 is arithmetic that can be asserted without a
/// device. This is the claim that cannot: that the shader's last two lines,
/// the rasteriser and `nearest` sampling together produce a frame that is the
/// other frame *translated*, rather than one resampled by a fraction of a texel.
/// The second is what a magnification usually costs, it looks like a slight
/// change in the art, and no arithmetic in `camera.rs` would notice it.
///
/// Two cameras a third of a virtual pixel apart at `3x`, which is one real pixel
/// and is the finest step the display has. The quads are built once and shared
/// deliberately: `to_view` measures from the eye *rounded*, and both eyes round
/// to the same virtual pixel, so the only difference between the two frames is
/// `Projection::origin` — which is exactly the claim.
///
/// A third and not a half, because a half is the one fraction that could be
/// right for the wrong reason: it is on the lattice of `2x` as well, so a
/// rounding that quietly went to the nearest *even* real pixel would pass it.
#[test]
fn a_third_of_a_virtual_pixel_moves_a_magnified_frame_one_real_pixel() {
    let (Some(dir), Some((device, queue))) = (client_dir(), gpu()) else {
        return;
    };
    let map = Map::load_facet(&dir, 0).expect("Felucca");
    let art = Art::open(&dir).expect("artLegacyMUL.uop");

    let mut camera = Camera::new(Point::new(1495, 1629, 0), 512, 256);
    let mut zoom = Zoom::ONE;
    for _ in 0..2 {
        zoom = zoom.scale_up();
    }
    camera.zoom_about(256, 128, zoom);
    assert_eq!(camera.zoom().to_string(), "3x", "the rung this test is about");
    assert!(
        !camera.minifies(),
        "and the world is drawn at the display's own size"
    );

    let wanted = ground::visible_graphics(&map, &camera);
    let land = LandAtlas::build(&art, wanted.iter().copied()).expect("fits");
    let texmaps = texmap_atlas(&dir, wanted);
    let quads = ground::collect(&map, &camera, &land, &texmaps, &Cutaway::OPEN);

    // Along `x` only: a diagonal move would pass on a frame shifted the right
    // distance the wrong way round, and the two axes are separate lines of
    // shader.
    let mut shifted = camera;
    let at = camera.eye_at();
    shifted.look_at(WorldPoint {
        x: at.x + camera.quantum(),
        y: at.y,
    });
    assert_eq!(shifted.eye(), camera.eye(), "the same whole virtual pixel");
    assert_ne!(shifted.projection(), camera.projection(), "and a different frame");

    let (width, height) = (camera.width, camera.height);
    let before = render_projected(&device, &queue, &land, &texmaps, &quads, width, height, camera);
    let after = render_projected(&device, &queue, &land, &texmaps, &quads, width, height, shifted);
    assert_ne!(before.pixels, after.pixels, "a real pixel moved nothing at all");

    // The eye moved right, so the world moved left: what is at `x` in the second
    // frame was at `x + 1` in the first. Compared over the interior, because the
    // column the shift walks in from has no counterpart to be compared with.
    let mut checked = 0usize;
    let mut moved = 0usize;
    let mut resampled = 0usize;
    for y in 0..height {
        for x in 0..width - 1 {
            checked += 1;
            if after.pixel(x, y) == before.pixel(x + 1, y) {
                moved += 1;
            } else {
                resampled += 1;
            }
        }
    }
    // Counted and asserted, because "every pixel matched" and "no pixel was
    // looked at" are the same green — this repository has produced the second
    // one before.
    assert_eq!(checked, (width as usize - 1) * height as usize);

    // What is *not* an exact translation, and why it cannot be. A sloped tile is
    // textured by stretching a square texmap over a diamond, so its `uv` is
    // interpolated across a quad that is not axis-aligned and a fragment centre
    // a third of a texel along lands on the other side of a texel boundary here
    // and there. There is no placement of the quantiser that fixes that: it is
    // what stretching a texture means. Everything drawn from the *art* — flat
    // ground, statics, sprites — is texel-aligned and translates exactly, which
    // is what the sprite half of this gate below asserts with no allowance at
    // all.
    //
    // One in a thousand is a ceiling and not a measurement (it is one in seven
    // thousand over Britain), and the mutation is what says a ceiling is enough:
    // an origin that rounds its fraction away draws the *same* frame twice, so
    // the number this is separating a correct camera from is not 1 in 7,000 but
    // 130,815 in 130,816.
    assert!(
        resampled * 1000 < checked,
        "{resampled} of {checked} pixels are not the frame before it, translated",
    );
    assert!(
        moved > checked / 2,
        "a frame that agreed nowhere is not a translation"
    );

    // And the guard against all of that holding vacuously. Comparing the two
    // frames *without* the translation is not a good enough test on its own —
    // ground is large flat regions of colour, so more than half the pixels have
    // the same value one pixel over regardless — so what is asserted is that the
    // translation explains strictly more of the frame than standing still does.
    // Under the mutation the two are equal, which is what makes this the
    // discriminating comparison rather than a restatement of the one above.
    let still = (0..height)
        .flat_map(|y| (0..width - 1).map(move |x| (x, y)))
        .filter(|&(x, y)| after.pixel(x, y) == before.pixel(x, y))
        .count();
    assert!(
        moved > still,
        "translating explains {moved} pixels and standing still explains {still}",
    );
}

/// And the half of that gate with no allowance in it: a *sprite* at `3x`,
/// shifted a third of a virtual pixel, is the same picture one real pixel over.
///
/// Everything drawn from the art rather than from a texmap is texel-aligned —
/// the quad is the sprite's own rectangle, so a fragment centre lands on a
/// texel centre at every magnification — and a translation of the quad by a
/// whole real pixel is therefore a translation of the picture, exactly, with no
/// resampling anywhere. This is the claim the character's own smoothness rests
/// on, so it is asserted without a tolerance.
#[test]
fn a_magnified_sprite_translates_texel_for_texel() {
    let (Some(dir), Some((device, queue))) = (client_dir(), gpu()) else {
        return;
    };
    let art = Art::open(&dir).expect("artLegacyMUL.uop");

    let mut camera = Camera::new(Point::new(200, 200, 0), 512, 256);
    let mut zoom = Zoom::ONE;
    for _ in 0..2 {
        zoom = zoom.scale_up();
    }
    camera.zoom_about(256, 128, zoom);
    assert_eq!(camera.zoom().to_string(), "3x");

    // A static's sprite, drawn through the same pass a mobile uses: what is
    // being asserted is the pass and the transform, and a static is the one this
    // suite can build without an animation file.
    let graphic = Graphic(0x0CE3);
    let atlas = StaticAtlas::build(&art, [graphic]).expect("one sprite fits");
    let sprite = atlas.sprite(graphic).expect("just packed");
    let quads = vec![SpriteQuad {
        rect: Rect {
            x: (camera.render_width() as i32 / 2) as f32,
            y: (camera.render_height() as i32 / 2) as f32,
            width: f32::from(sprite.width),
            height: f32::from(sprite.height),
        },
        region: sprite.region,
        depth: 0.5,
        hue: 0,
        place: Place::NOWHERE,
        twin: 0,
        owner: 0,
    }];

    let mut shifted = camera;
    let at = camera.eye_at();
    shifted.look_at(WorldPoint {
        x: at.x + camera.quantum(),
        y: at.y,
    });

    let land = LandAtlas::build(&art, []).expect("nothing always fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let none = AnimAtlas::pack([]).expect("nothing always fits");
    let (width, height) = (camera.width, camera.height);
    let frame = |camera: Camera| {
        render_both(
            &device,
            &queue,
            &land,
            &texmaps,
            &[],
            &atlas,
            &quads,
            (none.pixels(), &[]),
            width,
            height,
            camera.projection(),
        )
    };
    let before = frame(camera);
    let after = frame(shifted);

    let mut drawn = 0usize;
    for y in 0..height {
        for x in 0..width - 1 {
            assert_eq!(
                after.pixel(x, y),
                before.pixel(x + 1, y),
                "({x}, {y}) is not the frame before it, translated",
            );
            if after.pixel(x, y)[3] != 0 {
                drawn += 1;
            }
        }
    }
    // The sprite has to actually be on screen, or the assertion above compared
    // a cleared frame with a cleared frame and passed for it.
    assert!(
        drawn > 1_000,
        "only {drawn} pixels of sprite: a blank frame agrees"
    );
}

/// A screen of Britain with its statics on it: the buildings cover a real part
/// of the frame, and the ground still covers all of it.
///
/// Two claims in one, and they are the two ways this layer fails as a whole.
/// Statics covering nothing means the sprites, the atlas or the placement
/// dropped everything and the frame is the old ground-only one; ground no
/// longer covering the viewport means the depth buffer or the second pass took
/// pixels away from it, which is a hole in the world rather than a wall.
#[test]
fn britains_statics_cover_part_of_a_frame_that_is_still_whole() {
    let (Some(dir), Some((device, queue))) = (client_dir(), gpu()) else {
        return;
    };
    let map = Map::load_facet(&dir, 0).expect("Felucca");
    let art = Art::open(&dir).expect("artLegacyMUL.uop");
    let tiledata = TileData::load(dir.join("tiledata.mul")).expect("tiledata.mul");
    let camera = Camera::new(Point::new(1495, 1629, 0), 768, 512);

    let wanted = ground::visible_graphics(&map, &camera);
    let land = LandAtlas::build(&art, wanted.iter().copied()).expect("fits");
    let texmaps = texmap_atlas(&dir, wanted);
    let quads = ground::collect(&map, &camera, &land, &texmaps, &Cutaway::OPEN);

    let wanted_statics = statics::visible_graphics(&map, &camera, &StaticAnimations::default());
    let static_atlas = StaticAtlas::build(&art, wanted_statics).expect("a screen of statics fits");
    let static_quads = statics::collect(
        &map,
        &camera,
        &tiledata,
        &StaticAnimations::default(),
        &static_atlas,
        &Cutaway::OPEN,
        &openshard_client_render::occlusion::Occlusion::EMPTY,
    )
    .quads;
    assert!(
        static_quads.len() > 500,
        "only {} statics in the middle of Britain",
        static_quads.len(),
    );

    let ground_only = render(
        &device,
        &queue,
        &land,
        &texmaps,
        &quads,
        camera.width,
        camera.height,
    );
    let none = AnimAtlas::pack([]).expect("nothing always fits");
    let frame = render_both(
        &device,
        &queue,
        &land,
        &texmaps,
        &quads,
        &static_atlas,
        &static_quads,
        (none.pixels(), &[]),
        camera.width,
        camera.height,
        camera.projection(),
    );

    // Still whole: every pixel drawn, exactly as with ground alone.
    let total = (camera.width * camera.height) as usize;
    assert_eq!(frame.drawn(), total, "the statics pass left holes in the world");

    // And a real part of it changed. A tenth is a floor rather than a
    // measurement — the point is that it is not a handful of pixels, which is
    // what a placement off by a tile or an atlas that packed nothing produces.
    let changed = (0..camera.height)
        .flat_map(|y| (0..camera.width).map(move |x| (x, y)))
        .filter(|&(x, y)| frame.pixel(x, y) != ground_only.pixel(x, y))
        .count();
    assert!(
        changed > total / 10,
        "the statics changed only {changed} of {total} pixels",
    );
}

/// Write a frame of Britain out as a picture, for a person to look at.
///
/// Ignored: it is not an assertion, it is the eye. Every other test here counts
/// pixels, and counting is what catches a sprite sampled one texel over — it is
/// not what catches ground that is the right shape and the wrong terrain. Run it
/// with a client and look:
///
/// ```sh
/// OPENSHARD_CLIENT=… cargo test -p openshard-client-render --test frame -- \
///     --ignored dump_a_frame
/// ```
///
/// PNG, through `crate::png` — the crate's own encoder, so nothing has to be
/// added to the workspace to write one.
#[test]
#[ignore = "writes a picture for a person, and asserts nothing"]
fn dump_a_frame_of_britain() {
    let (Some(dir), Some((device, queue))) = (client_dir(), gpu()) else {
        return;
    };
    let map = Map::load_facet(&dir, 0).expect("Felucca");
    let art = Art::open(&dir).expect("artLegacyMUL.uop");
    let centre = Point::new(1495, 1629, 0);
    let camera = Camera::new(centre, 768, 512);

    let tiledata = TileData::load(dir.join("tiledata.mul")).expect("tiledata.mul");
    let wanted = ground::visible_graphics(&map, &camera);
    let atlas = LandAtlas::build(&art, wanted.iter().copied()).expect("fits");
    let texmaps = texmap_atlas(&dir, wanted);
    let quads = ground::collect(&map, &camera, &atlas, &texmaps, &Cutaway::OPEN);

    let static_atlas = StaticAtlas::build(
        &art,
        statics::visible_graphics(&map, &camera, &StaticAnimations::default()),
    )
    .expect("statics fit");
    let static_quads = statics::collect(
        &map,
        &camera,
        &tiledata,
        &StaticAnimations::default(),
        &static_atlas,
        &Cutaway::OPEN,
        &openshard_client_render::occlusion::Occlusion::EMPTY,
    )
    .quads;

    // A character standing where the camera looks, facing each way in turn, so
    // the picture shows both the placement and the mirrored facings.
    let mut anim = Anim::open(&dir).expect("anim.idx and anim.mul");
    let people: Vec<Mobile> = Direction::ALL
        .iter()
        .enumerate()
        .map(|(index, facing)| {
            let (x, y) = (centre.x - 3 + index as u16 % 4, centre.y - 3 + index as u16 / 4);
            // On the ground rather than at the camera's height: a mobile
            // standing below the terrain is *correctly* hidden by it, which is
            // what the first run of this dump showed.
            //
            // The tile's average and not its stored corner, which is where a
            // body actually stands (`Map::average_land_z`): the corner is the
            // diamond's northern vertex, and on a slope standing at it is
            // standing under the floor — the ground sorts at that same average,
            // less two, so it is drawn over the body rather than beside it.
            let ground = Point::new(x, y, map.average_land_z(x, y).expect("inside the facet"));
            Mobile {
                at: ground,
                body: 400,
                group: 4,
                facing: *facing,
                frame: 0,
                // Standing, so there is no second tile to sort between.
                from: None,
                hue: openshard_protocol::wire::Hue::NONE,
                // Standing where the server put them: nothing here is walking.
                drawn: openshard_client_render::follow::Gaze::on(ground),
                equipment: Vec::new(),
            }
        })
        .collect();
    let equip_conv = EquipConv::default();
    let mobile_atlas =
        AnimAtlas::build(&mut anim, mobiles::needed_animations(&people, &equip_conv)).expect("a body fits");
    let mobile_quads = mobiles::collect(&people, &camera, &mobile_atlas, &Cutaway::OPEN, &equip_conv, None);

    let frame = render_both(
        &device,
        &queue,
        &atlas,
        &texmaps,
        &quads,
        &static_atlas,
        &static_quads,
        (mobile_atlas.pixels(), &mobile_quads),
        camera.width,
        camera.height,
        camera.projection(),
    );

    let path = std::env::var_os("OPENSHARD_FRAME_DUMP")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("britain.png"));
    std::fs::write(
        &path,
        openshard_client_render::png::encode_rgba(camera.width, camera.height, &frame.pixels),
    )
    .expect("writing the frame");
    eprintln!("wrote {}", path.display());
}

/// A sprite packed into an atlas *after* its renderer was built is drawn, and
/// drawn from its own pixels.
///
/// The load-bearing test for growing an atlas instead of rebuilding it. The
/// whole saving is that a growth uploads a band of rows rather than a 16MB
/// texture, and the band is the one thing in that arrangement with arithmetic in
/// it: `write_rows` cuts a slice out of the atlas and names a `y` to start it at,
/// and the two have to agree. If they do not, the sprite is drawn from whatever
/// the texture held there — which on a fresh atlas is transparent, so the
/// failure is a graphic that silently does not appear rather than one that
/// appears wrong.
///
/// The first sprite is 2,040 wide on purpose: it fills the shelf, so the second
/// starts a new row and the band has a non-zero origin. A band starting at zero
/// passes with the offset arithmetic missing entirely.
///
/// No client files: the pictures are this test's own.
#[test]
fn a_sprite_added_after_the_pass_was_built_is_drawn_from_the_rows_uploaded() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const SHELF_FILLER: Graphic = Graphic(1);
    const LATE: Graphic = Graphic(2);
    let (width, height) = (24u16, 18u16);
    let color = Color16(0b0_11111_00000_00000);

    let mut atlas = StaticAtlas::pack([(
        SHELF_FILLER,
        Image::new(2040, 40, vec![Color16(0b0_00000_11111_00000); 2040 * 40]),
    )])
    .expect("one wide sprite fits");

    let format = wgpu::TextureFormat::Rgba8Unorm;
    let hue_ramp = HueRamp::build(&Hues::parse(&[0u8; 708]).expect("one empty group"));
    // Built from the atlas as it stands, which is the point: the pass below is
    // never rebuilt, exactly as `client/app` no longer rebuilds it.
    let mut statics = SpriteRenderer::new(&device, &queue, format, atlas.pixels(), &hue_ramp);

    atlas
        .pack_more([(
            LATE,
            Image::new(
                width,
                height,
                vec![color; usize::from(width) * usize::from(height)],
            ),
        )])
        .expect("a second sprite fits");
    let rows = atlas.take_dirty().expect("the growth wrote something");
    assert!(
        rows.start > 0,
        "the second sprite should have started a new shelf"
    );
    statics.upload_rows(&queue, atlas.pixels(), rows);

    let sprite = atlas.sprite(LATE).expect("packed");
    let quads = [SpriteQuad {
        rect: Rect {
            x: 10.0,
            y: 12.0,
            width: f32::from(sprite.width),
            height: f32::from(sprite.height),
        },
        region: sprite.region,
        depth: 0.5,
        hue: 0,
        place: Place::NOWHERE,
        twin: 0,
        owner: 0,
    }];

    let (frame_width, frame_height) = (128u32, 128u32);
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("frame"),
        size: wgpu::Extent3d {
            width: frame_width,
            height: frame_height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let depth = renderer::depth_texture(&device, frame_width, frame_height);
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
    let place = openshard_client_render::place::texture(&device, frame_width, frame_height);
    let place_view = place.create_view(&wgpu::TextureViewDescriptor::default());
    // The ground pass clears; the sprite pass loads what it left. Given nothing
    // to draw, it is the clear on its own.
    let land = LandAtlas::pack([]).expect("nothing always fits");
    let texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    let mut ground = GroundRenderer::new(&device, &queue, format, &land, &texmaps);

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let target_view = Target::whole(&view, &depth_view, &place_view, frame_width, frame_height);
    ground.render(&device, &queue, &mut encoder, target_view, &[]);
    statics.render(&device, &queue, &mut encoder, target_view, &quads, None);
    queue.submit([encoder.finish()]);
    let frame = read_back(&device, &queue, &target);

    let (r, g, b) = color.rgb8();
    let mut drawn = 0;
    for y in 0..frame_height {
        for x in 0..frame_width {
            let inside =
                (10..10 + u32::from(width)).contains(&x) && (12..12 + u32::from(height)).contains(&y);
            let got = frame.pixel(x, y);
            if !inside {
                assert_eq!(got[3], 0, "({x}, {y}) is outside the sprite and was drawn");
                continue;
            }
            assert_eq!(
                got,
                [r, g, b, u8::MAX],
                "({x}, {y}) is not the late sprite's pixel"
            );
            drawn += 1;
        }
    }
    assert_eq!(drawn, usize::from(width) * usize::from(height));
}

/// Draw `quads` into a world image, ring the ones in `outlined`, blit the lot
/// onto a surface and read the surface back.
///
/// The whole outline pipeline in one helper, in the order the client runs it:
/// the picture, then the silhouette mask against the picture's own depth, then
/// the blit, then the ring over it. A test that skipped a step would be
/// asserting about a pipeline nothing draws.
/// `width` and `height` are the *world image*'s, as the client's are; the
/// surface comes out at `zoom` of them, which is where a minified ring is
/// point-sampled and can lose half of itself. See `Ring::for_zoom`.
#[allow(clippy::too_many_arguments)]
fn render_outlined(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    atlas: &StaticAtlas,
    quads: &[SpriteQuad],
    outlined: &[SpriteQuad],
    width: u32,
    height: u32,
    zoom: Zoom,
    ring: Ring,
) -> Frame {
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let world = openshard_client_render::blit::world_texture(device, width, height);
    let world_view = world.create_view(&wgpu::TextureViewDescriptor::default());
    let depth = renderer::depth_texture(device, width, height);
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
    let place = openshard_client_render::place::texture(device, width, height);
    let place_view = place.create_view(&wgpu::TextureViewDescriptor::default());
    let mask = outline::mask_texture(device, width, height);
    let mask_view = mask.create_view(&wgpu::TextureViewDescriptor::default());
    let hue_ramp = HueRamp::build(&Hues::parse(&[0u8; 708]).expect("one empty group"));

    let target = Target::whole(&world_view, &depth_view, &place_view, width, height);
    let empty_land = LandAtlas::pack([]).expect("nothing always fits");
    let empty_texmaps = TexmapAtlas::pack([]).expect("nothing always fits");
    // The ground pass with nothing in it, purely to clear the world image: it is
    // the pass that owns the clear, and a world texture nobody cleared holds
    // whatever the driver left there.
    let mut ground_pass = GroundRenderer::new(device, queue, format, &empty_land, &empty_texmaps);
    let mut sprites = SpriteRenderer::new(device, queue, format, atlas.pixels(), &hue_ramp);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    ground_pass.render(device, queue, &mut encoder, target, &[]);
    sprites.render(device, queue, &mut encoder, target, quads, None);
    // One quad, one ring — the item case, and what the tests below assert
    // about two sprites that touch.
    let rings: Vec<&[SpriteQuad]> = outlined.iter().map(std::slice::from_ref).collect();
    sprites.render_mask(device, queue, &mut encoder, target, &mask_view, &rings);

    let (surface_width, surface_height) = (
        width * zoom.numerator() / zoom.denominator(),
        height * zoom.numerator() / zoom.denominator(),
    );
    let surface = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("surface"),
        size: wgpu::Extent3d {
            width: surface_width,
            height: surface_height,
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
    let rect = ViewportRect {
        x: 0,
        y: 0,
        width: surface_width,
        height: surface_height,
    };
    // No mobile pass in this fixture: the dummy stands in for it.
    let dummy_instances = openshard_client_render::blit::dummy_instances(device);
    let dummy_mesh_instances = openshard_client_render::blit::dummy_mesh_instances(device);
    Blit::new(device, format).render(
        device,
        queue,
        &mut encoder,
        openshard_client_render::blit::Frame {
            target: &surface_view,
            world: &world_view,
            place: &place_view,
            face_instances: sprites.instances_buffer(),
            mobile_instances: &dummy_instances,
            mesh_instances: &dummy_mesh_instances,
            // Empty, like the ground pass drawn above — a real buffer of no
            // rows rather than a dummy, the same reason that pass was given
            // `&[]` instead of being skipped.
            ground_instances: ground_pass.instances_buffer(),
            zoom,
            rect,
        },
        &Lighting::NONE,
    );
    Outline::new(device, format).render(
        device,
        queue,
        &mut encoder,
        openshard_client_render::outline::Frame {
            target: &surface_view,
            mask: &mask_view,
            mask_size: (width, height),
            rect,
        },
        ring,
    );
    queue.submit([encoder.finish()]);

    read_back(device, queue, &surface)
}

/// A solid square of one colour, packed alone.
fn square(graphic: Graphic, side: u16, color: Color16) -> StaticAtlas {
    StaticAtlas::pack([(
        graphic,
        Image::new(side, side, vec![color; usize::from(side) * usize::from(side)]),
    )])
    .expect("one sprite fits")
}

/// The ring is exactly the pixels next to the silhouette and outside it — and
/// the sprite itself is left alone.
///
/// Both halves are the assertion. A dilation that drew the *whole* grown shape
/// instead of the grown-minus-original ring passes any test that only looks at
/// the border, and it covers the art it was supposed to be pointing at.
#[test]
fn a_ring_is_drawn_around_a_silhouette_and_not_over_it() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const GRAPHIC: Graphic = Graphic(1);
    const SIDE: u16 = 16;
    let green = Color16(0b0_00000_11111_00000);
    let atlas = square(GRAPHIC, SIDE, green);
    let sprite = atlas.sprite(GRAPHIC).expect("packed");
    let (x, y) = (40.0, 50.0);
    let quads = [SpriteQuad {
        rect: Rect {
            x,
            y,
            width: f32::from(SIDE),
            height: f32::from(SIDE),
        },
        region: sprite.region,
        depth: 0.5,
        hue: 0,
        place: Place::NOWHERE,
        twin: 0,
        owner: 0,
    }];

    let (width, height) = (128, 128);
    let frame = render_outlined(
        &device,
        &queue,
        &atlas,
        &quads,
        &quads,
        width,
        height,
        Zoom::ONE,
        Ring::DEFAULT,
    );

    let (green_r, green_g, green_b) = green.rgb8();
    let white = [u8::MAX; 4];
    let (left, top) = (x as u32, y as u32);
    let (right, bottom) = (left + u32::from(SIDE), top + u32::from(SIDE));
    let mut ringed = 0;
    for py in 0..height {
        for px in 0..width {
            let inside = (left..right).contains(&px) && (top..bottom).contains(&py);
            // One pixel out on every side, corners included: an eight-tap
            // neighbourhood rings the diagonal too, and a four-tap one does not
            // — which is the difference between a closed ring and one with four
            // holes in it.
            let bordering = (left - 1..right + 1).contains(&px) && (top - 1..bottom + 1).contains(&py);
            let got = frame.pixel(px, py);
            if inside {
                assert_eq!(
                    got,
                    [green_r, green_g, green_b, u8::MAX],
                    "({px}, {py}) is inside the sprite and the ring painted over it",
                );
            } else if bordering {
                assert_eq!(got, white, "({px}, {py}) borders the sprite and was not ringed");
                ringed += 1;
            } else {
                assert_eq!(got[3], 0, "({px}, {py}) is nowhere near the sprite");
            }
        }
    }
    // The frame of a 16x16 square grown by one: 18² - 16².
    assert_eq!(ringed, 18 * 18 - 16 * 16);
}

/// Two outlined sprites that touch keep one ring each.
///
/// This is the whole reason the mask holds an *id* rather than a coverage bit.
/// With coverage the shared edge is interior to the union — every neighbour of
/// it is "drawn" — so no ring is grown there and the pair comes out outlined as
/// a single blob. The seam below is the pixel column where that shows.
#[test]
fn two_touching_silhouettes_are_ringed_separately() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const GRAPHIC: Graphic = Graphic(1);
    const SIDE: u16 = 16;
    let green = Color16(0b0_00000_11111_00000);
    let atlas = square(GRAPHIC, SIDE, green);
    let sprite = atlas.sprite(GRAPHIC).expect("packed");
    let (x, y) = (40.0f32, 50.0f32);
    // Edge to edge, sharing no pixel: the left one ends where the right begins.
    let quads: Vec<SpriteQuad> = [x, x + f32::from(SIDE)]
        .into_iter()
        .map(|at| SpriteQuad {
            rect: Rect {
                x: at,
                y,
                width: f32::from(SIDE),
                height: f32::from(SIDE),
            },
            region: sprite.region,
            depth: 0.5,
            hue: 0,
            place: Place::NOWHERE,
            twin: 0,
            owner: 0,
        })
        .collect();

    let (width, height) = (128, 128);
    let frame = render_outlined(
        &device,
        &queue,
        &atlas,
        &quads,
        &quads,
        width,
        height,
        Zoom::ONE,
        Ring::DEFAULT,
    );

    let white = [u8::MAX; 4];
    let seam = x as u32 + u32::from(SIDE);
    let middle = y as u32 + u32::from(SIDE) / 2;
    assert_eq!(
        frame.pixel(seam - 1, middle),
        white,
        "the left sprite's own edge against the right one was not ringed — \
         the mask is behaving like coverage rather than an identity",
    );
    assert_eq!(
        frame.pixel(seam, middle),
        white,
        "and neither was the right sprite's edge against the left one",
    );
    // The outer edges are still there: a rule that only ever fired between two
    // ids would ring the seam and nothing else.
    assert_eq!(frame.pixel(x as u32 - 1, middle), white, "the pair's left edge");
    assert_eq!(
        frame.pixel(x as u32 + 2 * u32::from(SIDE), middle),
        white,
        "the pair's right edge",
    );
}

/// The glow reaches past the ring, fades with distance, and leaves the art it
/// is pointing at alone.
///
/// All three halves are the assertion, and the third is the one a blur gets
/// wrong: an additive wash that covered the silhouette too would brighten
/// exactly the pixels the player is being asked to look at, and it would still
/// look like a glow in a screenshot.
#[test]
fn a_glow_reaches_past_the_ring_and_fades_with_distance() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const GRAPHIC: Graphic = Graphic(1);
    const SIDE: u16 = 16;
    let green = Color16(0b0_00000_11111_00000);
    let atlas = square(GRAPHIC, SIDE, green);
    let sprite = atlas.sprite(GRAPHIC).expect("packed");
    let (x, y) = (40.0, 50.0);
    let quads = [SpriteQuad {
        rect: Rect {
            x,
            y,
            width: f32::from(SIDE),
            height: f32::from(SIDE),
        },
        region: sprite.region,
        depth: 0.5,
        hue: 0,
        place: Place::NOWHERE,
        twin: 0,
        owner: 0,
    }];

    let (width, height) = (128, 128);
    let frame = render_outlined(
        &device,
        &queue,
        &atlas,
        &quads,
        &quads,
        width,
        height,
        Zoom::ONE,
        Ring::SOFT,
    );

    let (green_r, green_g, green_b) = green.rgb8();
    let right = x as u32 + u32::from(SIDE);
    let middle = y as u32 + u32::from(SIDE) / 2;
    // Just past the ring, and further out. Read on the red channel: the glow is
    // white and the background is the ground pass's cleared black, so every
    // channel carries the same number and one of them is the measurement.
    let near = frame.pixel(right + 1, middle)[0];
    let far = frame.pixel(right + 4, middle)[0];
    assert!(near > 0, "nothing is lit one pixel past the ring");
    assert!(
        near > far,
        "the glow does not fade: {near} at one pixel out against {far} at four",
    );
    assert!(far > 0, "the glow stops dead at four pixels rather than fading");
    // Well past `Glow::DEFAULT`'s reach of six.
    assert_eq!(
        frame.pixel(right + 24, middle)[0],
        0,
        "the glow reaches 24 pixels, which is most of a static",
    );
    // And the sprite is exactly its own colour still.
    assert_eq!(
        frame.pixel(x as u32 + 2, middle),
        [green_r, green_g, green_b, u8::MAX],
        "the glow was added over the art it is pointing at",
    );
}

/// A minified ring keeps all four of its sides.
///
/// The mask is the world image and the composite reads it at the *surface*'s
/// resolution, so below 1:1 it is point-sampled: at `1/2` only every other mask
/// texel is ever looked at, and a one-texel ring loses whichever of its sides
/// falls on the parity nothing samples. `Ring::for_zoom` is the fix and the
/// second half of this test is why it is not imagined — the same frame with a
/// fixed one-texel ring comes back with edges missing.
#[test]
fn a_minified_ring_keeps_every_side() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const GRAPHIC: Graphic = Graphic(1);
    const SIDE: u16 = 16;
    let green = Color16(0b0_00000_11111_00000);
    let atlas = square(GRAPHIC, SIDE, green);
    let sprite = atlas.sprite(GRAPHIC).expect("packed");
    let (x, y) = (40.0, 50.0);
    let quads = [SpriteQuad {
        rect: Rect {
            x,
            y,
            width: f32::from(SIDE),
            height: f32::from(SIDE),
        },
        region: sprite.region,
        depth: 0.5,
        hue: 0,
        place: Place::NOWHERE,
        twin: 0,
        owner: 0,
    }];

    let (width, height) = (128, 128);
    // Half, the widest rung of the ladder and the worst case for this.
    let zoom = Zoom::ONE.scale_down().scale_down().scale_down();
    assert_eq!(
        (zoom.numerator(), zoom.denominator()),
        (1, 2),
        "the rung this is about"
    );

    // Where each side of the ring lands on a half-sized surface: one screen
    // pixel outside the sprite's own screen rectangle, on the middle of each
    // side.
    let (left, top) = (x as u32 / 2, y as u32 / 2);
    let side = u32::from(SIDE) / 2;
    let (middle_x, middle_y) = (left + side / 2, top + side / 2);
    let edges = [
        ("left", left - 1, middle_y),
        ("right", left + side, middle_y),
        ("top", middle_x, top - 1),
        ("bottom", middle_x, top + side),
    ];

    let lit = |ring: Ring| {
        let frame = render_outlined(&device, &queue, &atlas, &quads, &quads, width, height, zoom, ring);
        edges
            .into_iter()
            .filter(|(_, px, py)| frame.pixel(*px, *py) == [u8::MAX; 4])
            .map(|(name, _, _)| name)
            .collect::<Vec<_>>()
    };

    assert_eq!(
        lit(Ring::DEFAULT.for_zoom(zoom)).len(),
        edges.len(),
        "a ring widened for the zoom lost a side",
    );
    // The companion: without it the ring is not merely thinner, it is gone on
    // two sides — so the assertion above is measuring something.
    let naive = lit(Ring::DEFAULT);
    assert!(
        naive.len() < edges.len(),
        "a one-texel ring survived being point-sampled at half, so this test \
         proves nothing: {naive:?}",
    );
}

/// Write the soft highlight out as a picture, for a person to look at.
///
/// Ignored, for the reason [`dump_a_frame_of_britain`] is: the tests above count
/// the glow's pixels and counting cannot say whether it *reads* as a highlight.
/// A grey slab stands in for the world behind it, because a glow is additive and
/// what matters is how far it lifts a picture that is already there.
///
/// ```sh
/// cargo test -p openshard-client-render --test frame -- --ignored dump_a_glow
/// ```
#[test]
#[ignore = "writes a picture for a person, and asserts nothing"]
fn dump_a_glowing_sprite() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    const BACKDROP: Graphic = Graphic(1);
    const ITEM: Graphic = Graphic(2);
    let grey = Color16(0b0_01100_01100_01100);
    let green = Color16(0b0_00110_11000_00110);
    let atlas = StaticAtlas::pack([
        (BACKDROP, Image::new(96, 96, vec![grey; 96 * 96])),
        (ITEM, Image::new(20, 20, vec![green; 20 * 20])),
    ])
    .expect("two sprites fit");

    let quad = |graphic: Graphic, x: f32, y: f32, side: f32, depth: f32| SpriteQuad {
        rect: Rect {
            x,
            y,
            width: side,
            height: side,
        },
        region: atlas.sprite(graphic).expect("packed").region,
        depth,
        hue: 0,
        place: Place::NOWHERE,
        twin: 0,
        owner: 0,
    };
    let backdrop = quad(BACKDROP, 16.0, 16.0, 96.0, 0.9);
    let item = quad(ITEM, 54.0, 54.0, 20.0, 0.5);

    let frame = render_outlined(
        &device,
        &queue,
        &atlas,
        &[backdrop, item],
        &[item],
        128,
        128,
        Zoom::ONE,
        Ring::SOFT,
    );

    let path = std::env::var_os("OPENSHARD_FRAME_DUMP")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("glow.png"));
    std::fs::write(
        &path,
        openshard_client_render::png::encode_rgba(128, 128, &frame.pixels),
    )
    .expect("writing the frame");
    eprintln!("wrote {}", path.display());
}

/// How many pixels of the parity frame make one tile.
///
/// Eight, so that a 64-pixel frame is eight tiles across — the room in
/// `scene::room` plus a ring of street around it — and so that the sub-tile
/// fraction takes eight distinct values down each tile rather than being a
/// constant the shader could ignore without anybody noticing.
const PARITY_TILE: u32 = 8;

/// Where the parity frame says a pixel is: a tile, and where in it.
///
/// The frame is laid out as a plain grid of tiles rather than as a projection.
/// Nothing here is testing the projection — what is being compared is one
/// formula against another, and a synthetic layout means the *expected* value
/// comes from a tile this function names rather than from a second copy of the
/// camera's arithmetic.
fn parity_place(px: u32, py: u32) -> (u16, u16, u16, u16) {
    let (cx, cy) = openshard_client_render::scene::CENTRE;
    let tile_x = cx - 4 + (px / PARITY_TILE) as u16;
    let tile_y = cy - 4 + (py / PARITY_TILE) as u16;
    // Sixteenths of a tile, which is what a seven-bit fraction holds exactly:
    // a value the shader divides by 127 and the CPU side divides by 127, so the
    // two agree on the number and not merely on the intent.
    let sub_x = (px % PARITY_TILE) as u16 * 16;
    let sub_y = (py % PARITY_TILE) as u16 * 16;
    (tile_x, tile_y, sub_x, sub_y)
}

/// What every pixel of a parity fixture *is*: a surface, at a height, of an
/// occluder.
///
/// The three travel together because they are one statement about the fragment
/// and the two sides have to make the same one — the attachment says it to the
/// shader and the [`Spot`](openshard_client_render::light::Spot) says it to
/// `light::sample`, and a fixture that set them apart could tell the two
/// different stories without failing to compile.
#[derive(Clone, Copy)]
struct Fixture {
    surface: Surface,
    z: i8,
    /// Which occluder of its own cell every pixel is a point of.
    ///
    /// [`OwnerId::NONE`] for every fixture that predates sub-tile lids, and that
    /// is the honest default: those scenes are flat ground and walls, where a
    /// pixel is a point of nothing and identity decides nothing. A fixture whose
    /// scene has a *flight* in it must say otherwise — a tread's top is excused
    /// from its own lid by identity alone, and without an owner the fragment is
    /// shadowed by the very step it stands on and every other question about it
    /// is unreachable.
    owner: OwnerId,
}

impl Fixture {
    /// Flat ground at `z = 0`, a point of nothing: what every parity scene was
    /// before there was anything else to be.
    fn ground() -> Self {
        Self {
            surface: Surface::Upright,
            z: 0,
            owner: OwnerId::NONE,
        }
    }
}

/// One frame of the parity fixture: a white world, a place attachment this test
/// wrote, and `scene::room`'s lighting drawn in `view`.
///
/// White because the blit multiplies the art by the lighting: with every channel
/// at one, what comes out *is* the multiplier, clamped — so a mismatch is a
/// mismatch in the lighting and not in a sprite.
fn parity_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    lighting: &Lighting,
    width: u32,
    height: u32,
    fixture: Fixture,
) -> Frame {
    let Fixture { surface, z, owner } = fixture;
    let world = openshard_client_render::blit::world_texture(device, width, height);
    let world_view = world.create_view(&wgpu::TextureViewDescriptor::default());
    let place = openshard_client_render::place::texture(device, width, height);
    let place_view = place.create_view(&wgpu::TextureViewDescriptor::default());

    // Neither `Kind::Static` nor `Kind::Land` pixels carry their own `x`/`y`
    // in the attachment any more — `docs/gbuffer.md` step 3 moved a static's
    // to a row this fixture has to build itself, the same way `statics.wgsl`
    // would have, and step 7 did the same for the ground. One row per
    // distinct tile the sweep below actually uses, keyed by first sight: the
    // sweep visits the same handful of tiles many times over (`PARITY_TILE`
    // pixels apiece), and a row per *pixel* would be the id repeating the
    // very repetition decision 2 exists to remove.
    let mut face_ids: std::collections::HashMap<(u16, u16), u32> = std::collections::HashMap::new();
    let mut face_rows: Vec<u8> = Vec::new();
    let mut id_of = |x: u16, y: u16| -> u32 {
        *face_ids.entry((x, y)).or_insert_with(|| {
            let id = (face_rows.len() as u64 / openshard_client_render::sprite::SpriteQuad::STRIDE) as u32;
            SpriteQuad {
                rect: Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 0.0,
                    height: 0.0,
                },
                region: openshard_client_render::atlas::Region {
                    u: 0.0,
                    v: 0.0,
                    du: 0.0,
                    dv: 0.0,
                },
                depth: 0.0,
                hue: 0,
                place: openshard_client_render::place::Place::land(x, y),
                // This sweep never asks for a corner `Stance`, so there is
                // never a second half to point at — see
                // `crate::sprite::split_corners` for the real pass's row.
                twin: 0,
                // The fixture's own, so the shader is told what `Spot` is told.
                owner: u32::from(owner.raw()),
            }
            .write(&mut face_rows);
            id
        })
    };

    // `Kind::Land` pixels need the same treatment since `docs/gbuffer.md`
    // step 7 — one row per distinct tile, keyed by first sight, the ground
    // half of `id_of` above.
    let mut ground_ids: std::collections::HashMap<(u16, u16), u32> = std::collections::HashMap::new();
    let mut ground_rows: Vec<u8> = Vec::new();
    let mut ground_id_of = |x: u16, y: u16| -> u32 {
        *ground_ids.entry((x, y)).or_insert_with(|| {
            let id = (ground_rows.len() as u64 / openshard_client_render::ground::GroundQuad::STRIDE) as u32;
            openshard_client_render::ground::GroundQuad {
                x: 0.0,
                y: 0.0,
                corners: [0.0; 4],
                region: openshard_client_render::atlas::Region {
                    u: 0.0,
                    v: 0.0,
                    du: 0.0,
                    dv: 0.0,
                },
                texmap: None,
                depth: 0.0,
                place: openshard_client_render::place::Place::land(x, y),
            }
            .write(&mut ground_rows);
            id
        })
    };

    let mut texels: Vec<u16> = Vec::with_capacity((width * height * 4) as usize);
    for py in 0..height {
        for px in 0..width {
            let (x, y, sub_x, sub_y) = parity_place(px, py);
            // `(id, z + 128 | stance, kind | sub)` either way now — an id
            // into `ground_rows` for the ground, into `face_rows` for a
            // static's face: the packing `crate::place` documents and
            // `blit.wgsl` takes apart. Land at
            // `z = 0` — the ground of the room — unless the fixture is about a
            // wall's face, in which case every pixel is a static standing on
            // that face.
            //
            // The stance is what the facing test reads, and a fixture without one
            // would leave the whole of that test uncompared: `light::sample` would
            // agree with the shader about a formula neither of them ran.
            let (kind, stance) = match surface {
                // Land with no stance: what every fixture that predates surfaces
                // is, and a billboard's answer — nothing is known about which way
                // it looks, so every flame that reaches it lights it.
                Surface::Upright => (1u16, openshard_client_render::place::Stance::Upright as u16),
                // A floor, a rug, the top of a wall: it looks up, and that is the
                // fixture decision 27 needed. Without one the shader could return
                // any normal at all for a flat pixel and every parity test here
                // would still pass.
                Surface::Flat => (
                    openshard_client_render::place::Kind::Static as u16,
                    openshard_client_render::place::Stance::Flat as u16,
                ),
                Surface::Face(face) => (
                    openshard_client_render::place::Kind::Static as u16,
                    openshard_client_render::place::Stance::face(face) as u16,
                ),
            };
            let height = (i32::from(z) + 128) as u16 | stance << openshard_client_render::place::STANCE_SHIFT;
            // A static's or a mobile's *tile* comes from the row now — only
            // that moved. The fraction is still this fragment's own, packed
            // exactly as the ground's, in every case: see `blit.wgsl`'s own
            // comment for why a wall's face needs one too.
            let (word0, word1) = if kind == openshard_client_render::place::Kind::Static as u16 {
                let id = id_of(x, y);
                ((id & 0xFFFF) as u16, (id >> 16) as u16)
            } else {
                let id = ground_id_of(x, y);
                ((id & 0xFFFF) as u16, (id >> 16) as u16)
            };
            texels.extend_from_slice(&[word0, word1, height, kind | sub_x << 2 | sub_y << 9]);
        }
    }
    let bytes: Vec<u8> = texels.iter().flat_map(|word| word.to_le_bytes()).collect();
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &place,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 8),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );

    let surface = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("surface"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: openshard_client_render::blit::WORLD_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let surface_view = surface.create_view(&wgpu::TextureViewDescriptor::default());

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    // The white world, as a clear rather than as an upload: a render pass that
    // stores its clear is the one way to fill a texture that is a render target
    // and not a copy destination.
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("white world"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &world_view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        multiview_mask: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });

    let mut blit = Blit::new(device, openshard_client_render::blit::WORLD_FORMAT);
    let dummy_instances = openshard_client_render::blit::dummy_instances(device);
    let dummy_mesh_instances = openshard_client_render::blit::dummy_mesh_instances(device);
    let dummy_ground_instances = openshard_client_render::blit::dummy_ground_instances(device);
    // Only built above when the fixture used `Kind::Static` at all — a fixture
    // built entirely from `Surface::Flat`/`Surface::Face` has nothing for the
    // dummy to stand in for.
    let face_instances = if face_rows.is_empty() {
        None
    } else {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("parity face instances"),
            size: face_rows.len() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buffer, 0, &face_rows);
        Some(buffer)
    };
    // The mirror of `face_instances`: built only when the fixture used
    // `Surface::Upright`'s `Kind::Land` — every fixture but the ones that
    // never leave a wall's face.
    let ground_instances = if ground_rows.is_empty() {
        None
    } else {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("parity ground instances"),
            size: ground_rows.len() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buffer, 0, &ground_rows);
        Some(buffer)
    };
    blit.render(
        device,
        queue,
        &mut encoder,
        openshard_client_render::blit::Frame {
            target: &surface_view,
            world: &world_view,
            place: &place_view,
            face_instances: face_instances.as_ref().unwrap_or(&dummy_instances),
            // No mobile pixels in this fixture: the dummy stands in for it.
            mobile_instances: &dummy_instances,
            mesh_instances: &dummy_mesh_instances,
            ground_instances: ground_instances.as_ref().unwrap_or(&dummy_ground_instances),
            zoom: Zoom::ONE,
            rect: ViewportRect {
                x: 0,
                y: 0,
                width,
                height,
            },
        },
        lighting,
    );
    queue.submit([encoder.finish()]);
    read_back(device, queue, &surface)
}

/// **A vertical ray on the GPU is stopped only by lids it is actually under.**
///
/// `light::a_vertical_ray_is_not_stopped_by_lids_it_is_not_over` is the same
/// claim about the two CPU walks; this is the shader's own copy of the shortcut,
/// and it needed a fixture of its own because **nothing here could see it**.
/// Deleting the gate from `blit.wesl` alone left all forty-seven frame tests
/// green: no parity scene has a sub-tile lid in it, so the branch that reads one
/// was never run. The gap was in the harness, not in the sweep's size.
///
/// The frame is read **directly**, and that is the whole of it now. This used to
/// stand beside a parity sweep of the shader against `light::sample`, and the
/// sweep was the circular half: both sides were fixed together, so it reported
/// agreement whether or not either was right. What is left is the claim neither
/// walk gets a vote on — the pixel over the bottom tread is lit and the pixel
/// over the middle one is not, which is a statement about this scene's geometry.
///
/// Why those two pixels. Every point of the tile is over exactly one tread — the
/// three strips tile it — so "a lid the ray is not under" is only reachable from
/// a fragment standing on a tread of its own, excused from that one lid by
/// identity and asking about the other two. Hence [`Fixture::owner`]: without it
/// the fragment is shadowed by the step it stands on and the question never
/// arises. The middle-tread pixel is the control, blocked by its own tread's lid
/// two units under it, and it is what says the scene occludes at all.
#[test]
fn the_shader_does_not_stop_a_vertical_ray_with_a_lid_it_is_not_under() {
    let Some((device, queue)) = gpu() else {
        return;
    };

    // The three-tread flight of `light`'s own copy of this test: north, so the
    // treads divide the tile up `y` — tread 0 over `100.667..101` capped at
    // `z 1`, tread 1 over `100.333..100.667` at `z 3`, tread 2 over
    // `100..100.333` at `z 5`.
    let stair = openshard_uofiles::tiledata::StaticTile {
        flags: openshard_uofiles::tiledata::TileFlags::new(
            openshard_uofiles::tiledata::TileFlags::NO_SHOOT
                | openshard_uofiles::tiledata::TileFlags::CLIMBABLE,
        ),
        height: 20,
        ..openshard_uofiles::tiledata::StaticTile::default()
    };
    let prism =
        openshard_client_render::facing::Prism::new(openshard_client_render::facing::Face::North, &[1, 3, 5])
            .expect("three treads");
    let (cx, cy) = openshard_client_render::scene::CENTRE;
    let mut builder = Builder::new(TileBounds {
        min_x: i32::from(cx) - 10,
        max_x: i32::from(cx) + 10,
        min_y: i32::from(cy) - 10,
        max_y: i32::from(cy) + 10,
    });
    let graphic = Graphic(0x0736);
    builder.add(cx, cy, 0, graphic, &stair, Shape::solid(prism));
    let occlusion = builder.finish(&Cutaway::OPEN);
    let owner = occlusion.owner_at(i32::from(cx), i32::from(cy), 0, graphic);
    assert!(
        !owner.same(OwnerId::NONE),
        "the flight has to have an owner or the fragment is shadowed by its own tread",
    );

    // The pixel the ray is vertical at, and its control. Both are on the centre
    // tile; `LIT` sits in the *bottom* tread's strip and `BLOCKED` in the middle
    // tread's. The flame's position comes out of `parity_place` rather than being
    // written down again — "directly above" has to be exact, and a second copy of
    // the fixture's own arithmetic is the one way to get it a float off.
    //
    // **Above and not below**, which is not arbitrary: a `Stance::Flat` fragment
    // looks up, so a flame under it is behind its own plane and `light::faces`
    // takes the whole term to nothing — both pixels come out at the ambient and
    // the fixture measures the facing rule instead of the shortcut. Hence the
    // bottom tread: it is the one whose two neighbouring lids are *over* it.
    const LIT: (u32, u32) = (35, 38);
    const BLOCKED: (u32, u32) = (35, 35);
    let (x, y, sub_x, sub_y) = parity_place(LIT.0, LIT.1);
    assert_eq!(
        (x, y),
        (cx, cy),
        "the lit pixel has to be on the flight's own tile"
    );
    let over = Vec2::new(
        f32::from(x) + f32::from(sub_x) / 127.0,
        f32::from(y) + f32::from(sub_y) / 127.0,
    );

    let lighting = Lighting {
        ambient: openshard_client_render::light::NIGHT,
        lights: vec![Light {
            at: over,
            // Well above the flight, so the ray runs up past every tread.
            z: 15.0,
            radius: 40.0,
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
            beam: None,
        }],
        occlusion,
        sun: None,
        view: View::default(),
    };
    let fixture = Fixture {
        surface: Surface::Flat,
        // The bottom tread's own height: what makes `LIT` a point *of* that
        // tread rather than of the air over it.
        z: 1,
        owner,
    };

    let (width, height) = (64, 64);
    let frame = parity_frame(&device, &queue, &lighting, width, height, fixture);
    let lit = frame.pixel(LIT.0, LIT.1);
    let blocked = frame.pixel(BLOCKED.0, BLOCKED.1);
    assert!(
        lit[0] > blocked[0] + 40,
        "the flame is directly over {LIT:?} and the lids between them are strips \
         of the tile it is not under: {lit:?} there against {blocked:?} over the \
         middle tread, which its own lid does stop",
    );
}

/// Whether `Reach::through` counts as blocked — `light.rs`'s own
/// `RAY_CUTOFF`, restated here rather than imported: it is not `pub`, and
/// every oracle in `docs/lighting_raymarch.md` (`tests/lighting.rs`'s
/// brute-force grid and fuzz) already carries its own copy of the same
/// number rather than reach into the crate for it.
const EXACT_WALK_BLOCKED: f32 = 0.004;

/// Whether the straight segment from `from` to `to` passes through any solid
/// standing between the two tiles the walk itself exempts — the same idea as
/// `tests/lighting.rs`'s `brute_force_blocked`, restated here rather than
/// shared across two independent test crates: dumb, fixed-step marching and a
/// point-in-box test, sharing no arithmetic with either walk.
///
/// **Twenty thousand steps over the whole segment, not `brute_force_blocked`'s
/// fixed `0.02`-tile step.** The first version of this oracle used that same
/// constant and returned a false "open" for a real, `ray_vs_solid`-confirmed
/// crossing: a ray clipping a panel box at a shallow, corner-grazing angle can
/// spend far less than [`crate::occlusion::PANEL_THICKNESS`]'s own depth
/// inside it — a hundredth of a tile, in the case that found this — and
/// `brute_force_blocked`'s own step was sized to the panel's *thickness*, not
/// to how thin a graze through it can be. A point sampler cannot rule out an
/// arbitrarily thin sliver at any finite resolution; twenty thousand steps
/// is a practical stand-in that this file's own scenes have not defeated, not
/// a claim of exactness the way `ray_vs_solid`'s analytic slab test is one.
///
/// Returns `None` where a solid with an aperture stands on the marched path.
/// `brute_force_blocked` instead hard-`assert!`s a scene never has one, which
/// it can afford: every fixture in that file is hand-built to keep the
/// premise true. This one runs over `scene::wall_with_a_hole_in_it`, whose
/// entire point is an aperture, so a disagreement whose path crosses the hole
/// is left unexplained rather than a reason to panic the whole sweep.
fn ground_truth_blocked(
    from: [f32; 3],
    to: [f32; 3],
    own_tile: (i32, i32),
    target_tile: (i32, i32),
    skip_last: bool,
    occlusion: &Occlusion,
) -> Option<bool> {
    const STEPS: u32 = 20_000;
    let delta = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    for step in 1..STEPS {
        let t = step as f32 / STEPS as f32;
        let point = [
            from[0] + delta[0] * t,
            from[1] + delta[1] * t,
            from[2] + delta[2] * t,
        ];
        let tile = (point[0].floor() as i32, point[1].floor() as i32);
        if tile == own_tile || (skip_last && tile == target_tile) {
            continue;
        }
        for solid in occlusion.solids_at(tile.0, tile.1) {
            if solid.aperture.is_some() {
                return None;
            }
            let (min, max) = (solid.space.min, solid.space.max);
            let inside = f64::from(point[0]) >= min.x
                && f64::from(point[0]) <= max.x
                && f64::from(point[1]) >= min.y
                && f64::from(point[1]) <= max.y
                && f64::from(point[2]) >= min.z
                && f64::from(point[2]) <= max.z;
            if inside {
                return Some(true);
            }
        }
    }
    Some(false)
}

/// Whether the march that backs [`ground_truth_blocked`] ever finds itself a
/// real, non-degenerate distance inside `blamed`'s own box, rather than only
/// brushing its corner.
///
/// `docs/lighting_raymarch.md`'s own established stance on a ray that only
/// ever touches a solid's corner — never a length of its inside — is that
/// this is not a bug in either walk to resolve, it is the accepted overlap
/// two panels physically share at a shared corner
/// (`crate::occlusion::PANEL_THICKNESS`). [`walk_cells_exact`]'s
/// `candidate_tiles` unconditionally probes both diagonal neighbours at
/// every step — a deliberate feature, so a genuine corner-cutting occluder
/// is never missed — and at an *exact* corner tie a ray can graze a
/// diagonal neighbour's own corner point without its straight line ever
/// entering that tile's interior at all. [`walk_cells_streaming`] has no
/// such probe and simply never visits that tile, which is why the two can
/// disagree exactly here without either being wrong: [`exact_walk_
/// disagreements`] arbitrates by [`ground_truth_blocked`], which is blind to
/// *which* tile it found itself inside, so a genuine crossing elsewhere on
/// the segment can back a blamed tile it was never actually about. This
/// walks the identical march restricted to `blamed` alone and requires more
/// than a couple of isolated samples — `floor`'s own discontinuity at an
/// exact tile boundary can put one or two stray samples on the wrong side of
/// a corner it never really entered, which is the same hazard this doc's own
/// `floor`-vs-`round` entry already named for a different oracle.
fn blamed_tile_has_a_real_crossing(
    from: [f32; 3],
    to: [f32; 3],
    blamed: (i32, i32),
    occlusion: &Occlusion,
) -> bool {
    const STEPS: u32 = 20_000;
    const MIN_SAMPLES: u32 = 4;
    let delta = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let mut samples = 0;
    for step in 1..STEPS {
        let t = step as f32 / STEPS as f32;
        let point = [
            from[0] + delta[0] * t,
            from[1] + delta[1] * t,
            from[2] + delta[2] * t,
        ];
        let tile = (point[0].floor() as i32, point[1].floor() as i32);
        if tile != blamed {
            continue;
        }
        for solid in occlusion.solids_at(tile.0, tile.1) {
            let (min, max) = (solid.space.min, solid.space.max);
            let inside = f64::from(point[0]) >= min.x
                && f64::from(point[0]) <= max.x
                && f64::from(point[1]) >= min.y
                && f64::from(point[1]) <= max.y
                && f64::from(point[2]) >= min.z
                && f64::from(point[2]) <= max.z;
            if inside {
                samples += 1;
                if samples >= MIN_SAMPLES {
                    return true;
                }
            }
        }
    }
    false
}

/// What a sweep of [`light::sample_exact`] against [`light::sample`] found,
/// over one of decision 9's own real-geometry scenes.
struct ExactWalkReport {
    /// A classification flip [`ground_truth_blocked`] backs `walk_cells`
    /// for — a genuine, new defect in `walk_cells_exact` this doc has not
    /// already catalogued, and the only thing these tests fail on.
    bugs: Vec<String>,
    /// A classification flip [`ground_truth_blocked`] backs
    /// `walk_cells_exact` for: an already-real `walk_cells` gap, the same
    /// family `docs/lighting_raymarch.md`'s session 9 found four of.
    explained: usize,
    /// A classification flip this oracle cannot rule on — the marched path
    /// crossed a solid with an aperture, which it does not model.
    unexplained: usize,
    /// A classification flip backed by a tile `walk_cells_exact` blamed that
    /// the straight segment only ever grazes the corner of — the accepted
    /// corner-grazing ambiguity, not a defect. See `blamed_tile_has_a_real_
    /// crossing`'s own doc comment.
    grazed: usize,
}

/// [`light::sample_exact`] against [`light::sample`], over one of decision
/// 9's own real-geometry scenes — `docs/lighting_raymarch.md`'s point 3, the
/// half of it that needs no GPU because neither side being compared is the
/// shader. The scene is the fixture, the same reason decision 9's own suite
/// reuses these rather than a fixture built for this test alone: a room's
/// torch, a house corner, a hole in a wall, a window's sun each put a ray
/// through geometry nobody hand-picked to flatter either walk.
///
/// **Not full agreement, on purpose.** The first version of this sweep
/// asserted zero classification flips and failed on more than a hundred
/// pixels of the plainest scene here, `scene::room` — a spot standing on one
/// tile of a straight wall run blamed a *neighbour* tile of the same run for
/// blocking a ray that, marched by hand, never enters that neighbour's box
/// at all. `docs/lighting_raymarch.md`'s session 9 already catalogued four
/// `walk_cells` gaps this exact track's fuzzing found; this is a real fifth,
/// found by real geometry rather than a fuzzer, in the opposite direction
/// from the other four (over-occlusion, not under). See this test file's own
/// handoff entry for the coordinates and the confirming hand-march.
///
/// So a flip is not itself a failure: [`ground_truth_blocked`] — independent
/// of both walks, sharing no arithmetic with either — is asked to arbitrate.
/// Backing `walk_cells_exact` explains the flip as one of `walk_cells`'s own
/// gaps; backing `walk_cells` instead is a real bug in the exact walk, which
/// is what fails these tests.
fn exact_walk_disagreements(lighting: &Lighting, surface: Surface, z: i8) -> ExactWalkReport {
    let mut report = ExactWalkReport {
        bugs: Vec::new(),
        explained: 0,
        grazed: 0,
        unexplained: 0,
    };
    for py in 0..64 {
        for px in 0..64 {
            let (x, y, sub_x, sub_y) = parity_place(px, py);
            let own_tile = (i32::from(x), i32::from(y));
            let spot = openshard_client_render::light::Spot {
                surface,
                ..openshard_client_render::light::Spot::at(
                    Vec2::new(
                        f32::from(x) + f32::from(sub_x) / 127.0,
                        f32::from(y) + f32::from(sub_y) / 127.0,
                    ),
                    f32::from(z),
                    own_tile,
                )
            };
            let from = [spot.at.x, spot.at.y, spot.z];
            let sample = openshard_client_render::light::sample(spot, lighting);
            let exact = openshard_client_render::light::sample_exact(spot, lighting);
            for (index, (a, b)) in sample.reaches.iter().zip(exact.reaches.iter()).enumerate() {
                if !a.within {
                    continue;
                }
                let a_blocked = a.through <= EXACT_WALK_BLOCKED;
                let b_blocked = b.through <= EXACT_WALK_BLOCKED;
                if a_blocked == b_blocked {
                    continue;
                }
                let light = &lighting.lights[index];
                let to = [light.at.x, light.at.y, light.z];
                let target_tile = (light.at.x.floor() as i32, light.at.y.floor() as i32);
                match ground_truth_blocked(from, to, own_tile, target_tile, true, &lighting.occlusion) {
                    None => report.unexplained += 1,
                    Some(truth) if truth == b_blocked => report.explained += 1,
                    // `walk_cells_exact` blamed a tile the straight segment
                    // never actually enters — only its own corner, which
                    // `candidate_tiles`' unconditional diagonal probe reaches
                    // and a bare point-in-box march (blind to which tile it
                    // is even asking about) cannot itself rule out. See
                    // `blamed_tile_has_a_real_crossing`'s own doc comment:
                    // this is `docs/lighting_raymarch.md`'s own accepted
                    // corner-grazing ambiguity, the same shape `walk_cells`'
                    // `corner_tie` used to be generous about on purpose, not
                    // a defect in `walk_cells_exact` to chase.
                    Some(_)
                        if b_blocked
                            && !blamed_tile_has_a_real_crossing(
                                from,
                                to,
                                b.stopped_by.expect("blocked implies a blamed tile").cell,
                                &lighting.occlusion,
                            ) =>
                    {
                        report.grazed += 1;
                    }
                    Some(_) => report.bugs.push(format!(
                        "({px}, {py}) light {index}: walk_cells {} ({:.4}), walk_cells_exact {} \
                         ({:.4}), ground truth says {}",
                        if a_blocked { "blocked" } else { "open" },
                        a.through,
                        if b_blocked { "blocked" } else { "open" },
                        b.through,
                        if a_blocked { "blocked" } else { "open" },
                    )),
                }
            }
            // The sun has no `target_tile` of its own to skip — `walk_sun`/
            // `walk_sun_exact` both pass `skip_last: false` — and no scene
            // here disagreed about it even once this oracle existed to check,
            // so it is compared but not (yet) arbitrated by ground truth.
            if let (Some(a), Some(b)) = (sample.sun, exact.sun) {
                let a_blocked = a.through <= EXACT_WALK_BLOCKED;
                let b_blocked = b.through <= EXACT_WALK_BLOCKED;
                if a_blocked != b_blocked {
                    report.bugs.push(format!(
                        "({px}, {py}) sun: walk_cells {} ({:.4}), walk_cells_exact {} ({:.4}), \
                         not yet arbitrated by ground truth",
                        if a_blocked { "blocked" } else { "open" },
                        a.through,
                        if b_blocked { "blocked" } else { "open" },
                        b.through,
                    ));
                }
            }
        }
    }
    report
}

/// Asserts [`exact_walk_disagreements`] found no unexplained bug, and prints
/// the explained/unexplained counts either way — visible with `--nocapture`,
/// and in a failure's own message, since a reader deciding whether a fifth
/// `walk_cells` gap needs its own backlog entry wants those numbers regardless
/// of which count actually fired.
fn assert_no_exact_walk_bugs(report: &ExactWalkReport) {
    println!(
        "{} explained (known walk_cells gaps), {} unexplained (aperture on the path), \
         {} grazed (corner-only touch)",
        report.explained, report.unexplained, report.grazed,
    );
    assert!(
        report.bugs.is_empty(),
        "{} of 4096 pixels found a real walk_cells_exact bug:\n{}",
        report.bugs.len(),
        report.bugs.join("\n"),
    );
}

#[test]
fn the_exact_walk_agrees_with_light_sample_over_a_room() {
    let scene = openshard_client_render::scene::room();
    let report = exact_walk_disagreements(&scene.lighting(0.0), Surface::Upright, 0);
    assert_no_exact_walk_bugs(&report);
}

/// And the same for a scene whose occluders are panels on named edges —
/// see `the_shader_and_light_sample_agree_about_which_side_a_wall_is_on`'s
/// own comment for why this shape gets its own fixture.
#[test]
fn the_exact_walk_agrees_with_light_sample_about_which_side_a_wall_is_on() {
    let scene = openshard_client_render::scene::wall_with_a_torch_beside_it();
    let report = exact_walk_disagreements(&scene.lighting(0.0), Surface::Upright, 0);
    assert_no_exact_walk_bugs(&report);
}

/// And the same at a house corner — a run of panels meeting a faceless whole
/// tile, exactly the shape `corner_tie` exists for and the one
/// `docs/lighting_raymarch.md`'s session 9 traced two of `walk_cells`'s own
/// gaps from.
#[test]
fn the_exact_walk_agrees_with_light_sample_at_the_corner_of_a_house() {
    let scene = openshard_client_render::scene::house_corner();
    let report = exact_walk_disagreements(&scene.lighting(0.0), Surface::Upright, 0);
    assert_no_exact_walk_bugs(&report);
}

/// And the same for a scene with a hole in one of its panels — where
/// [`ground_truth_blocked`]'s own aperture scope is expected to leave some
/// flips unexplained rather than arbitrated, see this test's own handoff
/// entry for the count.
#[test]
fn the_exact_walk_agrees_with_light_sample_about_a_hole_in_a_wall() {
    let scene = openshard_client_render::scene::wall_with_a_hole_in_it();
    let report = exact_walk_disagreements(&scene.lighting(0.0), Surface::Upright, 0);
    assert_no_exact_walk_bugs(&report);
}

/// And the same for a scene with a sun in it — [`walk_sun_exact`] against
/// [`walk_sun`], not just the flame end of the seam.
#[test]
fn the_exact_walk_agrees_with_light_sample_about_the_sun() {
    let scene = openshard_client_render::scene::sunlit_room_with_window();
    let report = exact_walk_disagreements(&scene.lighting(0.0), Surface::Upright, 0);
    assert_no_exact_walk_bugs(&report);
}

/// And the same over a surface that looks up, at both a lit floor and a
/// lid over the flame — decision 27's own reason for a second fixture,
/// restated for this seam.
#[test]
fn the_exact_walk_agrees_with_light_sample_about_a_surface_that_looks_up() {
    let lighting = openshard_client_render::scene::room().lighting(0.0);
    for z in [0, 20] {
        let report = exact_walk_disagreements(&lighting, Surface::Flat, z);
        assert_no_exact_walk_bugs(&report);
    }
}

/// And the same for a carried beam — the one term the other scenes above
/// cannot exercise, since every static flame lights all ways.
#[test]
fn the_exact_walk_agrees_with_light_sample_about_a_carried_beam() {
    let scene = openshard_client_render::scene::lantern_in_a_room();
    let report = exact_walk_disagreements(&scene.lighting(0.0), Surface::Upright, 0);
    assert_no_exact_walk_bugs(&report);
}

/// The light view has no plateau in the middle of a pool.
///
/// The failure this protects against is the one it was written for: the view
/// clamped, a torch's multiplier is past `1.0` for the whole middle of its pool,
/// and so the core of every flame — the part of the shape no other view shows,
/// because the lit frame multiplies it by dark art — was drawn as one flat white
/// disc. A clamp is invisible in a screenshot of a *dim* scene, which is why it
/// survived; a monotone curve is the property, and the way to state it is that
/// walking towards the flame never stops getting brighter.
#[test]
fn the_light_view_keeps_a_pools_shape_where_it_is_brightest() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    let (width, height) = (64, 64);
    let scene = openshard_client_render::scene::room();
    let mut lighting = scene.lighting(0.0);
    lighting.view = View::Light;
    let frame = parity_frame(&device, &queue, &lighting, width, height, Fixture::ground());

    // The torch is at the room's centre, which the fixture puts at the middle of
    // the frame: a row through it runs from the pool's rim to its brightest
    // point. Every step inwards must be brighter than the last — under a clamp
    // the last two tiles' worth of it were one number.
    let middle = height / 2;
    for px in 1..width / 2 {
        let before = i32::from(frame.pixel(px - 1, middle)[0]);
        let after = i32::from(frame.pixel(px, middle)[0]);
        assert!(
            after >= before,
            "at ({px}, {middle}) the row towards the flame darkens: {before} then {after}",
        );
    }
    // And it is a ramp rather than a couple of terraces: a curve that clipped
    // would still pass the test above, which is the trap the first version fell
    // into. So **every** step of the row that is actually inside the room rises,
    // which is a stronger claim than the "more than a quarter of the whole row"
    // this used to make — and it has to be stated over that stretch rather than
    // over the whole row, because the row starts outside the house.
    //
    // The fixture's leftmost two tiles are the ground outside the ring and the
    // ring's own wall tile, and both are flat dark. That the *wall* stretch is
    // flat is `docs/lighting_height.md` phase 3: this fixture's pixels are
    // `Surface::Upright` points of no occluder at all, so they are exempt from
    // nothing, and a point standing inside the room's own wall body is behind it.
    // The old count happened to include those eight pixels because the height
    // guess exempted them from the wall they stand in — which is the guess this
    // phase removed, and a bar that counted them was measuring the guess.
    let inside = (4 - u32::from(openshard_client_render::scene::ROOM_HALF) + 1) * PARITY_TILE;
    let mut steps = 0;
    for px in inside..width / 2 {
        let before = i32::from(frame.pixel(px - 1, middle)[0]);
        let after = i32::from(frame.pixel(px, middle)[0]);
        assert!(
            after > before,
            "at ({px}, {middle}) the row inside the room does not rise: {before} then {after}",
        );
        steps += 1;
    }
    assert_eq!(
        steps,
        (width / 2 - inside) as usize,
        "the sweep inside the room compared nothing like the stretch it claims to",
    );
    // Nothing saturates, anywhere. A tone map that reached white would be a
    // clamp again for whatever is brighter than the thing that reached it.
    for py in 0..height {
        for px in 0..width {
            let pixel = frame.pixel(px, py);
            assert!(
                pixel[..3].iter().all(|channel| *channel < 255),
                "at ({px}, {py}) the light view saturates: {pixel:?}",
            );
        }
    }
}

/// Write every debug view of a scene out as a picture, for a person to look at.
///
/// Ignored, and asserts nothing: the views are read as shapes, and a shape is
/// the one thing a count cannot check. Run it and look:
///
/// ```sh
/// OPENSHARD_VIEW_DUMP=/tmp/views cargo test -p openshard-client-render --test frame -- \
///     --ignored dump_the_lighting_views
/// ```
#[test]
#[ignore = "writes pictures for a person, and asserts nothing"]
fn dump_the_lighting_views() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    let (width, height) = (64, 64);
    let dir = std::env::var_os("OPENSHARD_VIEW_DUMP")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("views"));
    std::fs::create_dir_all(&dir).expect("the dump directory");
    let scenes = [
        ("room", openshard_client_render::scene::room()),
        (
            "window",
            openshard_client_render::scene::sunlit_room_with_window(),
        ),
        ("roofed", openshard_client_render::scene::roofed_room()),
    ];
    for (name, scene) in scenes {
        for view in View::ALL {
            let mut lighting = scene.lighting(0.0);
            lighting.view = view;
            let frame = parity_frame(&device, &queue, &lighting, width, height, Fixture::ground());
            let path = dir.join(format!("{name}-{}.png", view.name()));
            std::fs::write(
                &path,
                openshard_client_render::png::encode_rgba(width, height, &frame.pixels),
            )
            .expect("writing the frame");
            eprintln!("wrote {}", path.display());
        }
    }
}

/// A debug view reaches the shader, and draws what it says it draws.
///
/// Two things at once, and both are contracts no compiler checks: that
/// `View`'s numbers are the ones `blit.wgsl` switches on, and that the uniform
/// block's third header `vec4` lands where the shader looks for it — a field
/// written at the wrong offset would not fail validation, it would silently be
/// the light count or a corner of the occlusion grid.
#[test]
fn a_debug_view_reaches_the_shader() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    let (width, height) = (64, 64);
    let scene = openshard_client_render::scene::room();
    let mut lighting = scene.lighting(0.0);
    lighting.view = View::Kind;
    let frame = parity_frame(&device, &queue, &lighting, width, height, Fixture::ground());

    // Every pixel of the fixture is land, and the kind view paints land one
    // colour whatever the lighting did — so a frame that still shows a pool of
    // firelight means the view never arrived.
    let land = [
        (0.20 * 255.0) as i32,
        (0.65 * 255.0) as i32,
        (0.30 * 255.0) as i32,
    ];
    for (px, py) in [(0, 0), (31, 31), (63, 63), (8, 40)] {
        let drawn = frame.pixel(px, py);
        for (channel, want) in land.iter().enumerate() {
            assert!(
                (want - i32::from(drawn[channel])).abs() <= 1,
                "at ({px}, {py}), channel {channel}: {} is not the land colour {want}",
                drawn[channel],
            );
        }
    }
}
