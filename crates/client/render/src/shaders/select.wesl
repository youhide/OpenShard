// The wash over what is selected: the picked sprite's own pixels, and the floor
// of the tile it stands on.
//
// Geometry is `blit.wgsl`'s and `outline.wgsl`'s: four corners from the vertex
// index, the render pass's viewport is the rectangle the UI left free, so `uv`
// means the same thing in all three and a world texel is found from a surface
// pixel with one multiplication.
//
// Two questions are asked per fragment, and they are asked of two different
// records because they are two different facts:
//
//   1. **Is this pixel the selected thing?** That is the mask: the silhouette
//      pass drew the picked static's quad into it, depth-tested against the
//      world, so what is in there is what is *visible* of it. The G-buffer's
//      id plane (`docs/archive/render/gbuffer.md` step 3) now *can* tell two statics on
//      one tile apart from each other, but not this pixel from the picked one —
//      the silhouette is its own tiny draw with its own instance numbering, not
//      a second use of the world pass's ids, so there is no id here to compare
//      against. See `docs/archive/render/gbuffer.md`'s "not settled" item on whether
//      `render_mask` shrinks.
//   2. **Is this pixel the ground that thing stands on?** That is the id plane,
//      which names whatever was drawn at each pixel — so the answer is a
//      comparison, and needs no second mask, but never a
//      *direct* one: every kind's pixel carries an id rather than a tile,
//      into `face_instances` for a static (`docs/archive/render/gbuffer.md` step 3, see step
//      6) or `ground_instances` for the land (step 7), so the tile is one more
//      lookup away either way.
//
// The floor is *land or a flat static* of that tile, not land alone. Indoors the
// land is under a wooden floor and never drawn, so "the tile the wall stands on"
// would come out unshaded in exactly the building the player is pointing at. A
// flat static is a floor, a rug or a road — `Stance::Flat`, which is the client's
// own `Background` bit — and it is what is visibly *the ground here*.

import package::place_format::{KIND_LAND, KIND_STATIC, STANCE_FLAT, ids_kind, ids_stance, ids_id};

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOut {
    let corner = vec2<f32>(f32(index & 1u), f32((index >> 1u) & 1u));
    var out: VertexOut;
    out.position = vec4<f32>(corner.x * 2.0 - 1.0, 1.0 - corner.y * 2.0, 0.0, 1.0);
    out.uv = corner;
    return out;
}

// Which selected object each world pixel belongs to, or zero for none. Its own
// mask and not the outline's: the ring pass draws an edge round every id it
// finds, so a selection sharing that texture would be ringed as well as shaded.
@group(0) @binding(0) var mask: texture_2d<u32>;
// What drew each world pixel — `crate::gbuffer`'s id plane, the same one the
// lighting reads, and `place_format.wesl` is the layout.
@group(0) @binding(1) var ids_of: texture_2d<u32>;

// The statics pass's own instance buffer, bound a second time as storage —
// `docs/archive/render/gbuffer.md` decision 2 and step 3, the same buffer `blit.wgsl` reads
// as `face_instances`. A `Kind::Static` pixel's `place.x`/`place.y` is an id
// into this, not a tile; only its `place` field is read here; the rest is
// declared so the struct's size matches `SpriteQuad::STRIDE` and every row
// past the first is not read at the wrong offset. See `docs/archive/render/gbuffer.md` step
// 6.
struct FaceInstance {
    rect: vec4<f32>,
    region: vec4<f32>,
    depth: f32,
    hue: u32,
    place: vec2<u32>,
    twin: u32,
};

@group(0) @binding(3) var<storage, read> face_instances: array<FaceInstance>;

// The ground pass's own instance buffer, bound a second time as storage —
// `docs/archive/render/gbuffer.md` step 7, the ground half of what step 3 did for a static's
// tile above. Scalars rather than `FaceInstance`'s `vec4`s, for the reason
// `blit.wgsl`'s own copy of this struct gives: `GroundQuad::write`'s first
// field is only eight bytes wide, and a `vec4` right after it would force
// WGSL to open a gap the real bytes do not have. Only `place0` is read; the
// rest is declared so the struct's size matches `GroundQuad::STRIDE`.
struct GroundInstance {
    origin_x: f32,
    origin_y: f32,
    corner_a: f32,
    corner_b: f32,
    corner_c: f32,
    corner_d: f32,
    region_u: f32,
    region_v: f32,
    region_du: f32,
    region_dv: f32,
    texmap_u: f32,
    texmap_v: f32,
    texmap_du: f32,
    texmap_dv: f32,
    depth: f32,
    place0: u32,
    place1: u32,
};

@group(0) @binding(4) var<storage, read> ground_instances: array<GroundInstance>;

struct Selection {
    // The mask's size in texels, which is the world image's; the rest is
    // padding.
    shape: vec4<f32>,
    // The selected tile as (x, y), then whether there is one at all. Zero is
    // "no tile", which is the frame where something is selected that is not
    // standing anywhere — there is none today, and it is one comparison rather
    // than a second pipeline.
    tile: vec4<u32>,
    // The wash over the selected thing's own pixels: colour, and `a` is how much
    // of it replaces what is under it.
    sprite: vec4<f32>,
    // And over the ground it stands on.
    ground: vec4<f32>,
};

@group(0) @binding(2) var<uniform> selection: Selection;

// What the mask holds where nothing was drawn.
const EMPTY: u32 = 0u;

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let size = vec2<i32>(selection.shape.xy);
    // Floor rather than round, which is `outline.wgsl`'s reason exactly: `uv`
    // runs 0..1 across the whole image, so this is the nearest-texel mapping the
    // magnifying blit does, and the wash is blocky in step with the art rather
    // than half a texel off it.
    let at = vec2<i32>(in.uv * selection.shape.xy);
    if at.x < 0 || at.y < 0 || at.x >= size.x || at.y >= size.y {
        discard;
    }

    var wash = vec4<f32>(0.0);
    if textureLoad(mask, at, 0).r != EMPTY {
        wash = selection.sprite;
    } else if selection.tile.z != 0u {
        let ids = textureLoad(ids_of, at, 0).x;
        let kind = ids_kind(ids);
        let stance = ids_stance(ids);
        let ground = kind == KIND_LAND || (kind == KIND_STATIC && stance == STANCE_FLAT);
        // The **tile** itself, never the id directly: every kind's pixel
        // carries an id into its own instance buffer, not a tile — see
        // `docs/archive/render/gbuffer.md` step 3 for a static, step 7 for the ground.
        let id = ids_id(ids);
        var tile = vec2<u32>(0u);
        if kind == KIND_LAND {
            tile = vec2<u32>(ground_instances[id].place0 & 0xFFFFu, ground_instances[id].place0 >> 16u);
        } else if kind == KIND_STATIC {
            let row = face_instances[id].place.x;
            tile = vec2<u32>(row & 0xFFFFu, row >> 16u);
        }
        if ground && tile.x == selection.tile.x && tile.y == selection.tile.y {
            wash = selection.ground;
        }
    }

    if wash.a <= 0.0 {
        discard;
    }
    // Premultiplied, like the ring pass and for the same reason: the target's
    // blend is `dst = src.rgb + dst * (1 - src.a)`, so a colour scaled by its own
    // alpha is the ordinary blend and the picture shows through by whatever is
    // left. A wash and not a replacement — the art has to stay readable under it,
    // or the selection has hidden the thing it is pointing at.
    return vec4<f32>(wash.rgb * wash.a, wash.a);
}
