//! The selection wash, on a real GPU: what it paints and — the half that
//! matters — what it leaves alone.
//!
//! The scene is built out of the two records the pass actually reads, uploaded
//! rather than rendered: an id plane written texel by texel, and a
//! silhouette mask written the same way. That is deliberate and it is
//! `crate::gbuffer`'s own argument for making a plane `COPY_DST` — this
//! test is about a rule over those two records, and drawing sprites to produce
//! them would only be a slower way of producing the same bytes, with a whole art
//! pipeline in between the assertion and what it is about.
//!
//! The rule, in one sentence: **the wash is the mask, plus the ground of the
//! selected tile — and nothing else standing on that tile.** The last clause is
//! what a screen-space pass keyed on the tile alone gets wrong, and it is the
//! reason the mask exists at all, so it is the assertion this file is built
//! around.
//!
//! Gated on an adapter, like every other GPU test here, and an honest skip
//! rather than a failure where there is none.

use openshard_client_render::blit::ViewportRect;
use openshard_client_render::gbuffer;
use openshard_client_render::place::{
    Kind,
    Place,
    Stance,
};
use openshard_client_render::select::{
    Frame as SelectFrame,
    Select,
    Selection,
};
use openshard_client_render::sprite::SpriteQuad;

/// The world image, and the surface: one size, so the pass's `uv` mapping is the
/// identity and every assertion is about the rule rather than about sampling.
/// A multiple of 64, which is what makes a row copy 256-byte aligned.
const SIZE: u32 = 64;

/// The tile the selection is on, and its neighbour — which must come out
/// untouched however the comparison is written.
const SELECTED: (u16, u16) = (1000, 2000);
const NEIGHBOUR: (u16, u16) = (1001, 2000);

/// What the target holds before the wash, so that "unchanged" is a value and not
/// an absence. Mid-grey rather than black: a wash blended into black is the wash
/// itself, and the two would be indistinguishable.
const UNDER: [u8; 4] = [90, 90, 90, 255];

/// A GPU to draw with, or `None` where there is none.
fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: openshard_client_render::gbuffer::required_limits(),
        ..Default::default()
    }))
    .ok()
}

/// A land texel: `id` is an id into [`ground_rows`], not a tile — since
/// `docs/archive/render/gbuffer.md` step 7, the ground half of what step 6 did for a static.
fn land_texel(id: u32) -> u32 {
    gbuffer::pack_ids(id, Stance::Flat, Kind::Land)
}

/// A static texel: `id` is an id into [`face_rows`], not a tile — the whole
/// point of this test since step 6.
fn static_texel(id: u32, stance: Stance) -> u32 {
    gbuffer::pack_ids(id, stance, Kind::Static)
}

/// The only row [`scene`]'s statics need: both static bands stand on
/// [`SELECTED`], so one id serves them both. Only `place.x` (the packed word's
/// low half, `x | y << 16`) is ever read back by `select.wgsl` — the row's
/// `kind`/`stance`/`z` are not, so [`Place::land`] is as good a source for them
/// as any other constructor.
fn face_rows() -> Vec<u8> {
    let mut bytes = Vec::new();
    SpriteQuad {
        rect:    openshard_client_render::geometry::Rect {
            x:      0.0,
            y:      0.0,
            width:  0.0,
            height: 0.0,
        },
        region:  openshard_client_render::atlas::Region {
            u:  0.0,
            v:  0.0,
            du: 0.0,
            dv: 0.0,
        },
        depth:   0.0,
        hue:     0,
        place:   Place::land(SELECTED.0, SELECTED.1),
        twin:    0,
        owner:   0,
        volumes: openshard_client_render::impostor::Range::default(),
    }
    .write(&mut bytes);
    bytes
}

/// The id [`face_rows`] gives [`SELECTED`] — its only row, so its own place in
/// the buffer.
const SELECTED_ID: u32 = 0;

/// [`scene`]'s land rows: [`SELECTED`] and [`NEIGHBOUR`], one each — unlike
/// the statics, the two land bands are two different tiles and need two
/// different ids. `docs/archive/render/gbuffer.md` step 7, the ground half of [`face_rows`].
fn ground_rows() -> Vec<u8> {
    let mut bytes = Vec::new();
    for tile in [SELECTED, NEIGHBOUR] {
        openshard_client_render::ground::GroundQuad {
            x:       0.0,
            y:       0.0,
            corners: [0.0; 4],
            region:  openshard_client_render::atlas::Region {
                u:  0.0,
                v:  0.0,
                du: 0.0,
                dv: 0.0,
            },
            texmap:  None,
            depth:   0.0,
            place:   Place::land(tile.0, tile.1),
        }
        .write(&mut bytes);
    }
    bytes
}

/// The ids [`ground_rows`] gives [`SELECTED`] and [`NEIGHBOUR`] — first sight,
/// in the order written above.
const SELECTED_GROUND_ID: u32 = 0;
const NEIGHBOUR_GROUND_ID: u32 = 1;

/// Which band of rows each kind of pixel occupies. Bands rather than scattered
/// texels so that a failure can be read as "this band is wrong" instead of as a
/// coordinate.
const BANDS: [(u32, u32); 4] = [(0, 16), (16, 32), (32, 48), (48, 64)];

/// The four bands, in order:
///
/// 0. the land of the selected tile — the outdoor case;
/// 1. a *flat static* on it, which is a wooden floor or a rug: the indoor case,
///    where the land is under a floor and never drawn;
/// 2. an *upright static* on it — a second wall standing on the very same tile,
///    which is the thing that must not be washed;
/// 3. the land of the tile next door.
fn scene() -> Vec<u32> {
    let mut texels = Vec::with_capacity((SIZE * SIZE) as usize);
    for y in 0..SIZE {
        let texel = match y {
            y if y < BANDS[0].1 => land_texel(SELECTED_GROUND_ID),
            y if y < BANDS[1].1 => static_texel(SELECTED_ID, Stance::Flat),
            y if y < BANDS[2].1 => static_texel(SELECTED_ID, Stance::Upright),
            _ => land_texel(NEIGHBOUR_GROUND_ID),
        };
        for _ in 0..SIZE {
            texels.push(texel);
        }
    }
    texels
}

/// The left half of band 2: the picked wall's own visible pixels, as the
/// silhouette pass would have left them. The right half of that band is the
/// *other* wall on the same tile.
fn mask_id(x: u32, y: u32) -> u8 {
    let (top, bottom) = BANDS[2];
    match (top..bottom).contains(&y) && x < SIZE / 2 {
        true => 1,
        false => 0,
    }
}

/// Run the pass over the uploaded scene and read the surface back as RGBA8 rows.
fn wash(device: &wgpu::Device, queue: &wgpu::Queue, selection: Selection) -> Vec<[u8; 4]> {
    // The whole set rather than a texture of the right format made by hand:
    // this pass reads one plane of it, and a fixture that built its own would
    // be a second author of a format `crate::gbuffer` owns.
    let gbuffer = openshard_client_render::gbuffer::Gbuffer::new(device, SIZE, SIZE);
    let ids_bytes: Vec<u8> = scene().iter().flat_map(|word| word.to_le_bytes()).collect();
    queue.write_texture(
        gbuffer.ids().as_image_copy(),
        &ids_bytes,
        wgpu::TexelCopyBufferLayout {
            offset:         0,
            bytes_per_row:  Some(SIZE * 4),
            rows_per_image: Some(SIZE),
        },
        wgpu::Extent3d {
            width:                 SIZE,
            height:                SIZE,
            depth_or_array_layers: 1,
        },
    );

    // The mask, in the same format the silhouette pass writes and with
    // `COPY_DST` on top of it: `outline::mask_texture` does not ask for that
    // usage, because nothing but a test ever writes one from the CPU.
    let mask = device.create_texture(&wgpu::TextureDescriptor {
        label:           Some("selection mask"),
        size:            wgpu::Extent3d {
            width:                 SIZE,
            height:                SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count:    1,
        dimension:       wgpu::TextureDimension::D2,
        format:          openshard_client_render::outline::MASK_FORMAT,
        usage:           wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats:    &[],
    });
    let ids: Vec<u8> = (0..SIZE)
        .flat_map(|y| (0..SIZE).map(move |x| mask_id(x, y)))
        .collect();
    queue.write_texture(
        mask.as_image_copy(),
        &ids,
        wgpu::TexelCopyBufferLayout {
            offset:         0,
            bytes_per_row:  Some(SIZE),
            rows_per_image: Some(SIZE),
        },
        wgpu::Extent3d {
            width:                 SIZE,
            height:                SIZE,
            depth_or_array_layers: 1,
        },
    );

    let format = wgpu::TextureFormat::Rgba8Unorm;
    let surface = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("surface"),
        size: wgpu::Extent3d {
            width:                 SIZE,
            height:                SIZE,
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
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    // The picture the wash goes over, which here is one flat colour: this pass
    // never reads the world image, only writes over it, so what is under the
    // wash is the whole of the scene it needs.
    encoder
        .begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("under"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view:           &surface_view,
                depth_slice:    None,
                resolve_target: None,
                ops:            wgpu::Operations {
                    load:  wgpu::LoadOp::Clear(wgpu::Color {
                        r: f64::from(UNDER[0]) / 255.0,
                        g: f64::from(UNDER[1]) / 255.0,
                        b: f64::from(UNDER[2]) / 255.0,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            multiview_mask: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        })
        .forget_lifetime();

    // `face_rows`, on the GPU — what the two static bands' ids resolve
    // through, the same way `window.statics.instances_buffer()` is what a real
    // frame binds.
    let rows = face_rows();
    let face_instances = device.create_buffer(&wgpu::BufferDescriptor {
        label:              Some("select test face instances"),
        size:               rows.len() as u64,
        usage:              wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&face_instances, 0, &rows);

    // `ground_rows`, on the GPU — what the two land bands' ids resolve
    // through, the same way `window.renderer.instances_buffer()` is what a
    // real frame binds.
    let ground_rows = ground_rows();
    let ground_instances = device.create_buffer(&wgpu::BufferDescriptor {
        label:              Some("select test ground instances"),
        size:               ground_rows.len() as u64,
        usage:              wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&ground_instances, 0, &ground_rows);

    Select::new(device, format).render(
        device,
        queue,
        &mut encoder,
        SelectFrame {
            target:           &surface_view,
            mask:             &mask.create_view(&wgpu::TextureViewDescriptor::default()),
            ids:              &gbuffer.views().ids,
            face_instances:   &face_instances,
            ground_instances: &ground_instances,
            size:             (SIZE, SIZE),
            rect:             ViewportRect {
                x:      0,
                y:      0,
                width:  SIZE,
                height: SIZE,
            },
        },
        selection,
    );
    queue.submit([encoder.finish()]);

    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label:              Some("readback"),
        size:               u64::from(SIZE) * u64::from(SIZE) * 4,
        usage:              wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        surface.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset:         0,
                bytes_per_row:  Some(SIZE * 4),
                rows_per_image: Some(SIZE),
            },
        },
        wgpu::Extent3d {
            width:                 SIZE,
            height:                SIZE,
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
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|pixel| [pixel[0], pixel[1], pixel[2], pixel[3]])
        .collect()
}

/// What a premultiplied wash of `color` over [`UNDER`] comes to.
///
/// The blend the pipeline is built with — `dst = src.rgb + dst * (1 - src.a)`
/// with `src.rgb` already scaled by its alpha — written out here rather than
/// trusted: this is the one number in the pass a person can be wrong about and
/// still see a plausible highlight.
fn washed(color: [f32; 4]) -> [u8; 3] {
    let over = |channel: usize, under: u8| {
        let under = f32::from(under) / 255.0;
        let value = color[channel] * color[3] + under * (1.0 - color[3]);
        (value * 255.0).round() as u8
    };
    [over(0, UNDER[0]), over(1, UNDER[1]), over(2, UNDER[2])]
}

/// Every pixel of the scene, against the rule stated in the module docs.
///
/// One test and not four, because the four bands are one claim: what is washed
/// is what is washed *instead of* the others, and a test that only looked at the
/// pixels it expected to change would pass on a pass that washed the whole
/// screen.
#[test]
fn the_wash_covers_the_selected_sprite_and_its_ground_and_nothing_else() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    let selection = Selection::DEFAULT.on(openshard_map::grid::Tile::new(SELECTED.0, SELECTED.1));
    let frame = wash(&device, &queue, selection);

    let sprite = washed(selection.sprite);
    let ground = washed(selection.ground);
    // The three outcomes are distinguishable, or the assertions below are
    // satisfied by a pass that cannot tell them apart. A companion, and this
    // repository has produced the green without one before.
    assert_ne!(sprite, ground, "the two washes are the same colour");
    assert_ne!(
        ground,
        [UNDER[0], UNDER[1], UNDER[2]],
        "the ground wash is invisible"
    );

    let mut counted = [0usize; 3];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let got = frame[(y * SIZE + x) as usize];
            let (want, band) = match (mask_id(x, y) != 0, y) {
                // The picked wall's own pixels.
                (true, _) => (sprite, 0),
                // The land of its tile, and the floor lying on that same tile:
                // both are "the ground here", and indoors only the second is
                // ever drawn.
                (false, y) if y < BANDS[0].1 || (BANDS[1].0..BANDS[1].1).contains(&y) => (ground, 1),
                // Everything else: the *other* wall standing on the selected
                // tile, and the tile next door. Untouched — the first is what
                // separates this from a wash keyed on the tile alone.
                _ => ([UNDER[0], UNDER[1], UNDER[2]], 2),
            };
            counted[band] += 1;
            for channel in 0..3 {
                let (got, want) = (i32::from(got[channel]), i32::from(want[channel]));
                assert!(
                    (got - want).abs() <= 1,
                    "({x}, {y}) channel {channel}: {got} against {want} — band {band}",
                );
            }
            assert_eq!(got[3], 255, "({x}, {y}) lost the frame's own alpha");
        }
    }
    // And each band had something in it. Without this the loop above is happy
    // with a scene where two of the three cases never occur, which is exactly
    // how a rule test passes without testing its rule.
    for (band, count) in counted.iter().enumerate() {
        assert!(*count > 0, "band {band} was never reached");
    }
}

/// A selection with nothing under the cursor washes nothing at all.
///
/// The frame the client draws almost always: worth an assertion because the
/// *absence* is what the wash costs nothing for, and a pass that washed the
/// whole screen when no tile was named would be a client that turns cyan on a
/// click on open ground.
#[test]
fn a_selection_with_no_tile_leaves_the_ground_alone() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    let frame = wash(&device, &queue, Selection::DEFAULT);
    let sprite = washed(Selection::DEFAULT.sprite);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let got = frame[(y * SIZE + x) as usize];
            // The mask is still the mask: what "no tile" switches off is the
            // ground, and the thing itself is washed either way. That is the
            // distinction the uniform's third word carries.
            let want = match mask_id(x, y) != 0 {
                true => sprite,
                false => [UNDER[0], UNDER[1], UNDER[2]],
            };
            for channel in 0..3 {
                let (got, want) = (i32::from(got[channel]), i32::from(want[channel]));
                assert!((got - want).abs() <= 1, "({x}, {y}) channel {channel}");
            }
        }
    }
}
