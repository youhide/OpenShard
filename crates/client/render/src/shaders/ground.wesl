// The ground pass: one textured quad per land tile.
//
// Deliberately inside WebGL2's ceiling — vertex-buffer instancing, one uniform
// block, one sampled texture, no storage buffers and no compute. The unit quad
// arrives as a vertex buffer rather than being derived from `vertex_index`,
// because a pipeline with no vertex attributes is a place WebGL2 backends have
// historically differed.
//
// A tile has two shapes, and which one it gets is decided here rather than on
// the CPU, so the decision cannot disagree with the heights it is made from:
//
//   * Four equal corners — a *flat* tile. Drawn as the art's own 44x44 square,
//     texel for texel, with the diamond cut out of it by alpha. This is the
//     common case and the one a frame test can compare against the file.
//   * Anything else — a slope. Drawn as the diamond itself: four vertices at
//     the tile's four corners, each lifted by its own height. Neighbouring
//     tiles then share vertices instead of merely abutting, so a seam between
//     them is not expressible.
//
// On level ground the two agree exactly: the diamond's vertices land on the
// square's edge midpoints and the texture map is the identity.
//
// A slope is textured from `texmaps.mul` and not from the art. The art is a
// 44x44 diamond drawn for level ground, and pulling it over a steep quad smears
// it; the texture maps are square pictures of the same terrain, made to be
// mapped corner to corner onto exactly this shape. Which corner is which is
// ClassicUO's `DrawStretchedLand`, and it is the identity here: the unit quad's
// (0,0) is the tile's top vertex and the texture's top-left, (1,0) the right,
// (0,1) the left, (1,1) the bottom.
//
// A tile whose graphic has no texture — most of them do not — falls back to the
// stretched art. The client instead refuses to stretch such a tile at all and
// draws it flat, which leaves gaps against its stretched neighbours; ours stays
// watertight and is textured approximately. See docs/client.md.

import package::place_format::{KIND_LAND, STANCE_FLAT, pack_place};

struct Viewport {
    // The target's size in *real* pixels — the display's, which at a
    // magnification above 1 is not the world's own. Only the last two lines of
    // the vertex shader use it.
    size: vec2<f32>,
    // The size of one land sprite in virtual pixels: 44x44, from the file
    // format.
    tile: vec2<f32>,
    // Virtual pixels one unit of height lifts a corner up the screen.
    z_step: f32,
    // Real pixels per virtual pixel. One when the world is being drawn into the
    // offscreen image, which is the minifying path — see `camera::Projection`.
    scale: f32,
    // The point of the world's own grid that lands in the middle of the target,
    // in virtual pixels. Fractional: the eye is quantised to a real pixel, so at
    // 3x it carries thirds of a virtual one.
    origin: vec2<f32>,
};

@group(0) @binding(0) var<uniform> viewport: Viewport;
@group(0) @binding(1) var atlas_texture: texture_2d<f32>;
@group(0) @binding(2) var atlas_sampler: sampler;
@group(0) @binding(3) var texmap_texture: texture_2d<f32>;

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    // Constant across the instance, so ordinary interpolation reproduces it
    // exactly and no interpolation attribute is needed to keep it intact.
    @location(1) is_flat: f32,
    // Where to sample the texture atlas, and whether there is anything there.
    @location(2) texmap_uv: vec2<f32>,
    @location(3) has_texmap: f32,
    // This instance's own index — what the attachment now carries in place of
    // the tile, and what `blit.wgsl`/`select.wgsl` look up in this pass's own
    // instance buffer, bound a second time as storage. See `docs/gbuffer.md`
    // decision 2 and step 7.
    @location(4) @interpolate(flat) id: u32,
    // Where in that tile, as the diamond's own two axes in `0..1`. Interpolated,
    // and it is what makes a pool of light a gradient rather than a set of flat
    // tiles with steps between them: a tile is 44 pixels across and a fragment
    // lit by its tile's *centre* is lit in blocks the size of the ground.
    @location(6) local: vec2<f32>,
    // And the height, which is *not* flat: a hillside's pixels each carry the
    // one their corner interpolation gave them, which is the height the light
    // has to reach. Taking the tile's base instead would light a hilltop as if
    // it were the valley floor.
    @location(5) place_z: f32,
};

// What one fragment of a world pass writes: the picture, and where in the world
// it came from.
struct FragmentOut {
    @location(0) color: vec4<f32>,
    @location(1) place: vec4<u32>,
};

@vertex
fn vs_main(
    // The unit quad: (0,0) to (1,1). (0,0) is the tile's own corner, and the
    // two axes are the map's: `x` runs down-right on screen, `y` down-left.
    @location(0) corner: vec2<f32>,
    // Per instance: the diamond's centre at height zero, in viewport pixels.
    @location(1) origin: vec2<f32>,
    // Per instance: the four corner heights, indexed `a + 2*b`.
    @location(2) heights: vec4<f32>,
    // Per instance: its place in the atlas, as (u, v, du, dv).
    @location(3) region: vec4<f32>,
    // Per instance: its place in the texture atlas, the same way. A size of
    // zero means the tile has no texture map.
    @location(4) texmap: vec4<f32>,
    // Per instance: where this tile sorts against everything else in the frame,
    // statics included. Computed on the CPU — see `crate::depth` — because it
    // is one ordering shared by two passes and neither may derive its own.
    @location(5) depth: f32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOut {
    let half = viewport.tile * 0.5;
    let is_flat = f32(all(heights == vec4<f32>(heights.x)));

    // Bilinear over the unit square, which is exact at the four corners — the
    // only places it is evaluated.
    let height = mix(
        mix(heights.x, heights.y, corner.x),
        mix(heights.z, heights.w, corner.x),
        corner.y,
    );

    // The square: the sprite's own bounding box, centred on the tile.
    let flat_offset = (corner - vec2<f32>(0.5)) * viewport.tile;
    let flat_uv = corner;

    // The diamond: (0,0) is the top vertex, (1,0) the right, (0,1) the left and
    // (1,1) the bottom — one half-tile out from the centre along each screen
    // axis, which is the same projection the CPU used to place the centre.
    let slope_offset = vec2<f32>(
        (corner.x - corner.y) * half.x,
        (corner.x + corner.y - 1.0) * half.y,
    );
    let slope_uv = vec2<f32>(
        0.5 + (corner.x - corner.y) * 0.5,
        (corner.x + corner.y) * 0.5,
    );

    let offset = mix(slope_offset, flat_offset, is_flat);
    let local_uv = mix(slope_uv, flat_uv, is_flat);
    let pixel = origin + offset - vec2<f32>(0.0, height * viewport.z_step);

    // Virtual pixels to real ones, and the only line in this pass that knows a
    // magnification exists. Everything above is the art's own grid, which is
    // what makes it comparable against the files texel for texel.
    let real = (pixel - viewport.origin) * viewport.scale + viewport.size * 0.5;

    // Pixels to clip space. `y` flips: the viewport counts down from the top and
    // clip space counts up from the middle.
    let ndc = vec2<f32>(
        real.x / viewport.size.x * 2.0 - 1.0,
        1.0 - real.y / viewport.size.y * 2.0,
    );

    var out: VertexOut;
    // The whole tile shares one depth: a quad is one thing standing at one
    // place in the order, and a per-vertex depth would let a slope's own
    // corners sort against each other.
    out.clip = vec4<f32>(ndc, depth, 1.0);
    out.uv = region.xy + local_uv * region.zw;
    out.is_flat = is_flat;
    // Corner to corner: the unit quad's own coordinates *are* the texture's.
    out.texmap_uv = texmap.xy + corner * texmap.zw;
    out.has_texmap = f32(texmap.z > 0.0);
    out.id = instance_index;
    out.place_z = height;
    // The diamond's own axes, inverted from the screen offset this vertex is at:
    // `slope_offset` above is `((a - b) * 22, (a + b - 1) * 22)` for a tile-local
    // `(a, b)`, and this is that solved for the pair. Written from the offset
    // rather than passed as `corner` because a *flat* tile's quad is the
    // sprite's bounding square and not the diamond — its corners are half a tile
    // outside the tile, and only the offset says by how much.
    out.local = vec2<f32>(
        (offset.x + offset.y) / f32(viewport.tile.x) + 0.5,
        (offset.y - offset.x) / f32(viewport.tile.x) + 0.5,
    );
    return out;
}

// Where in the world this fragment's pixel is: the instance's id, the
// interpolated corner height, and "this is ground".
//
// **The tile itself no longer rides here** — `docs/gbuffer.md` step 7 moved it
// to this pass's own instance buffer, read back by `blit.wgsl`/`select.wgsl`
// as `ground_instances[id]`, the same move step 3 made for a static's tile.
// The height and the sub-tile fraction stay exactly where they were: both are
// this fragment's own, from the bilinear interpolation above, not a fact the
// whole instance shares — carrying either from the row would flatten exactly
// what they exist to vary.
fn place_of(in: VertexOut) -> vec4<u32> {
    return pack_place(in.id, in.place_z, STANCE_FLAT, KIND_LAND, in.local);
}

@fragment
fn fs_main(in: VertexOut) -> FragmentOut {
    var out: FragmentOut;
    out.place = place_of(in);

    // A slope with a texture of its own takes it, and nothing else about the
    // tile changes: the shape is still the diamond its four corners make.
    if in.is_flat < 0.5 && in.has_texmap > 0.5 {
        let textured = textureSample(texmap_texture, atlas_sampler, in.texmap_uv);
        out.color = vec4<f32>(textured.rgb, 1.0);
        return out;
    }

    let color = textureSample(atlas_texture, atlas_sampler, in.uv);

    // A flat tile's quad is the sprite's square, so a quarter of it is outside
    // the diamond and must not be drawn. Discarding rather than blending keeps
    // the pass independent of draw order, which is what lets a frame be
    // compared byte for byte.
    //
    // A slope's quad *is* the diamond, so every sample is inside it by
    // construction — the corner texture coordinates are the diamond's own
    // vertices and interpolation cannot leave their convex hull. Discarding
    // there would only let a texel's worth of rounding punch a hole through the
    // ground, so it does not.
    if in.is_flat > 0.5 && color.a < 0.5 {
        discard;
    }
    out.color = vec4<f32>(color.rgb, 1.0);
    return out;
}
