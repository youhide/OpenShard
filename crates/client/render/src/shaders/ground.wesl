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

import package::place_format::{KIND_LAND, STANCE_FLAT, pack_ids};
import package::normal_format::pack_normal;
import package::solid_format::SOLID_NONE;

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
    // instance buffer, bound a second time as storage. See `docs/archive/render/gbuffer.md`
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
    @location(5) world_z: f32,
    // This tile's own `(x, y)`, flat across the instance — the same two numbers
    // `blit.wgsl` fetches back out of `ground_instances[id].place0`, carried
    // here because a fragment's *world position* is this plus `local`, and that
    // is per pixel. See `FragmentOut::position`.
    @location(7) @interpolate(flat) tile: vec2<f32>,
    // And the four corner heights, because the *normal* is per pixel on a
    // bilinear patch and its two derivatives are not constant across one.
    //
    // Flat rather than interpolated for the reason `heights` is a per-instance
    // attribute in the first place: they are the tile's, not this vertex's, and
    // interpolating them would give a fragment four corners it does not have.
    //
    // **In tiles, divided on the way out of the vertex stage**, which is where
    // the two numbers that do the dividing are. `viewport` is bound to the
    // vertex stage alone and there is no reason to widen it to the fragment one
    // for a constant: the fragment shader wants these in the isotropic metric
    // and in no other, so the conversion belongs to the producer.
    @location(8) @interpolate(flat) heights_in_tiles: vec4<f32>,
};

// What one fragment of a world pass writes: the picture, and where in the world
// it came from.
struct FragmentOut {
    @location(0) color: vec4<f32>,
    // What drew this fragment and which row it came from —
    // `crate::gbuffer::IDS_FORMAT`.
    @location(1) ids: u32,
    // The same fragment as a point rather than as an id and a fraction —
    // `crate::gbuffer`.
    @location(2) position: vec4<f32>,
    // And which way it looks — for land, the bilinear patch's own normal, packed
    // by `normal_format.wesl`. See `land_normal`.
    @location(3) normal: u32,
};

// The land's own normal, from the four corner heights the tile was lifted by.
//
// The surface is the bilinear patch `vs_main` puts its vertices on, so this is
// the cross product of that patch's two tangents:
//
//   h(a, b)  = mix(mix(h00, h10, a), mix(h01, h11, a), b)
//   dh/da    = mix(h10 - h00, h11 - h01, b)
//   dh/db    = mix(h01 - h00, h11 - h10, a)
//   Ta x Tb  = (1, 0, dh/da) x (0, 1, dh/db) = (-dh/da, -dh/db, 1)
//
// The heights arrive **in tiles** (`VertexOut::heights_in_tiles`), because a
// normal is a direction in the *isotropic* world `blit.wgsl` states a
// fragment-to-flame offset in — one `z` unit is an eleventh of a tile there. In
// raw units a one-unit step across a tile would read as forty-five degrees.
//
// A flat tile's four heights are equal, both derivatives are zero, and this is
// exactly `(0, 0, 1)` — the vector a level surface has, arrived at
// rather than special-cased.
fn land_normal(heights: vec4<f32>, at: vec2<f32>) -> vec3<f32> {
    let along_x = mix(heights.y - heights.x, heights.w - heights.z, at.y);
    let along_y = mix(heights.z - heights.x, heights.w - heights.y, at.x);
    return normalize(vec3<f32>(-along_x, -along_y, 1.0));
}

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
    // Per instance: `crate::place::Place::packed`'s first word, the tile as
    // `x | y << 16`. The second word is not declared — nothing in this pass
    // reads a ground quad's `z`, kind or stance.
    @location(6) place0: u32,
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
    //
    // **`floor`, and it is the whole of `docs/render/design_frame_assembly.md`'s window-parity
    // entry.** The world is centred on the image, and the middle of an image
    // with an *odd* extent is the middle of a pixel rather than the join
    // between two. Centred there, a fragment sampling at `i + 0.5` lands on a
    // whole virtual pixel — every column at `1:1`, every `scale`-th one
    // magnified — and a box's own corner is at a whole virtual pixel by
    // construction, so a ray passes exactly through it and `impostor::meets`
    // answers the tie with a face that has no area from here. That is a green
    // line down every wall in the live client and in no tool ever built, since
    // every tool's viewport has been even.
    //
    // Floored, the world's centre is on a pixel join at every extent, the
    // sample sits at a half-integer over `scale`, and no integer `scale`
    // divides a half-integer — so no primary sample can land on a whole
    // virtual pixel at any rung of the ladder. That is a proof rather than a
    // margin, which is why it is here and not a nudge somewhere. It costs half
    // a real pixel of centring at an odd extent, which is below the quantum
    // `Zoom` is built around, and it is a no-op at an even one — `floor` of a
    // whole number. The other two passes end on this same line and must keep
    // ending on it: two passes centring differently is two pictures.
    let real = (pixel - viewport.origin) * viewport.scale + floor(viewport.size * 0.5);

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
    out.world_z = height;
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
    out.tile = vec2<f32>(f32(place0 & 0xFFFFu), f32(place0 >> 16u));
    // 44 virtual pixels of tile over 4 a unit of height: `light::Z_PER_TILE`'s
    // own derivation, off the two numbers this pass is already handed rather
    // than as a third written-down copy of the constant.
    out.heights_in_tiles = heights / (viewport.tile.x / viewport.z_step);
    return out;
}

// Who drew this fragment: the instance's id, "this lies in its tile", and
// "this is ground".
//
// **The tile itself does not ride here** — `docs/archive/render/gbuffer.md` step 7 moved it
// to this pass's own instance buffer, read back by `blit.wgsl`/`select.wgsl`
// as `ground_instances[id]`, the same move step 3 made for a static's tile.
// Neither does the height or the sub-tile fraction any more: both are this
// fragment's own rather than a fact the whole instance shares, so both are in
// the position plane below, unquantised. `docs/render/design_model.md` phase 2.
fn ids_of(in: VertexOut) -> u32 {
    return pack_ids(in.id, STANCE_FLAT, KIND_LAND);
}

@fragment
fn fs_main(in: VertexOut) -> FragmentOut {
    var out: FragmentOut;
    out.ids = ids_of(in);
    // And the fragment as a point. Clamped into the tile its id names — a flat
    // tile's quad is the sprite's 44x44 square, whose corners lie outside the
    // diamond, and a fragment there — one the alpha cut below has not reached
    // yet — would otherwise name a neighbour nothing was drawn on.
    let local = clamp(in.local, vec2<f32>(0.0), vec2<f32>(1.0));
    // The fourth channel names the solid this fragment is a point of, and the
    // ground is a point of none: land is no occluder, so it is exempt from
    // nothing — which is what it has always been in effect.
    // `solid_format.wesl`.
    out.position = vec4<f32>(in.tile + local, in.world_z, SOLID_NONE);
    // And the normal, which land now has and which it did not before.
    //
    // **This was a deliberate zero and phase 3 is what changed it.** The old
    // reason was the shading term: it was a *half-space* — a flame on the wrong
    // side of a surface's plane lit it not at all, softened over a fifth of a
    // tile — and a floor is the one surface a flame is routinely almost in the
    // plane of, so gating land blacked out every ground pixel a fixture was not
    // comfortably above. That is a defect of the half-space and not of the
    // normal: an angular cosine is small at a grazing flame rather than absent,
    // which is what a floor lit from a torch on it actually looks like.
    // `docs/render/design_model.md`'s third open question, answered by having one.
    out.normal = pack_normal(land_normal(in.heights_in_tiles, local));

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
