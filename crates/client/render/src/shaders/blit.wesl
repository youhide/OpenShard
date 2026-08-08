// The world image, onto the viewport — and the lighting, on the way past.
//
// No vertex buffer: four corners derived from the vertex index, which is a
// triangle strip covering clip space. The render pass sets the viewport rect, so
// "clip space" is whatever rectangle the UI left free for the world.
//
// The lighting here is in *world* coordinates and not in this image's pixels:
// every fragment reads which tile its picture came from out of the place
// attachment (see `crate::place`) and is lit as that tile is lit. The screen
// folds height into `y` and a wall's sprite stands above the tile it occludes
// from, so nothing decided from a screen position can tell the lit face of a
// wall from the shadow behind it. `docs/lighting.md` is the argument.

import package::place_format::{
    KIND_MASK, KIND_NOTHING, KIND_LAND, KIND_STATIC, KIND_MOBILE,
    SUB_TILE_MASK, SUB_TILE, PLACE_STANCE_SHIFT, PLACE_STANCE_MASK,
    unpack_place_z, STANCE_MESH_FACE, STANCE_FLAT, STANCE_FACE_NORTH,
    STANCE_FACE_EAST, STANCE_FACE_SOUTH, STANCE_FACE_WEST,
};

import package::tonemap::shade;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOut {
    // (0,0) (1,0) (0,1) (1,1), the same unit quad the world passes use.
    let corner = vec2<f32>(f32(index & 1u), f32((index >> 1u) & 1u));
    var out: VertexOut;
    // Clip space is y-up and the image is y-down, so the vertical axis flips
    // here rather than in the sampling — one negation, in the place where the
    // convention actually changes.
    out.position = vec4<f32>(corner.x * 2.0 - 1.0, 1.0 - corner.y * 2.0, 0.0, 1.0);
    out.uv = corner;
    return out;
}

@group(0) @binding(0) var world: texture_2d<f32>;
@group(0) @binding(1) var world_sampler: sampler;
// Which tile each pixel of `world` came from: (id, z + 128, kind and the
// fraction) — an id into one of the three instance buffers below rather than
// a literal tile, since `docs/gbuffer.md` steps 3 and 7 moved it there for
// every kind. Never sampled — an integer texture has no filtering, and a
// place averaged with its neighbour would name a third tile nothing was drawn
// on.
@group(0) @binding(3) var place_of: texture_2d<u32>;
// Where each tile's references are, one texel a tile over the rectangle in
// `lighting.grid`: (offset & 255, offset >> 8, offset >> 16, count). The index
// of `docs/lighting.md`'s decision 30 — a count of zero is open ground, and
// nothing else says a tile stands nothing. What the offset points at is the id
// list at binding 8, which is step 23.1. See `crate::occlusion`.
@group(0) @binding(4) var occluders: texture_2d<u32>;
// What each of those tiles *is*, over the same rectangle: (sky, unused, unused,
// unused). Only the sky channel is written today. See `crate::occlusion`, and
// `docs/lighting_world.md` for why this is a second plane rather than four more
// channels of the cell above.
@group(0) @binding(5) var field: texture_2d<u32>;
// And the solids themselves, one texel each, folded into rows `LIST_ROW`
// wide: (z_bottom + 128, z_top + 128, opacity, PRESENT | HOLED | edges). A
// texture and not a storage buffer because the ceiling is WebGL2 — decision 30.5.
//
// The box's `x` and `y` are not in *this* plane — binding 13 below is where
// they arrived, once a reader existed for them.
@group(0) @binding(6) var solids: texture_2d<u32>;
// The hole in each of those solids, indexed by the same number:
// (near, far, z_bottom + 128, z_top + 128) — `occlusion::Aperture`. A plane
// beside the list rather than four more channels of it, because the list is what
// the walk reads cell after cell in a loop and a hole is what almost nothing
// has: the `HOLED` bit is what makes this read at all, so an ordinary wall pays
// one bit test and no fetch.
@group(0) @binding(7) var apertures: texture_2d<u32>;
// And the references between the cells and the solids, one texel each and folded
// the same way: (id & 255, id >> 8, id >> 16, unused). A cell counts through a
// run of these and each one names a solid — decision 38's ownership, which is
// what lets one box be referenced by every cell it stands over.
@group(0) @binding(8) var solid_ids: texture_2d<u32>;

// One flame. Three `vec4`s rather than nine fields, because a uniform array's
// stride is rounded up to 16 bytes either way and this is what the CPU writes:
//
//   place = (tile x, tile y, z, radius in tiles)
//   color = (r, g, b, intensity)
//   beam  = (axis x, axis y, axis z, cosine of the half-angle)
//
// A fire standing in the world lights every direction and writes a `beam` of
// `(0, 0, 0, -1)`: no cosine is below -1, so the test below is one comparison
// and the dot product is never reached. `crate::light::Beam`.
struct Light {
    place: vec4<f32>,
    color: vec4<f32>,
    beam: vec4<f32>,
};

// Sized to `Lighting::MAX`, and the two have to agree — this is a uniform
// array, so its length is fixed at compile time and a mismatch is a buffer
// wgpu rejects rather than a wrong picture.
const MAX_LIGHTS: u32 = 64u;

struct Lighting {
    // What a tile with an open column above it is multiplied by away from every
    // flame, then how many of `lights` are real. Scaled per fragment by the sky
    // channel of `field`, which is what makes the inside of a house darker than
    // the street with nothing in either — `crate::light::Ambient`.
    sky: vec4<f32>,
    // And what every tile gets, roof or no roof: the floor under the darkness,
    // so that an unlit room is deep rather than pure black. `(1,1,1,·)` sky with
    // a `(0,0,0,·)` ground over an empty grid is no lighting at all, and the
    // identity below is exact for it.
    ground: vec4<f32>,
    // The occlusion grid's rectangle: its lowest tile `x` and `y`, then its
    // width and height in tiles. A tile outside it occludes nothing, which is
    // right by construction — the grid is grown by the widest pool's reach, so
    // nothing outside it could shadow anything this frame draws.
    grid: vec4<i32>,
    // Which picture to draw: `crate::debug::View` in the first component, the
    // rest padding. Zero is the lit frame and every other value is a diagnostic
    // drawn from the values this pass lit it with — see `docs/lighting.md`,
    // decision 8. The numbers are pinned in a test on the Rust side.
    view: vec4<u32>,
    // Which way the sun is — `x` and `y` in tiles, `z` in tiles as well — and in
    // the fourth component the height above which nothing in this frame's grid
    // can stop it. A ray that has climbed past that is in the sky and stops
    // walking, which is what makes a daylit frame affordable.
    sun: vec4<f32>,
    // Its colour, then how much it adds. An intensity of zero is "no sun", and
    // it is the only thing tested: a night frame never enters the walk.
    sun_color: vec4<f32>,
    lights: array<Light, MAX_LIGHTS>,
};

// `crate::debug::View`, and the same numbers.
const VIEW_LIT: u32 = 0u;
const VIEW_PLACE: u32 = 1u;
const VIEW_KIND: u32 = 2u;
const VIEW_HEIGHT: u32 = 3u;
const VIEW_OCCLUDERS: u32 = 4u;
const VIEW_LIGHT: u32 = 5u;
const VIEW_SHADOW: u32 = 6u;
const VIEW_REACH: u32 = 7u;
const VIEW_SUN: u32 = 8u;
const VIEW_SKY: u32 = 9u;
const VIEW_FLAMES: u32 = 10u;

@group(0) @binding(2) var<uniform> lighting: Lighting;

// The fourth channel of the place attachment: the kind in the low two bits,
// then seven bits of tile-local `x` and seven of tile-local `y`. The world
// passes pack it; `crate::place` documents it. `KIND_MASK`/`SUB_TILE_MASK`/
// `SUB_TILE`/`KIND_NOTHING`/`KIND_LAND`/`KIND_STATIC`/`KIND_MOBILE` are the
// import above, `place_format.wesl`.

// The statics and mobiles passes' own `SpriteQuad`, laid out exactly as
// `sprite.rs`'s `SpriteQuad::write` puts it on the GPU — this is that same
// buffer, bound a second time as storage rather than uploaded twice. Only
// `place` is read here today; the rest exists so the struct's size matches
// `SpriteQuad::STRIDE` and a future reader (decision 3's real per-face
// normal, step 4) has the rest already in reach. `docs/gbuffer.md` step 3.
// `twin` (step 4) is what `statics.wgsl` reads to give a corner's two faces
// two different ids — it is never read here, `face_instances[id]` is already
// addressed by whichever of the two ids a fragment's `place` carries, but it
// still has to be declared: this struct's size is what WGSL uses to stride
// through the buffer, and a struct narrower than `SpriteQuad::STRIDE` reads
// every row past the first at the wrong offset.
// `owner` (`docs/lighting_height.md` phase 3) *is* read here, unlike `twin`:
// which occluder of its own tile the thing drawn on a pixel is, so that
// `cell_stopped` can ask whether a solid it meets is the one the fragment is a
// point of instead of guessing from its height. `crate::sprite::SpriteQuad`.
struct FaceInstance {
    rect: vec4<f32>,
    region: vec4<f32>,
    depth: f32,
    hue: u32,
    place: vec2<u32>,
    twin: u32,
    owner: u32,
};

@group(0) @binding(9) var<storage, read> face_instances: array<FaceInstance>;
@group(0) @binding(10) var<storage, read> mobile_instances: array<FaceInstance>;

// `docs/gbuffer.md` step 4c's mesh-face pass's own row: the tile a face's
// static stands on, and which way that face really faces — a mesh face has
// no picture, so it shares no field with `FaceInstance` and lives in its own
// buffer instead. `crate::mesh_face::MeshFaceRow`, laid out identically.
struct MeshFaceInstance {
    tile: u32,
    stance: u32,
    owner: u32,
};

@group(0) @binding(11) var<storage, read> mesh_instances: array<MeshFaceInstance>;

// The ground pass's own `GroundQuad`, laid out exactly as `ground.rs`'s
// `GroundQuad::write` puts it on the GPU — this is that same buffer, bound a
// second time as storage. `docs/gbuffer.md` step 7, the ground half of what
// step 3 did for a static's tile.
//
// Scalars rather than the `vec4`s `GroundQuad`'s own doc groups them into,
// unlike `FaceInstance` above: `GroundQuad::write`'s first field is a bare
// `(x, y)`, only eight bytes wide, and a `vec4` declared right after it would
// force WGSL to align that field to sixteen bytes — opening a gap the real
// bytes do not have, and reading every field after it four bytes short of
// where the Rust side actually put it. A scalar's alignment is never wider
// than four bytes, so a struct built only from them can never drift from an
// offset `GroundQuad::write` chose. Only `place0` is read; the rest is
// declared so the struct's size matches `GroundQuad::STRIDE`, the same reason
// `FaceInstance`'s unread fields are — a struct narrower than the real stride
// reads every row past the first at the wrong offset. `place1` — the second
// word `crate::place::Place::packed` writes, `z`/`kind`/`stance` — is never
// read: the ground pass's `z` and fraction are this fragment's own, computed
// by `ground.wgsl` and carried in the attachment already, not a fact the row
// holds.
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
    // `Place::packed`'s two words: `place0` is `x | y << 16`, and `place1` —
    // never read here — is `(z + 128) | kind << 8 | stance << 16`.
    place0: u32,
    place1: u32,
};

@group(0) @binding(12) var<storage, read> ground_instances: array<GroundInstance>;
// One solid's own horizontal footprint, indexed and folded exactly as
// `solids`/`apertures` are: (min.x, max.x, min.y, max.y), a fraction of the
// tile it stands on quantised to a byte — `occlusion::Solid::fraction`.
// `docs/lighting_raymarch.md`'s "second bigger idea": the reader `box_of`
// used to have no channel to read, arrived.
@group(0) @binding(13) var footprints: texture_2d<u32>;
// And a solid's own `z` span, indexed and folded the same way: sixteen bits an
// end, low byte first, in steps of `1/SOLID_Z_STEPS` from `SOLID_Z_FLOOR` —
// `occlusion::Solid::z_bytes`, and **the only place a solid's height is on the
// wire**. `docs/lighting_height.md` phase 2: the horizontal half of a solid's
// box had been exact since `footprints` landed, and the vertical half was a
// whole unit, so a box based at `z 3.5` arrived here spanning `4` and the bottom
// half unit of its own faces sat below its own solid.
@group(0) @binding(14) var solid_z: texture_2d<u32>;

// No occluder at all — `occlusion::OwnerId::NONE`, and what the ground, a mobile
// and anything the grid refused stamp. Zero and matching nothing, which is why
// the numbering `Builder::finish` hands out starts at one: two fragments that are
// each a point of nothing are not a point of the same thing.
const OWNER_NONE: u32 = 0u;

// Steps of a `z` unit that plane carries one in, and where it counts from.
//
// `occlusion::Solid::Z_STEPS` and `occlusion::Z_FLOOR`, and nothing but a person
// can compare the two pairs. A power of two, so a whole `z` — what every static
// off `tiledata` stands at — lands on a step exactly.
const SOLID_Z_STEPS: f32 = 256.0;
const SOLID_Z_FLOOR: f32 = -128.0;

// The third channel is `z`'s whole units offset by 128 in its low eight bits,
// sixteenths of a unit in the four above them, and the sprite's stance in the
// four above those — `crate::place::STANCE_SHIFT`, and `statics.wgsl` writes
// it. The four faces are the only stance values this pass reads: a flat or
// faceless surface has no direction to be lit from, and a **corner** never
// arrives here at all — `statics.wgsl` has already resolved it to the face of the
// half the fragment was drawn on, because a corner's two surfaces are two halves
// of one picture and one pixel is on one of them. `PLACE_STANCE_SHIFT`/
// `PLACE_STANCE_MASK`/`unpack_place_z`/`STANCE_MESH_FACE`/`STANCE_FLAT`/
// `STANCE_FACE_NORTH`/`STANCE_FACE_EAST`/`STANCE_FACE_SOUTH`/`STANCE_FACE_WEST`
// are the import above, `place_format.wesl`. `STANCE_MESH_FACE` is not a real
// stance, a routing sentinel the mesh-face pass writes instead of one of the
// four faces — read before `outward` ever sees it: the branch above replaces
// it with the row's own real stance first.

// Which way a face looks, in tiles. Zero for everything that has no face.
//
// The direction is the *drawn* one, which is the same thing: the art only ever
// draws the two faces an isometric camera can see — a wall stands on its tile's
// `y1` or `x1` edge and its picture is the surface turned towards the viewer —
// so a south face looks towards `+y` and an east face towards `+x`. North and
// west are five graphics out of 1197 and are here because the geometry has four
// edges. `docs/lighting.md`, step 15.
fn outward(stance: u32) -> vec3<f32> {
    switch stance {
        case STANCE_FACE_NORTH: { return vec3<f32>(0.0, -1.0, 0.0); }
        case STANCE_FACE_EAST: { return vec3<f32>(1.0, 0.0, 0.0); }
        case STANCE_FACE_SOUTH: { return vec3<f32>(0.0, 1.0, 0.0); }
        case STANCE_FACE_WEST: { return vec3<f32>(-1.0, 0.0, 0.0); }
        // A **flat** surface looks up, and this is where that was missing. A
        // wall's top cap is a `Flat` static, so nothing tested which way it looked
        // and a lamp beside a wall lit its top as fully as one standing over it —
        // reported from the client as two walls "adding up" at a corner, which is
        // the bright diamond a corner's cap makes between two lit faces. Decision
        // 27.
        case STANCE_FLAT: { return vec3<f32>(0.0, 0.0, 1.0); }
        // And `STANCE_UPRIGHT` looks nowhere: a billboard has no side, so every
        // flame that reaches it lights it. That is a statement about what is not
        // known, and `light::Surface::normal` returns nothing for the same reason.
        default: { return vec3<f32>(0.0); }
    }
}

// How wide the band is, in tiles, over which a flame passing behind a face stops
// lighting it.
//
// Not a step, for the reason a beam's rim is not a step: a hard edge is what the
// eye finds first, and a lamp walking past the end of a wall would switch its
// face off between two frames. A fifth of a tile is narrow enough that a wall is
// still plainly one-sided and wide enough that nothing pops.
const FACE_EDGE: f32 = 0.2;

// How much of a flame at `toward` reaches a surface facing `normal`: 1 in front
// of it, 0 behind it, and a gradient `FACE_EDGE` wide across the plane.
//
// **The one thing a fraction cannot say.** A wall's two faces are one tile and
// one plane — the same tile, the same fraction, the same height — so without a
// facing a torch inside a room lights the outside of the house exactly as
// brightly as the inside. That was reported from the client as a wall behaving
// like glass, and it is what this fixes.
//
// `light::faces`, and the two are one formula.
fn faces(normal: vec3<f32>, toward: vec3<f32>) -> f32 {
    return clamp(dot(normal, toward) / FACE_EDGE + 0.5, 0.0, 1.0);
}

// How many `z` units make one tile — `light::Z_PER_TILE`, and the same
// derivation: 44 virtual pixels of tile over 4 pixels a unit of height.
const Z_PER_TILE: f32 = 11.0;
// How tall a flame is: `light::FLAME_DEPTH`, a quarter of a tile — the
// projection draws four screen pixels to one `z` and a torch's drawn flame is
// eight or ten of them. **It is what turns a softness in tiles into one in `z`,
// everywhere** — a penumbra is the size of the source across the edge it spills
// over, and every edge softened vertically here (a wall's top, a hole's sill, a
// lid's plane) is horizontal, so what blurs it is the flame's height and not its
// width. `Z_PER_TILE` did that conversion until a house's corner was measured.
const FLAME_DEPTH: f32 = Z_PER_TILE / 4.0;

// How many cells of the grid one ray may look at.
//
// One number for both rays, because they are one walk. A pool reaches nine tiles
// at the widest (`light::CAMPFIRE`) and a sunbeam's segment runs `MAX_SUN_TILES`,
// and `walk` visits every cell the segment crosses rather than a fixed number of
// samples — which on a diagonal is both axes' worth, so this is twice the longer
// of the two and a little. It is not meant to be reached: a fragment outside a
// pool never walks at all, and a sunbeam ends where it leaves the grid's ceiling.
// It exists so that a loop over data cannot be made unbounded by a radius
// somebody widens later. `light::MAX_WALK_STEPS`, and the two are one number.
const MAX_WALK_STEPS: i32 = 72;

// How far a ray must travel inside an occluding cell for that cell to stop all
// it can, in tiles.
//
// The walk knows how long the ray's crossing of each cell is, and using it is
// what makes a shadow's edge a gradient instead of a step: a ray that clips the
// corner of a wall tile passes almost all of its light, and one that crosses the
// tile squarely passes none. Without it a cell is all or nothing and the edge of
// every shadow lands exactly on a tile boundary — which is the blockiness the
// pools were accused of, arriving from the second of its three directions.
//
// It is not one length, though, because a shadow's edge is not equally soft
// everywhere: a flame is a body rather than a point, so an occluder close to the
// thing it shadows draws a sharp edge and a distant one draws a wide penumbra.
// The width of that penumbra is the flame's own size times `t / (1 - t)`, where
// `t` is how far along the ray the occluder is from the *lit* end — the ordinary
// similar-triangles answer, and it costs one division rather than a second ray.
// A wall a fragment is standing against is crisp; the doorpost four tiles away
// is soft, which is what a torch in a room actually looks like.
//
// `FLAME_SPREAD` is that size, in tiles, and the bounds keep the ends of the
// ratio finite. Invented here, like `occlusion::PANE` — no client file has a
// number for how big a flame is.
const FLAME_SPREAD: f32 = 1.0;
const SOFT_CROSSING_MIN: f32 = 0.05;
const SOFT_CROSSING_MAX: f32 = 0.7;

// The four sides of a tile, in the low bits of a cell's fourth channel, and the
// `PRESENT` bit above them. `crate::occlusion` writes them; the numbers are
// pinned there and nothing but a person reading both files can check they agree.
//
// A mask of zero is a **lid** — a floor, a roof — whose occlusion is entirely its
// `z` span and which no vertical side describes. All four is "it stands up and
// the art would not say which way", which is the whole-tile occluder decision 3
// started with and the answer everything falls back to.
const EDGE_NORTH: u32 = 1u;
const EDGE_EAST: u32 = 2u;
const EDGE_SOUTH: u32 = 4u;
const EDGE_WEST: u32 = 8u;
const EDGE_MASK: u32 = 15u;
// And the bit that says this surface has a hole in it, so that its texel in the
// aperture plane means something. `occlusion::HOLED`.
const HOLED: u32 = 64u;
// How many parts of a tile one step of a hole's run coordinate is worth.
// `occlusion::RUN_STEPS`, and the two are one number: both sides read the same
// byte and divide it by this, so the two implementations agree exactly rather
// than to a tolerance.
const RUN_STEPS: f32 = 255.0;

// How far past an exact tangent `ray_vs_solid`'s own `entered > leaves`
// rejection still counts as a hit — a cross-implementation tolerance, and
// it lives only on this side. `light::ray_vs_solid`'s own doc comment has
// the story: `docs/lighting_raymarch.md`'s point 4 cutover found a ray
// grazing a box's corner where this shader's own `f32` division rounds to
// `entered > leaves` (a miss) on inputs `light::sample` — with the exact,
// untolerant formula — rounds to `entered <= leaves` (a hit) for. Widening
// the CPU copy to match broke a different equivalence instead of fixing
// this one; widening only here, to agree with what the CPU side already
// computes unprompted, is what `light::ray_vs_solid`'s own doc comment
// argues is the narrower, correct fix.
//
// **It is a rounding-scale number and it used to be `1.0e-2`**, which is not
// a rounding scale at all: `entered` and `leaves` are fractions of the whole
// segment, so a hundredth of one is a hundredth of the way from a fragment to
// its flame — around a screen pixel of world at this crate's own top zoom, and
// every box in the frame silently a pixel fatter wherever a ray grazed it. It
// cost 299 of the 302 disagreements `examples/boxes.rs`'s ground oracle
// reported on the `tree` scene and 27 of its face oracle's 43, all of them the
// shader alone against a `light::sample` that agreed with the independent
// geometry. And it never bought what it was introduced for:
// `the_shader_and_light_sample_agree_about_a_carried_beam` and
// `the_shader_lights_a_frame_as_light_sample_does` fail on the same pixel with
// the same two numbers at `1.0e-2`, at `1.0e-6` and at `0.0` — measured, not
// argued. What is left here is the guard the comment above describes and
// nothing more: two `f32` implementations of one formula, one rounding apart.
const RAY_TANGENT_TOLERANCE: f32 = 1.0e-6;

// How much of a panel a ray pierces at height `z` runs into: 1 well inside the
// span it occupies, 0 well outside, and a gradient `tall` `z` units wide across
// its edges.
//
// The gradient is the vertical half of decision 14's penumbra, and it is all that
// is left of it: a flame is a body rather than a point, so a ray grazing the top
// of a wall is dimmed rather than switched.
//
// **The band is centred on the top edge and hangs below the bottom one**, which is
// the one asymmetry here and it is not cosmetic. A wall is based at the height of
// the ground it stands on, and the ray a person actually looks at — a torch and a
// floor, both at `z = 0` — runs exactly along that bottom edge. Centred there too,
// it would meet half the wall and every wall in the frame would pass half its
// light along the ground. Measured: a shut room's shadow read `0.378` against the
// `0.356` of its ambient before this line said so.
//
// `light::pierces`, and the two are one formula.
fn pierces(z: f32, low: f32, high: f32, tall: f32) -> f32 {
    let band = max(tall, 1.0e-3);
    let inside = min(z - low + band * 0.5, high - z);
    return clamp(inside / band + 0.5, 0.0, 1.0);
}

// How much of a **lid** is in the way of a ray that runs from `from` to `to` in
// `z` across one cell: 1 where the ray went through the plane and out the other
// side, 0 where it stayed on one side of it, and a gradient between where the
// flame itself straddles the plane.
//
// **A lid is a plane and not a slab**, which is why this is neither `pierces` nor
// the length rule beside it. A floor is `height 0` in `tiledata.mul` — 4,534 of
// the 4,647 lids over the block of Britain `artscan`'s `column` example reads —
// so its span is zero deep, and scaling what it stops by how far the ray ran
// *inside* that span gets zero out of every floor in the world. That is what lit
// the storey above a torch through its own floorboards; see
// `scene::storey_over_a_torch`.
//
// The crossing test is **strict**: a ray running exactly along the top of a lid —
// a candle standing on the floor it lights — has not gone through anything, and
// counting a touch would lay half a floor's shadow across every room lit from
// inside it. It is `pierces`'s asymmetry, arriving at the surface that has no
// thickness for a band to hang under.
//
// The softness is the flame's own size and is measured at the flame: the plane
// cuts the source, so what gets through is the share of it left on the lit side.
// A sunbeam passes a `spread` of 0 and gets the hard edge a point source casts.
//
// **How tall that flame is, though, is `FLAME_DEPTH` and not its width** — half a
// tile, `light::FLAME_LIFT`'s own number. The two were one for a day and it lit
// the storey over every wall sconce in Britain: a sconce burns four or five `z`
// under the floor above it, and a flame eleven `z` tall pokes a tenth of itself
// through the boards.
//
// `light::crosses`, and the two are one formula.
fn crosses(entering: f32, leaving: f32, low: f32, high: f32, source: f32, spread: f32) -> f32 {
    let under = min(entering, leaving);
    let over = max(entering, leaving);
    if under >= high || over <= low {
        return 0.0;
    }
    // How far past the lid the flame itself stands, on the side the ray left by.
    let beyond = select(low - source, source - high, leaving >= entering);
    return clamp(beyond / max(spread * FLAME_DEPTH, 1.0e-3) + 0.5, 0.0, 1.0);
}

// How far in front of its own plane a **face** pixel is walked from, in tiles,
// and how far above whatever it lies on every point of the world is, in `z`.
// `light::STAND_OFF` and `light::ON_TOP`, and the numbers are one pair: two steps
// of the seven-bit fraction the attachment carries, and well under the one `z`
// unit it quantises height to. Only the walk moves — the attachment still names
// the wall's own tile, so picking and the debug views are untouched.
const STAND_OFF: f32 = 2.0 / 127.0;
const ON_TOP: f32 = 1.0 / 128.0;

// Whether a lit point lies **on** a surface: its `z` is inside the span that
// surface occupies, its two edges included — and inclusive by `ON_TOP`, which is
// the nudge `walk` gave the point and has to be given back here, or a pixel of a
// wall's top cap stops being a point of its own wall and the wall shadows it.
// What the exemptions are asked, one surface at a time — see `walk`, and
// `light::on_surface`.
//
// `low`/`high` are `span_of`'s pair and not the texel's own two channels: this
// is a question about whether a fragment is a point *of* the solid, and half a
// unit of rounding is the difference between a face's bottom row belonging to
// the box it is drawn on and standing under it.
fn on_surface(z: f32, low: f32, high: f32) -> bool {
    return z >= low - ON_TOP && z <= high + ON_TOP;
}

// Whether a **lid** is a plane the fragment is drawn *on*, rather than one
// standing between it and the flame. `docs/lighting_height.md` phase 4's rule,
// and `light::drawn_on` — see that function for the whole of the argument. Two
// facts, no tolerance: the lid is a plane (`low == high`), so a ray leaving it
// crosses it at its own origin and nowhere else and there is no later crossing an
// exemption could swallow; and the fragment is drawn at exactly that height,
// asked of `drawn`, which is `walk`'s `raw_start.z` and **not** its nudged
// `start.z`. The nudge is the walk's own; a question about which surface a
// fragment *is* has to be asked where the fragment is, and a
// hundred-and-twenty-eighth of `ON_TOP` answering it as a crossing is exactly
// what painted a stair's tread tops black.
fn drawn_on(drawn: f32, low: f32, high: f32) -> bool {
    return low == high && drawn == low;
}

// A soft interval: 1 well inside `low..=high`, 0 well outside, and a gradient
// `band` wide across each edge.
//
// `pierces` with its one asymmetry taken out, and that is why it is a second
// function rather than a call of the first. A wall's *bottom* edge is the ground
// it stands on and the ray a person looks at runs along it, so that band hangs
// below rather than straddling. A **hole's** edges are in the middle of a
// surface and no ray runs along them by construction, so a hole softens the same
// amount both ways or it is a hole that has been moved half a penumbra down.
//
// `light::inside`.
fn inside(x: f32, low: f32, high: f32, band: f32) -> f32 {
    let width = max(band, 1.0e-3);
    return clamp(min(x - low, high - x) / width + 0.5, 0.0, 1.0);
}

// Where along a panel's own run a point of it lies, 0 to 1 across the tile.
//
// A panel on a north or south side lies in a plane of constant `y`, so what runs
// along it is `x`; an east or west one is the other way round. That is the whole
// of a surface's own coordinate system, and it is why only a *named* panel may
// have a hole: a lid and a body have no run to measure one along. `light::run_v`.
fn run_v(edges: u32, px: f32, py: f32) -> f32 {
    let along = select(py, px, (edges & (EDGE_NORTH | EDGE_SOUTH)) != 0u);
    return along - floor(along);
}

// How much of a surface is **missing** where a ray goes through it: 1 well inside
// the hole, 0 well outside, with the same penumbra its top edge gets.
//
// The two spans meet with `min` and not with a product, so the corner of a hole
// is softened once: a point halfway into the hole across *and* halfway up is on
// one corner's diagonal, and two halves multiplied would make it a quarter of a
// hole. `light::hole`.
fn hole(texel: vec4<u32>, v: f32, z: f32, wide: f32, tall: f32) -> f32 {
    let across = inside(v, f32(texel.x) / RUN_STEPS, f32(texel.y) / RUN_STEPS, wide);
    let up = inside(z, f32(texel.z) - 128.0, f32(texel.w) - 128.0, tall);
    return min(across, up);
}

// How much of a surface stands in the way at the point a ray goes through its
// plane: the span it occupies, less the hole in it.
//
// The whole of `docs/lighting.md`'s step 21.3, and the reason it is this small is
// decision 30.7 — a panel was already *pierced at a point* rather than travelled
// through, so the point was already being computed and a window is what that
// point is asked about. `id` is the solid's own place in the list, which is
// where its hole is; `cross` is where the ray goes through the plane, in all
// three, because the height alone cannot say whether the ray went through the
// window or through the wall beside it. `light::pierced`.
fn pierced(stands: vec4<u32>, id: u32, low: f32, high: f32, cross: vec3<f32>, wide: f32, tall: f32) -> f32 {
    let across = pierces(cross.z, low, high, tall);
    if (stands.w & HOLED) == 0u {
        return across;
    }
    let v = run_v(stands.w & EDGE_MASK, cross.x, cross.y);
    return across * (1.0 - hole(aperture_at(id), v, cross.z, wide, tall));
}

// Which of a cell's sides are **the same wall the lit end is part of**, and
// therefore must not shadow it.
//
// A wall's face lies *on* the panel it is the face of — that is what decision 16
// is about — so a pixel of a wall is a point in the plane of its own tile's
// panel, and the panels of the tiles either side of it are in that same plane.
// Every ray leaving such a pixel along the wall therefore *grazes* the
// neighbours' panels, and whether it counts as crossing one is decided by the
// last bits of a float. It showed as a thin dark stroke down the wall at every
// tile seam, one per corner, on any wall lit by a lamp standing near it —
// the same corner ambiguity that used to draw bright spokes, arriving with the
// sign reversed.
//
// A run of wall is one surface, and no part of a surface shadows another part of
// it. So a panel on the same side of its tile as the lit end's own, on the same
// *line* — the same row for a north or south face, the same column for an east
// or west one — is not an occluder for this ray. Anything else about that cell
// still is: a wall tile that also carries the perpendicular face of a corner
// stops the ray on that face as it always did.
//
// Which side of its own tile a stance is the face of, as an edge bit. Nothing
// for a flat or faceless pixel: neither is part of a run of wall.
//
// `light::Surface::face`, put through `occlusion::edges_of`.
fn own_side(stance: u32) -> u32 {
    switch stance {
        case STANCE_FACE_NORTH: { return EDGE_NORTH; }
        case STANCE_FACE_EAST: { return EDGE_EAST; }
        case STANCE_FACE_SOUTH: { return EDGE_SOUTH; }
        case STANCE_FACE_WEST: { return EDGE_WEST; }
        default: { return 0u; }
    }
}

// `light::own_run`.
fn own_run(own: u32, cell: vec2<i32>, first: vec2<i32>) -> u32 {
    var line = 0u;
    if cell.y == first.y {
        line = line | EDGE_NORTH | EDGE_SOUTH;
    }
    if cell.x == first.x {
        line = line | EDGE_EAST | EDGE_WEST;
    }
    return own & line;
}

// How wide the lists are as textures, in texels. `occlusion::LIST_ROW`, and the
// two are one number — a list is one dimensional and a texture is not.
const LIST_ROW: u32 = 1024u;

// Where one tile's references are: `(offset, count)`, and `(0, 0)` for open
// ground and for anything outside the grid.
//
// The three-channel offset is `crate::occlusion::Occlusion::bytes`: one byte
// holds 255 references and a city block holds thousands, so it is spread over
// the three channels the count does not need.
fn solids_at(x: i32, y: i32) -> vec2<u32> {
    let cell = vec2<i32>(x - lighting.grid.x, y - lighting.grid.y);
    if cell.x < 0 || cell.y < 0 || cell.x >= lighting.grid.z || cell.y >= lighting.grid.w {
        return vec2<u32>(0u, 0u);
    }
    let span = textureLoad(occluders, cell, 0);
    return vec2<u32>(span.x | (span.y << 8u) | (span.z << 16u), span.w);
}

// Which solid a cell's `n`th reference names, and **which occluder of that cell
// it is** — `occlusion::SolidId` spread over three channels exactly as the
// index's offset is, and `occlusion::OwnerId` in the fourth.
//
// The one fetch step 23.1 added to the hot loop, and it is a fetch rather than
// arithmetic because a reference is a fact about the world's geometry: the run a
// cell holds is contiguous, what it points at need not be. The owner rides in the
// same texel for the same reason — it is a fact about the *reference*, and the
// loop is already reading this word.
fn id_at(reference: u32) -> vec2<u32> {
    let at = vec2<i32>(i32(reference % LIST_ROW), i32(reference / LIST_ROW));
    let id = textureLoad(solid_ids, at, 0);
    return vec2<u32>(id.x | (id.y << 8u) | (id.z << 16u), id.w);
}

// One solid of the list: (0, 0, opacity, PRESENT | HOLED | edges).
//
// The first two channels held `(z_bottom + 128, z_top + 128)` until
// `docs/lighting_height.md` phase 2 gave the span sixteen bits an end in
// `solid_z`; they are zero now rather than a rounded copy of it. See `span_of`.
fn solid_at(id: u32) -> vec4<u32> {
    let at = vec2<i32>(i32(id % LIST_ROW), i32(id / LIST_ROW));
    return textureLoad(solids, at, 0);
}

// And its hole: (near, far, z_bottom + 128, z_top + 128). The same id and the
// same folding, which is what makes the two planes one list — read only where
// the solid's `HOLED` bit says there is one.
fn aperture_at(id: u32) -> vec4<u32> {
    let at = vec2<i32>(i32(id % LIST_ROW), i32(id / LIST_ROW));
    return textureLoad(apertures, at, 0);
}

// And its footprint: (min.x, max.x, min.y, max.y), read every time rather
// than gated the way `aperture_at` is — every solid has one, where almost
// none has a hole.
fn footprint_at(id: u32) -> vec4<u32> {
    let at = vec2<i32>(i32(id % LIST_ROW), i32(id / LIST_ROW));
    return textureLoad(footprints, at, 0);
}

// The `z` span one solid occupies, off `solid_z`'s own four bytes.
//
// `light::wire_span`, and `occlusion::Solid::span_from_bytes` is the arithmetic.
// **The one place this pass turns the wire into a height**, and the reason
// `solid_at`'s first two channels are zero rather than a rounded copy: a second
// height a reader could reach for would still compile, still look like a height,
// and be wrong by up to half a unit. `docs/lighting_height.md` phase 2.
fn span_of(id: u32) -> vec2<f32> {
    let raw = textureLoad(solid_z, vec2<i32>(i32(id % LIST_ROW), i32(id / LIST_ROW)), 0);
    return vec2<f32>(
        f32(raw.x | (raw.y << 8u)) / SOLID_Z_STEPS + SOLID_Z_FLOOR,
        f32(raw.z | (raw.w << 8u)) / SOLID_Z_STEPS + SOLID_Z_FLOOR,
    );
}

// One tile's surfaces as one box: the union of the spans, the largest opacity
// and the union of the sides, with `edges` of zero for a tile that stands
// nothing.
//
// `Occlusion::at`'s merged view, and for the same readers — a picture of the
// grid rather than a step of the walk. The walk stopped asking it when the list
// arrived, so nothing in the hot loop pays for this.
//
// The span comes out of `span_of` like every other height in this pass, and not
// as the two raw channels this used to hand back. Its one reader is a
// *diagnostic* picture, which is exactly where phase 1 was bitten: `plan.rs`'s
// elevation view kept its own copy of a format, rounded the height, and drew the
// staircase the plan was about into the instrument meant to show it.
struct MergedCell {
    low: f32,
    high: f32,
    opacity: u32,
    edges: u32,
};

fn merged_at(x: i32, y: i32) -> MergedCell {
    let run = solids_at(x, y);
    if run.y == 0u {
        return MergedCell(0.0, 0.0, 0u, 0u);
    }
    let first_id = id_at(run.x).x;
    let first = solid_at(first_id);
    let first_span = span_of(first_id);
    var merged = MergedCell(first_span.x, first_span.y, first.z, first.w);
    for (var i = 1u; i < run.y; i = i + 1u) {
        let id = id_at(run.x + i).x;
        let stands = solid_at(id);
        let span = span_of(id);
        merged = MergedCell(
            min(merged.low, span.x),
            max(merged.high, span.y),
            max(merged.opacity, stands.z),
            merged.edges | stands.w,
        );
    }
    return merged;
}

// How much of the sky one tile can see, `0..=1`.
//
// Open sky outside the rectangle, which is `Occlusion::sky_at`'s own answer and
// the honest one in the direction that matters: the grid is grown by the widest
// pool's reach, so a tile outside it is one the frame does not draw, and
// answering "dark" there would put a band of night around every frame.
fn sky_at(x: i32, y: i32) -> f32 {
    let cell = vec2<i32>(x - lighting.grid.x, y - lighting.grid.y);
    if cell.x < 0 || cell.y < 0 || cell.x >= lighting.grid.z || cell.y >= lighting.grid.w {
        return 1.0;
    }
    return f32(textureLoad(field, cell, 0).x) / 255.0;
}

// How much of a ray survives a segment of the world: 1 for nothing in the way,
// 0 for a wall.
//
// **One walk for both the flame and the sun.** They differ in their *ends* and in
// nothing else, so the ends are the parameters: `skip_last` is the flame's own
// tile, which must not shadow it (a sconce stands *on* a wall), and `spread` is
// how big the source is — see `FLAME_SPREAD`. A sunbeam passes `0` for both: its
// far end is a point in the sky rather than a tile, and the sun subtends half a
// degree, so its penumbra is the narrowest this walk draws.
//
// They were two functions until the sun's was measured. The sun's sampled one
// point per tile — the arrangement this walk replaced for flames — and a sealed
// roofed room therefore had a full-strength column of sunlight down the inside of
// its sunward wall: at noon a ray climbs 11 `z` a tile, so it crossed the wall's
// plane at 16 and was looked at, one tile later, at 22. It stepped over the top of
// a wall it had gone through. Two implementations of one idea, one of them a
// generation behind, is what that failure is made of; hence one.
//
// A grid traversal of the cells the segment crosses — every one, in order, with
// the length of each crossing. Not a fixed number of samples along the ray: at
// two tiles apart that was one interior point, so whether a fragment was in
// shadow was decided at the resolution of a tile and every shadow's edge was a
// staircase.
//
// The starting cell is always left out: the tile being lit must not shadow
// itself, which is what keeps a wall's own face the brightest thing near a torch.
//
// **A cell in between stops the ray only where the ray crosses the side the
// thing actually stands on.** That is decision 3 revised: an occluder used to be
// a whole tile, because nothing said which edge a wall was built on, and a lamp
// mounted on a house was therefore shadowed by the next tile of its own wall —
// the light could not run *along* the street it hung over. The walk already
// knows which boundary it crossed to enter a cell and which it will cross to
// leave, so the test costs two bit operations and no extra sampling.
//
// The height is interpolated along the ray, so a cell only stops the light where
// the ray actually passes through the span it occupies — and it is the *share*
// of the crossing that is inside that span which counts, so a ray grazing the
// top of a wall is dimmed rather than switched. That is what keeps a cellar's
// wall out of the street above it, without a step where the two meet.

// The box one occluder standing on `cell` is — `light::occlusion::Solid::
// box_from_footprint`, its own WGSL mirror. Used to be reconstructed from
// only `(cell, bottom, top, edges)`, guessing whole-tile-or-panel-strip and
// blind to a body narrower than its tile; `footprint` is `footprint_at`'s own
// four bytes, `occlusion::Solid::fraction`'s quantised `(min.x, max.x, min.y,
// max.y)`, so this is now a direct read rather than a guess — bit-for-bit
// what `Solid::fraction` measured off the real `space` at upload time, lossy
// only by that one shared quantisation step. `docs/lighting_raymarch.md`'s
// "second bigger idea".
struct SolidBox {
    lo: vec3<f32>,
    hi: vec3<f32>,
};

fn box_of(cell: vec2<i32>, bottom: f32, top: f32, footprint: vec4<u32>) -> SolidBox {
    let lo = vec3<f32>(
        f32(cell.x) + f32(footprint.x) / 255.0,
        f32(cell.y) + f32(footprint.z) / 255.0,
        bottom,
    );
    let hi = vec3<f32>(
        f32(cell.x) + f32(footprint.y) / 255.0,
        f32(cell.y) + f32(footprint.w) / 255.0,
        top,
    );
    return SolidBox(lo, hi);
}

// Whether a point stands over a solid's own horizontal footprint —
// `light::over_footprint`, and `ray_vs_solid`'s parallel-axis branch on the two
// horizontal axes alone, both ends inclusive as they are there.
//
// Used by the no-horizontal-run shortcut in `walk`, which cannot call
// `ray_vs_solid` outright: that one answers the *height* half hard, and a
// vertical ray's height answer is `crosses`'s soft one — a flame is a body
// `spread` deep, so a lid a little past the end of the ray still takes part of
// it. The Rust twin's doc comment states the same split.
fn over_footprint(at: vec3<f32>, bx: SolidBox) -> bool {
    return at.x >= bx.lo.x && at.x <= bx.hi.x && at.y >= bx.lo.y && at.y <= bx.hi.y;
}

// Where a straight segment enters and leaves an axis-aligned box, as a
// fraction of the whole segment (`0.0` is `from`, `1.0` is `to`) — the slab
// method, continuous throughout, no notion of "which tile" anywhere in it.
// `light::ray_vs_solid`, and the two must answer the same segment identically.
//
// `hit` is `false` when the segment misses the box on at least one axis
// outright, including a segment parallel to a flat axis that does not lie
// exactly in the box's own plane on it. `entered`/`leaves` are already
// clamped to `0.0..=1.0` — the box's own extent past either end of the
// segment does not count. Unrolled over `x`/`y`/`z` rather than a loop
// indexing a `vec3`, matching this file's own idiom everywhere else a walk
// has needed one axis at a time.
struct RayBox {
    hit: bool,
    entered: f32,
    leaves: f32,
};

// A flame is a point source, and a point source's shadow is exact — no
// widened corner test, no taper: an intersection here already means the
// segment genuinely passed through the box. See `docs/lighting_raymarch.md`'s
// "hard shadows" decision; `light::ray_vs_solid` is the whole test now, for
// every edge kind, and this is its mirror.
fn ray_vs_solid(ray_from: vec3<f32>, ray_to: vec3<f32>, lo: vec3<f32>, hi: vec3<f32>) -> RayBox {
    var entered = 0.0;
    var leaves = 1.0;
    let delta = ray_to - ray_from;
    if abs(delta.x) <= 1.0e-9 {
        if ray_from.x < lo.x || ray_from.x > hi.x {
            return RayBox(false, 0.0, 0.0);
        }
    } else {
        let t1 = (lo.x - ray_from.x) / delta.x;
        let t2 = (hi.x - ray_from.x) / delta.x;
        entered = max(entered, min(t1, t2));
        leaves = min(leaves, max(t1, t2));
        // `RAY_TANGENT_TOLERANCE`, not a bare `entered > leaves`: its own
        // comment has the reason — a ray grazing a box's corner can close one
        // axis's interval one rounding of `t` before the other opens, and CPU
        // and GPU are not guaranteed to agree which side of that razor-thin
        // gap is real for the identical segment.
        if entered > leaves + RAY_TANGENT_TOLERANCE {
            return RayBox(false, 0.0, 0.0);
        }
    }
    if abs(delta.y) <= 1.0e-9 {
        if ray_from.y < lo.y || ray_from.y > hi.y {
            return RayBox(false, 0.0, 0.0);
        }
    } else {
        let t1 = (lo.y - ray_from.y) / delta.y;
        let t2 = (hi.y - ray_from.y) / delta.y;
        entered = max(entered, min(t1, t2));
        leaves = min(leaves, max(t1, t2));
        if entered > leaves + RAY_TANGENT_TOLERANCE {
            return RayBox(false, 0.0, 0.0);
        }
    }
    if abs(delta.z) <= 1.0e-9 {
        if ray_from.z < lo.z || ray_from.z > hi.z {
            return RayBox(false, 0.0, 0.0);
        }
    } else {
        let t1 = (lo.z - ray_from.z) / delta.z;
        let t2 = (hi.z - ray_from.z) / delta.z;
        entered = max(entered, min(t1, t2));
        leaves = min(leaves, max(t1, t2));
        if entered > leaves + RAY_TANGENT_TOLERANCE {
            return RayBox(false, 0.0, 0.0);
        }
    }
    return RayBox(true, entered, max(entered, leaves));
}

// How much one candidate cell stops `walk`'s ray — every solid on it tested
// with its own exact `ray_vs_solid` interval, `light::walk_cells_streaming`'s
// own `apply` closure restated as a function, since WGSL has none.
//
// Split out of `walk`'s own loop so it can be called a second time, for the
// cell its own unconditional tie-break probe tests without moving the
// walk's own trajectory there — see `walk`'s own comment at the call sites.
//
// **The walk's own cell and its probe share one tolerance, and that took a
// measurement to be allowed.** This used to take the tolerance as a parameter
// — `RAY_TANGENT_TOLERANCE` here, a hard `0.0` at the probe — because sharing
// the *old* `1.0e-2` broke four previously-green parity tests
// (`_about_a_wall_that_faces_away`, `_about_which_side_a_wall_is_on`,
// `_about_a_hole_in_a_wall`, `_at_the_corner_of_a_house`) once the probe went
// unconditional: a probed cell is a diagonal neighbour by construction, which
// is exactly the geometry a ray is likeliest to graze rather than cross, so a
// tolerance a hundredth of a segment wide rescued spurious near-tangents on
// cells the ray never approached. At a rounding-scale tolerance there is
// nothing spurious left to rescue — the whole parity suite and all three of
// `examples/boxes.rs`'s oracles read the same with it shared as with the probe
// exact — so the parameter is gone and both calls are the one function.
// `light::walk_cells_streaming`'s own probe has always been exact both places.
fn cell_stopped(
    cell: vec2<i32>,
    first: vec2<i32>,
    last: vec2<i32>,
    own: u32,
    owner: u32,
    stance: u32,
    skip_last: bool,
    spread: f32,
    start: vec3<f32>,
    finish: vec3<f32>,
    // The lit end's height **before** `walk`'s own `ON_TOP` moved `start` off it.
    // The two travel together and answer different questions — `light::
    // ExemptionContext`'s `spot_z`/`drawn_z` pair, and `drawn_on` for which is
    // which.
    drawn_z: f32,
    delta: vec3<f32>,
    ground: f32,
) -> f32 {
    let own_cell = cell.x == first.x && cell.y == first.y;
    let span = solids_at(cell.x, cell.y);
    let lit = start;
    // **The tile-wide union of sides used to be gathered here**, in a loop of
    // its own over every solid of the ray's own cell, and `light::Surface::
    // shadowed_by_own_tile` turned it into a mask the exemption below was anded
    // with. `docs/lighting_height.md` phase 3 retired both: that mask was zero
    // for every surface that reached the test (the arm also required the stance
    // *not* to be flat, and the mask was only ever non-zero for a flat one), and
    // what a fragment is a point of is now a number it carries rather than
    // anything derived from its tile. One fetch loop a cell less, besides.

    // What the *cell* stops is the **largest** of what its surfaces stop,
    // and not the product: two panels on one tile are the two faces of one
    // corner, and a ray crossing both has gone through one thing once.
    var stopped = 0.0;
    for (var k = 0u; k < span.y; k = k + 1u) {
        let reference = id_at(span.x + k);
        let id = reference.x;
        let stands = solid_at(id);
        let edges = stands.w & EDGE_MASK;
        // The solid's real `z` span, fraction included — `span_of`, never the
        // texel's two channels on their own. `light::wire_span`.
        let vertical = span_of(id);
        let low = vertical.x;
        let high = vertical.y;
        // The exact box this one solid is, read off its own footprint plane
        // rather than reconstructed from `edges`, and the exact interval
        // this ray crosses it in — `light::ray_vs_solid`, replacing the
        // DDA's own per-*cell* `entered`/`leaves` with a per-*solid* one.
        let bx = box_of(cell, low, high, footprint_at(id));
        let hit = ray_vs_solid(start, finish, bx.lo, bx.hi);
        if !hit.hit {
            continue;
        }
        // Same `lit_end`/`flame_end`/`same_run` decision `light::exemption`
        // makes — see its own doc comment for the whole of why `lit_end` is an
        // identity now and not a height. `OWNER_NONE` matches nothing, so a
        // fragment that is a point of no occluder (the ground, a mobile) is
        // exempt from none of them; `light::occlusion::OwnerId::same`.
        let same_run = select(0u, own_run(own, cell, first), on_surface(lit.z, low, high));
        let lit_end = own_cell && owner != OWNER_NONE && owner == reference.y;
        let flame_end =
            skip_last && cell.x == last.x && cell.y == last.y && on_surface(finish.z, low, high);
        // **Which of the static's surfaces the fragment is a point of, and not
        // merely which static** — `light::exemption`'s own `match` on `edges`,
        // and see it for the whole argument. An owner is per static and a static
        // is several solids; a flight's own risers stand between its own tread
        // tops and a flame, and that is its body rather than a self-shadow. Each
        // shape is asked the one exact question the wire can answer about it: a
        // lid, the height the fragment is drawn at; a body, nothing, since it has
        // no face to be a point of one of; a panel, whether the fragment's own
        // stance names the side this panel stands on.
        var is_mine = (edges & own) != 0u;
        if edges == 0u {
            is_mine = drawn_on(drawn_z, low, high);
        } else if edges == EDGE_MASK {
            is_mine = true;
        }
        if (lit_end && is_mine) || (edges != 0u && flame_end) {
            continue;
        }
        // How soft this crossing's own edge is: the penumbra a source of
        // this size casts from this far along the ray, taken at the exact
        // crossing's own midpoint rather than the cell-shared one the old
        // walk used.
        let middle = (hit.entered + hit.leaves) * 0.5;
        let soft = clamp(
            spread * middle / max(1.0 - middle, 1.0e-3),
            SOFT_CROSSING_MIN,
            SOFT_CROSSING_MAX,
        );
        let opacity = f32(stands.z) / 255.0;
        let tall = soft * FLAME_DEPTH;
        var by_surface = 0.0;
        if edges == 0u {
            // A **lid** — a floor, a rug, a plank. What crosses it is asked
            // over the ray's whole run inside the lid's own real horizontal
            // footprint (an infinite-`z` box, since a lid's own box above is
            // flat in `z` and would only ever catch the instant the ray is
            // exactly at its height), not over `hit`'s own thin interval.
            // `crosses` does the actual plane test on the `z` that gives it.
            // `bx.lo`/`bx.hi`'s own `x`/`y`, not the whole cell — a tread's
            // own top is a lid narrower than its tile, and asking a wider
            // box than the lid actually is would read a ray grazing the
            // tile's corner past the tread's real edge as "crossed through"
            // it. `docs/lighting_raymarch.md`'s "second bigger idea".
            let unbounded = ray_vs_solid(
                start,
                finish,
                vec3<f32>(bx.lo.x, bx.lo.y, -1.0e6),
                vec3<f32>(bx.hi.x, bx.hi.y, 1.0e6),
            );
            var tile_entered = hit.entered;
            var tile_leaves = hit.leaves;
            if unbounded.hit {
                tile_entered = unbounded.entered;
                tile_leaves = unbounded.leaves;
            }
            by_surface = opacity * crosses(
                lit.z + delta.z * tile_entered,
                lit.z + delta.z * tile_leaves,
                low,
                high,
                finish.z,
                spread,
            );
        } else if edges == EDGE_MASK {
            // A **body** — a whole tile that stands up and whose art would
            // not say which way. `ray_vs_solid` is an exact slab test, so a
            // hit already means the segment genuinely passed through the
            // box over `hit.entered..hit.leaves`: occlusion is the body's
            // own opacity outright. `light::walk_cells_streaming`'s own
            // mirror; see `docs/lighting_raymarch.md`'s "hard shadows"
            // decision for why the length-and-pierce grading this used to
            // do (and the widened-corner graze feeding it) are both gone.
            by_surface = opacity;
        } else {
            // A **panel** — a wall standing on one side of its tile. A
            // *surface*, and what a surface does to a ray is decided where
            // the ray pierces it: one sample, at the exact crossing's own
            // midpoint, less whatever of this cell is the same run of wall
            // the lit end stands in.
            let stops = edges & ~same_run;
            if stops != 0u {
                by_surface = opacity * pierced(stands, id, low, high, start + delta * middle, soft, tall);
            }
        }
        stopped = max(stopped, by_surface);
    }
    return stopped;
}

fn walk(
    raw_start: vec3<f32>,
    raw_finish: vec3<f32>,
    tile: vec2<f32>,
    stance: u32,
    owner: u32,
    skip_last: bool,
    spread: f32,
) -> f32 {
    // **Both ends stand where they are drawn, not inside what they are drawn on.**
    // See `STAND_OFF` and `ON_TOP`, and `light::stand_clear`: a face pixel is
    // walked from a hair in front of the plane it is the face of, and every point
    // of the world from a hair above whatever it lies on. Without the pair a wall
    // pixel at exactly a floor's `z` was lit by the room below it — the bright
    // stroke a house wore along its floorboards — and neither closes it alone.
    //
    // The flame's end gets the height and not the offset: a mounted flame is
    // already outside the plane its tile names (`light::mounted_at`), and a flame
    // has no face of its own to be in front of.
    let ahead = outward(stance);
    let start = vec3<f32>(
        raw_start.x + ahead.x * STAND_OFF,
        raw_start.y + ahead.y * STAND_OFF,
        raw_start.z + ON_TOP,
    );
    let finish = vec3<f32>(raw_finish.x, raw_finish.y, raw_finish.z + ON_TOP);
    let delta = finish - start;
    let ground = length(delta.xy);
    let lit = start;
    // The caller's own tile, not `floor(start.x/y)`: `start` can legitimately sit
    // exactly on this tile's own far edge — `raw_start` is a mesh face's
    // reconstructed `(tile, sub)` pair with `sub` clamped short of `1.0`
    // (`mesh_face.wgsl`'s `INSIDE`), and a `Flat` surface's zero `outward` adds no
    // nudge to push it off that edge either — and flooring it back would pick
    // whichever side happens to round down rather than the side the geometry
    // actually stands on. `light::walk_cells`'s `first`, the same fix, and
    // `docs/lighting_raymarch.md` step 5: this is where it names the same class of
    // bug the CPU walk had, on the GPU walk decision 9 requires to match it.
    let first = vec2<i32>(i32(tile.x), i32(tile.y));
    if ground < 1.0e-6 {
        // Straight up or down. There is no direction to walk in and there never
        // was — but "the only cells on the line are the exempt ones", which is
        // what this used to say, stopped being true at decision 32: a **lid** on
        // that one cell is between the two ends of the ray, and a vertical ray is
        // the case a floor is most obviously in the way of. A torch on the ground
        // floor and the plank directly over it were the shortcut's own
        // counterexample. Only lids, for the reason both ends exempt everything
        // else: a panel stands beside a vertical ray rather than across it, and a
        // body is a solid one of the ends is standing in. `light::walk_cells`.
        let straight = solids_at(first.x, first.y);
        var stopped = 0.0;
        for (var i = 0u; i < straight.y; i = i + 1u) {
            let reference = id_at(straight.x + i);
            let id = reference.x;
            let stands = solid_at(id);
            if (stands.w & EDGE_MASK) != 0u {
                continue;
            }
            let vertical = span_of(id);
            // And only the lids this ray is actually **under**. A tread's top is
            // a lid narrower than its tile; the walk below this shortcut has
            // asked it since sub-tile footprints landed and this shortcut did
            // not follow, so a flight's three treads shadowed one another from
            // straight above and below. `docs/lighting_height.md`'s backlog, and
            // `light::walk_cells_streaming`'s own copy of the same gate — the
            // box is the wire's on both axes, for the same reason `span_of`'s
            // height is.
            if !over_footprint(start, box_of(first, vertical.x, vertical.y, footprint_at(id))) {
                continue;
            }
            // Phase 4's rule, here too — `light::walk_cells_streaming`'s own copy
            // of it in this same shortcut. Every solid this loop sees is on the
            // fragment's own cell by construction, so the owner is the whole of
            // what `cell_stopped` would add, and a ray going straight up or down
            // off a tread is exactly the ray whose only contact with that tread's
            // own top is the point it started at.
            if owner != OWNER_NONE && owner == reference.y && drawn_on(raw_start.z, vertical.x, vertical.y) {
                continue;
            }
            let stops = f32(stands.z) / 255.0 * crosses(
                start.z,
                finish.z,
                vertical.x,
                vertical.y,
                finish.z,
                spread,
            );
            stopped = max(stopped, stops);
        }
        return max(0.0, 1.0 - stopped);
    }
    let last = vec2<i32>(i32(floor(finish.x)), i32(floor(finish.y)));
    // Which side of its own tile the lit end **is the face of**. What it is for
    // is `own_run` below: a run of wall is one surface and no part of a surface
    // shadows another part of it. The pixel's own side and not its tile's whole
    // mask, which is what decision 23 says in words — a corner's *perpendicular*
    // panel is a different surface and stops the ray as it always did. A pixel
    // that is not a face is part of no run and gets nothing. `light::walk_cells_streaming`.
    let own = own_side(stance);
    // Which way each axis steps and how much of the whole segment one tile of
    // it is worth. An axis the ray does not move along never reaches its
    // boundary, which is what the enormous `t` says.
    let toward = vec2<i32>(select(-1, 1, delta.x >= 0.0), select(-1, 1, delta.y >= 0.0));
    var per_tile = vec2<f32>(1.0e30, 1.0e30);
    var boundary = vec2<f32>(1.0e30, 1.0e30);
    // The known tile's own edge, not `floor(lit.x/y)` — the same reason `first`
    // above reads `tile`. A `lit` sitting exactly on this axis' boundary must
    // seed `boundary[axis]` near zero (the ray is already leaving `first`), and
    // `floor(lit.x/y)` there can just as well pick the far side and seed a whole
    // tile of slack that was never there. `light::walk_cells_streaming`'s `boundary[axis]`.
    if abs(delta.x) > 1.0e-6 {
        per_tile.x = 1.0 / abs(delta.x);
        let ahead = select(lit.x - tile.x, tile.x + 1.0 - lit.x, delta.x >= 0.0);
        boundary.x = ahead * per_tile.x;
    }
    if abs(delta.y) > 1.0e-6 {
        per_tile.y = 1.0 / abs(delta.y);
        let ahead = select(lit.y - tile.y, tile.y + 1.0 - lit.y, delta.y >= 0.0);
        boundary.y = ahead * per_tile.y;
    }

    // **Plain single-axis DDA, one cell per step, no diagonal probe on the
    // walk's own trajectory** — `light::walk_cells_streaming`'s own shape,
    // `docs/lighting_raymarch.md`'s point 4 cutover. A walk that never skips
    // a cell is complete by construction (the textbook reason grid-line
    // rasterisation steps one axis at a time), so there is nothing here for
    // `corner_tie`/`panel_stop` to add — see that function's own doc comment
    // for the fault-injection discipline that checked this rather than
    // assumed it. The unconditional probe below is a different, narrower
    // thing: see its own comment at the tie-break.
    var through = 1.0;
    var cell = first;
    for (var i = 0; i < MAX_WALK_STEPS; i = i + 1) {
        let stopped = cell_stopped(
            cell, first, last, own, owner, stance, skip_last, spread, start, finish, raw_start.z, delta, ground,
        );
        through = through * (1.0 - stopped);
        if through <= 0.004 {
            // Under a byte's worth of light: nothing further can matter, and a
            // wall is the common case.
            return 0.0;
        }

        let next = min(boundary.x, boundary.y);
        if next >= 1.0 {
            break;
        }
        // A bare `<` decides which cell the walk's own trajectory visits
        // next, matching `light::walk_cells_streaming`'s own and
        // `light::dda_walk`'s bare comparison — biasing this one with a
        // tolerance was tried and reverted, see `light::walk_cells_streaming`'s
        // own doc comment for why: it changes which cell the walk visits, not
        // just how a near-tie candidate is read, and that broke agreement
        // with `light::dda_walk`'s own trajectory on ordinary geometry.
        let out_by_x = boundary.x < boundary.y;
        // Probe the side this step does not take, without moving the walk's
        // own trajectory there — unconditional, not gated behind how close
        // `boundary.x`/`boundary.y` are. `light::walk_cells_streaming`'s own
        // doc comment has the full argument: a *gated* probe was tried
        // first, and made parity worse rather than better, because CPU and
        // GPU do not compute a close-enough `boundary.x - boundary.y` to
        // agree on which rays even count as near a tie — widening the gate
        // only widened the set of rays where one backend probed and the
        // other did not. Unconditional costs nothing `light::walk_cells_
        // exact` could disagree with either: `light::candidate_tiles`
        // already names both single-axis neighbours at every transition
        // regardless of any tie.
        let probe = select(
            vec2<i32>(cell.x + toward.x, cell.y),
            vec2<i32>(cell.x, cell.y + toward.y),
            out_by_x,
        );
        let probe_stopped = cell_stopped(
            probe, first, last, own, owner, stance, skip_last, spread, start, finish, raw_start.z, delta, ground,
        );
        through = through * (1.0 - probe_stopped);
        if through <= 0.004 {
            return 0.0;
        }
        if out_by_x {
            cell.x = cell.x + toward.x;
            boundary.x = boundary.x + per_tile.x;
        } else {
            cell.y = cell.y + toward.y;
            boundary.y = boundary.y + per_tile.y;
        }
    }
    return through;
}

// How far in from the rim of a beam its edge finishes softening, as a share of
// the way from the rim to the axis. `light::BEAM_EDGE`, and the two are one
// number — a cone with a hard rim reads as a stencil laid over the scene rather
// than as light.
const BEAM_EDGE: f32 = 0.25;

// How much of a beamed flame escapes it in every other direction: a hand is not
// a shutter, and a cone with nothing outside it leaves the character carrying the
// light as the only black thing in the frame. `light::BEAM_SPILL`, and the two
// are one number.
const BEAM_SPILL: f32 = 0.25;

// How much of a flame's beam falls on a spot `offset` away from it — `x` and `y`
// in tiles and `z` in tiles as well, pointing *from* the flame *to* the spot.
//
// `light::Beam::lights`, arithmetic for arithmetic, and the parity test of
// `docs/lighting.md`'s decision 9 is what holds the two together. The smoothstep
// is written out rather than called for the same reason: two texts that mean to
// be one polynomial should be one polynomial.
fn cone(beam: vec4<f32>, offset: vec3<f32>) -> f32 {
    let distance = length(offset);
    if distance < 1.0e-6 {
        // A spot at the flame itself is inside every beam: there is no direction
        // from a point to itself, and a lantern's own tile is not the place to
        // start refusing light.
        return 1.0;
    }
    let along = dot(offset / distance, beam.xyz);
    let inner = beam.w + (1.0 - beam.w) * BEAM_EDGE;
    let t = clamp((along - beam.w) / max(inner - beam.w, 1.0e-6), 0.0, 1.0);
    return BEAM_SPILL + (1.0 - BEAM_SPILL) * t * t * (3.0 - 2.0 * t);
}

// How far along the ground one sunbeam may run, in tiles. `light::MAX_SUN_TILES`.
//
// The bound that matters is the ceiling below — a ray that has climbed above
// everything in the grid is looking at sky and stops there, which over open
// ground is two or three tiles. This is what is left for a sun so low that it
// never climbs out: a shadow thirty-two tiles long is already longer than any
// frame, and an unbounded segment would be a walk with no end at sunset.
const MAX_SUN_TILES: f32 = 32.0;

// How much of the sun reaches a fragment: 1 in the open, 0 in a wall's shadow.
//
// The same walk as a flame's, which is the whole of it now: what the sun has
// instead of a position is a *direction*, so the far end of the segment is
// computed rather than given. It is the point at which the ray leaves the
// grid's ceiling — `lighting.sun.w`, the height above which nothing this frame
// holds can stop anything — because from there on the ray is in the sky. A
// fragment already above the ceiling is in open air and the walk never starts.
//
// `false` and `0.0`: there is no far tile to exempt, the far end being a point in
// the sky, and the sun is half a degree wide, so its penumbra is the narrowest
// `walk` draws. The fragment's own tile is skipped there as it is for a flame,
// which is what lights a wall on the side the sun is on instead of shadowing it
// with itself.
fn sunlight(at: vec3<f32>, tile: vec2<f32>, stance: u32, owner: u32) -> f32 {
    let horizontal = length(lighting.sun.xy);
    if horizontal < 1.0e-6 {
        // Straight overhead: nothing but the fragment's own tile is in the way,
        // and that one is exempt.
        return 1.0;
    }
    // One tile of ground a unit, so `z` climbs by the sun's own slope.
    let step = vec3<f32>(
        lighting.sun.x / horizontal,
        lighting.sun.y / horizontal,
        lighting.sun.z / horizontal * Z_PER_TILE,
    );
    var tiles = MAX_SUN_TILES;
    if step.z > 1.0e-6 {
        tiles = min(tiles, (lighting.sun.w - at.z) / step.z);
    }
    if tiles <= 0.0 {
        // Above everything the grid holds — or the grid holds nothing at all,
        // which arrives here as a ceiling below every fragment there is.
        return 1.0;
    }
    return walk(at, at + step * tiles, tile, stance, owner, false, 0.0);
}

// Where the light view stops being the multiplier and starts being a curve.
//
// The view's problem is that the interesting range is not `0..=1`. A night
// ambient is `0.36`, a torch adds `0.95` on top of it and a lit day is past
// `1.1`, so a clamp — which is what this was — painted the middle of every pool
// one flat white disc, and the middle is exactly the part of a flame's shape
// nothing else shows: the lit frame multiplies the art by it, and the art is
// dark, so the clipping never appears there.
//
// A tone map over the whole range is the other wrong answer, and it was tried:
// pulling `0.36` down to `0.26` and `1.27` to `0.56` leaves a picture where the
// lamp does not read as a lamp. So the curve is identity below `KNEE` and an
// exponential approach to `1.0` above it — everything a person reads as a level
// is untouched, and only what was being clipped is bent. Monotone and unbounded:
// nothing reaches white, so a step or a seam anywhere above the knee is still a
// step, and two flames overlapping still differ from one.
const KNEE: f32 = 0.6;

fn knee(value: f32) -> f32 {
    if value <= KNEE {
        return value;
    }
    let headroom = 1.0 - KNEE;
    return KNEE + headroom * (1.0 - exp(-(value - KNEE) / headroom));
}

// One of the pass's own values, as a colour.
//
// Every argument here is something the lit path already computed — none of these
// views measures anything of its own, which is what makes them evidence about
// *this* frame rather than about a second frame drawn like it. See
// `crate::debug::View` for what each one is for.
fn debug_color(
    view: u32,
    place: vec4<u32>,
    at: vec3<f32>,
    sub: vec2<f32>,
    lit: vec3<f32>,
    flames: vec3<f32>,
    reached: u32,
    nearest: f32,
    nearest_through: f32,
    sun_through: f32,
    share: f32,
) -> vec3<f32> {
    if view == VIEW_SKY {
        // The sky field on the ground it is a field of: white under open air,
        // black under a roof, and a gradient across a doorway. Drawn over the
        // picture rather than as a wireframe of the boxes, because the failure
        // this field actually has is a tile that is *wrongly open* — an eave that
        // did not cover the floor under it — and a box is drawn for what stands,
        // not for what does not. `docs/lighting_world.md`'s backlog says so.
        return vec3<f32>(share, share, 0.15 + share * 0.85);
    }
    if view == VIEW_SUN {
        // How much of the sun arrived: white in the open, black in a wall's
        // shadow, and the sun's own colour where a pane dimmed it rather than
        // stopping it — which is the picture a person looking for a lit patch on
        // a floor is actually after.
        if lighting.sun_color.w <= 0.0 {
            return vec3<f32>(0.0, 0.0, 0.35);
        }
        return vec3<f32>(sun_through);
    }
    if view == VIEW_PLACE {
        // A checkerboard of tiles with the sub-tile fraction laid over it: the
        // squares say which tile a pixel claims, and the gradient inside each
        // one says the fraction is being written at all. A wall reads as one
        // flat square with its sprite standing out of it — which is exactly the
        // property the lighting depends on.
        let checker = f32((place.x + place.y) & 1u) * 0.35 + 0.15;
        return vec3<f32>(sub.x, sub.y, checker);
    }
    if view == VIEW_KIND {
        let kind = place.w & KIND_MASK;
        if kind == 1u {
            return vec3<f32>(0.20, 0.65, 0.30);  // land
        }
        if kind == 2u {
            return vec3<f32>(0.25, 0.45, 1.00);  // a static or an item
        }
        return vec3<f32>(1.00, 0.40, 0.15);      // a mobile
    }
    if view == VIEW_HEIGHT {
        // A ramp over the `z` a map actually uses, with a band every tile's
        // worth of height: the ramp is for reading a slope, the bands are for
        // counting storeys, and eleven units is one tile — the ratio that
        // decides whether a cellar's flame reaches the street.
        let ramp = clamp((at.z + 64.0) / 128.0, 0.0, 1.0);
        let band = fract(at.z / Z_PER_TILE);
        return vec3<f32>(ramp, band * 0.8, 1.0 - ramp);
    }
    if view == VIEW_OCCLUDERS {
        let cell = merged_at(i32(floor(at.x)), i32(floor(at.y)));
        if cell.edges == 0u {
            return vec3<f32>(0.06, 0.06, 0.10);
        }
        let opacity = f32(cell.opacity) / 255.0;
        // Red where this pixel's own height is inside the span the tile stops
        // light at, blue where the tile occludes but not here. The second colour
        // is the one worth having: a wall of an upper storey standing over an
        // open street is blue, and a shadow appearing under it would be the bug.
        if at.z >= cell.low && at.z <= cell.high {
            return vec3<f32>(0.25 + 0.75 * opacity, 0.05, 0.05);
        }
        return vec3<f32>(0.05, 0.10, 0.25 + 0.55 * opacity);
    }
    if view == VIEW_LIGHT {
        // The lighting with the art thrown away: the pools' own shapes, where a
        // seam or a step has nothing to hide behind. `KNEE` is why it is not a
        // clamp and not a tone map either.
        return vec3<f32>(knee(lit.r), knee(lit.g), knee(lit.b));
    }
    if view == VIEW_FLAMES {
        // And the same pools with the ambient taken out, on black: what the
        // *flames* added and nothing else. No curve here — a flame's whole range
        // is already `0..=1` and a little over, and bending it is what makes the
        // view above unable to answer whether a pool has a shape at all.
        return clamp(flames, vec3<f32>(0.0), vec3<f32>(1.0));
    }
    if view == VIEW_SHADOW {
        // Blue for "no flame reaches here at all", which is a different fact
        // from "a wall is in the way" and the one people confuse.
        if nearest >= 1.0 {
            return vec3<f32>(0.0, 0.0, 0.35);
        }
        // Dark red for "a flame reaches, and is fully blocked" — the walk's
        // own answer of "none", not silence. Left as pure black this read
        // identically to the cleared background this fragment never reaches
        // (`KIND_NOTHING`, above, returns `color` before this view is ever
        // asked), and that has cost a wrong diagnosis twice: a shadowed pixel
        // beside empty background read as an orphaned shape, and a face's own
        // outline read as fewer sides than it has. `docs/lighting_raymarch.md`
        // step 1. `0.004` is `light.rs`'s own `RAY_CUTOFF`.
        if nearest_through <= 0.004 {
            return vec3<f32>(0.2, 0.0, 0.0);
        }
        return vec3<f32>(nearest_through);
    }
    // VIEW_REACH: how many flames got through, green through red, and the same
    // blue as above for none.
    if reached == 0u {
        return vec3<f32>(0.0, 0.0, 0.35);
    }
    let many = min(f32(reached) / 4.0, 1.0);
    return vec3<f32>(many, 1.0 - many * 0.7, 0.15);
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    // Straight through: both sides are `Rgba8Unorm` and this crate never
    // converts colour. Whatever filtering happens is the sampler's, chosen by
    // the caller from which way the zoom went.
    let color = textureSample(world, world_sampler, in.uv);

    // Which tile this pixel's picture came from. `textureLoad` and not a
    // sampler, and the texel is the nearest one whatever the zoom: a place is an
    // identity, and half of one is nobody.
    let size = vec2<f32>(textureDimensions(place_of));
    let texel = vec2<i32>(clamp(in.uv * size, vec2<f32>(0.0), size - vec2<f32>(1.0)));
    let place = textureLoad(place_of, texel, 0);

    let view = lighting.view.x;

    // Nothing was drawn here — the cleared background, or the letters over a
    // speaker's head, which are a message and not a thing standing in the
    // street. Neither is lit and neither is dimmed. See `crate::place::Kind`.
    //
    // The diagnostics leave it alone too, and deliberately: a view that painted
    // the background would make the world's own silhouette hard to find, and
    // every one of these is read by comparing a shape against the picture it
    // came from. The one exception is the kind view, whose whole subject is
    // which pixels are nothing.
    if (place.w & KIND_MASK) == KIND_NOTHING {
        if view == VIEW_KIND {
            return vec4<f32>(0.0, 0.0, 0.0, color.a);
        }
        return color;
    }

    // Where in the world this pixel is: a tile, and the fraction of it this
    // fragment sits at. The fraction is what makes a pool a gradient rather
    // than a set of flat tiles with a step at every edge — true of the ground
    // and equally true of a static's own face, which is why `statics.wgsl`
    // computes one for a wall's run and a floor's spread alike (see its own
    // comment: without it a row of wall tiles would light as separate flat
    // panels instead of one continuous surface across the seam). It is packed
    // identically for every kind, in the same bits, and decoded once here.
    //
    // The **tile itself** is not: `docs/gbuffer.md` step 3 moved it to a
    // per-kind instance buffer for a static or a mobile, and step 7 did the
    // same for the ground — the fragment's own picture used to repeat it into
    // every pixel it covers, and `place.x`/`place.y` carry an id into the
    // right buffer instead.
    let kind = place.w & KIND_MASK;
    let sub = vec2<f32>(
        f32((place.w >> 2u) & SUB_TILE_MASK) / SUB_TILE,
        f32((place.w >> 9u) & SUB_TILE_MASK) / SUB_TILE,
    );
    // Read before the id lookup below, not after: a mesh face's sentinel
    // (`STANCE_MESH_FACE`) decides *which* buffer the id addresses, and the
    // real stance a mesh face's normal names lives in that buffer's own row,
    // not in this word — see the branch below and
    // `crate::place::Stance::MeshFace`'s own doc.
    var stance = (place.z >> PLACE_STANCE_SHIFT) & PLACE_STANCE_MASK;
    let id = place.x | (place.y << 16u);
    var tile = vec2<f32>(0.0);
    // And which occluder of that tile this fragment is a point of — the row's
    // own, off the same fetch the tile comes from, `docs/lighting_height.md`
    // phase 3. Land keeps `OWNER_NONE`: the ground is no occluder, so it is
    // exempt from none of them, which is what it always was in effect.
    var owner = OWNER_NONE;
    if kind == KIND_LAND {
        tile = vec2<f32>(
            f32(ground_instances[id].place0 & 0xFFFFu),
            f32(ground_instances[id].place0 >> 16u),
        );
    } else if kind == KIND_STATIC && stance == STANCE_MESH_FACE {
        // `docs/gbuffer.md` step 4c: a mesh face has no picture of its
        // own, so its row lives in `mesh_instances`, not
        // `face_instances` — and carries the *real* stance the
        // attachment's sentinel stood in for.
        let row = mesh_instances[id];
        tile = vec2<f32>(f32(row.tile & 0xFFFFu), f32(row.tile >> 16u));
        stance = row.stance;
        owner = row.owner;
    } else {
        var row_place: vec2<u32>;
        if kind == KIND_STATIC {
            row_place = face_instances[id].place;
            owner = face_instances[id].owner;
        } else {
            row_place = mobile_instances[id].place;
            // A mobile's row stamps `OWNER_NONE` on the CPU side
            // (`mobiles::collect`), so this reads it rather than assuming it —
            // one place decides what a billboard is a point of, and it is the
            // one that builds the row.
            owner = mobile_instances[id].owner;
        }
        tile = vec2<f32>(f32(row_place.x & 0xFFFFu), f32(row_place.x >> 16u));
    }
    // The height is the channel's two fields together — whole units and the
    // sixteenths above them, `docs/lighting_height.md` phase 1. Through
    // `unpack_place_z` rather than by hand: reading only the low eight bits
    // still compiles, still draws, and quietly puts every fragment of a
    // vertical face back on the staircase of one-unit treads the fraction
    // exists to remove.
    let at = vec3<f32>(tile.x + sub.x, tile.y + sub.y, unpack_place_z(place.z));
    // And which way the surface drawn here looks, where it is a wall's face. Zero
    // for the ground, for a mobile and for anything standing up whose art would
    // not name an edge — none of those has a side to be lit from.
    //
    // `stance` alone no longer tells land apart from that "nothing standing up"
    // case: `ground.wgsl` now stamps `STANCE_FLAT` (its own doc comment has why
    // — `cell_stopped`'s exemption needs to see it), the same value a wall's own
    // flat cap carries. A cap wants `faces`'s half-space gate — decision 27, the
    // reason `STANCE_FLAT` looks up at all — but ground never has: a floor lit
    // at a grazing angle is not behind its own plane the way a wall's cap can be
    // behind a torch mounted low, and gating it darkened every ground pixel a
    // fixture's flame was not comfortably above, which nothing before this
    // stamped `stance` for land ever exercised. `kind` alone, not `stance`,
    // is what actually distinguishes the two.
    let normal = select(outward(stance), vec3<f32>(0.0), kind == KIND_LAND);

    // The ambient this *tile* has: the sky's share of it scaled by how much of
    // the sky the column over the tile can see, plus the floor everything gets.
    // `docs/lighting_world.md`, decision 1, and it is the whole of why a room is
    // darker than the road outside it before anything burns.
    let share = sky_at(i32(floor(at.x)), i32(floor(at.y)));
    var lit = lighting.ground.rgb + lighting.sky.rgb * share;
    // What the diagnostics are made of, gathered while the frame is lit rather
    // than computed a second time: how many flames actually reached this
    // fragment, and what the *nearest* of them lost on the way. Nearest and not
    // an average, because a shadow view is read as "is this pixel behind
    // something", and the flame that answers that is the one lighting it most.
    var reached = 0u;
    var nearest = 1.0e9;
    var nearest_through = 0.0;
    // What the flames added, apart from the ambient they were added to. Only
    // `VIEW_FLAMES` reads it, and it is accumulated here rather than recomputed
    // there for the reason every one of these values is: a diagnostic that lit its
    // own copy of the frame would answer about that copy.
    var flames = vec3<f32>(0.0);
    let count = u32(lighting.sky.w);
    for (var i = 0u; i < count; i = i + 1u) {
        let light = lighting.lights[i];
        let to = light.place.xyz;
        // All three axes in one unit: `z` is divided into tiles, so a flame
        // reaches as far up and down as it does sideways. Without it the screen's
        // own folding comes back — a cellar lighting the street.
        let offset = vec3<f32>(to.x - at.x, to.y - at.y, (to.z - at.z) / Z_PER_TILE);
        let d = length(offset) / max(light.place.w, 0.001);
        if d >= 1.0 {
            // Outside the pool entirely, which most fragments are for most
            // flames: no ray is walked and nothing is sampled. This is why the
            // grid walk below is affordable at all.
            continue;
        }
        // Which way it is pointing, where it points one way at all. A dot
        // product and a branch: a fire standing in the world writes a rim below
        // every cosine there is and never reaches the arithmetic.
        //
        // `offset` runs from the fragment to the flame and with `z` already
        // divided into tiles, which is the space the axis is stated in; a beam's
        // axis runs the other way, so the sign flips here.
        var lit_by = 1.0;
        if light.beam.w > -1.0 {
            lit_by = cone(light.beam, -offset);
        }
        // And whether the flame is on the side this surface looks at. A wall is
        // one-sided and its two faces are one tile, so this is the only thing that
        // keeps a torch in a room from lighting the outside of the house.
        //
        // Geometry and nothing else. This used to exempt a flame standing in the
        // wall's own row or column, because a lamp mounted on a house sits at its
        // tile's centre — behind the plane of the face it is bolted to — and
        // testing it blacked out the very wall it hangs on. But a *line* is the
        // whole length of a street: a lamp post standing south of a house is in
        // the column of its east wall, and that wall's faces came out fully lit
        // from a flame half a tile behind their plane, for the length of the run.
        // Reported from the client at Britain's `(1441, 1692)`. What answers it is
        // moving the mounted flame instead of excusing it — `light::mounted_at`,
        // and `docs/lighting.md`'s decision 26.
        // `offset` is already the flame less the fragment with `z` divided into
        // tiles, which is the space the normal is stated in — so a lid's answer is
        // how far *above* its plane the flame is, in tiles, through the same
        // formula and the same soft band.
        if any(normal != vec3<f32>(0.0)) {
            lit_by = lit_by * faces(normal, offset);
        }
        // The flame's own tile is exempt, and a flame is a body a tile wide:
        // `walk`'s two parameters, and the only two things that make this ray
        // different from the sun's.
        let through = walk(at, vec3<f32>(to.x, to.y, to.z), tile, stance, owner, true, FLAME_SPREAD);
        // Recorded before the shadow is tested, so that a fragment inside a pool
        // and behind a wall is *nearest, stopped* rather than indistinguishable
        // from open ground nothing reaches. That difference is the one the whole
        // shadow view exists to show.
        if d < nearest {
            nearest = d;
            nearest_through = through;
        }
        if through <= 0.0 {
            continue;
        }
        reached = reached + 1u;
        // Linear distance, squared falloff: `1 - d` alone gives a cone with a
        // visible straight edge in the gradient, and an inverse-square law with
        // no cutoff has no radius at all and tints the whole frame. This is the
        // soft pool with a hard end — the shape the reference isometrics draw.
        let fall = 1.0 - d;
        let added = light.color.rgb * (light.color.w * fall * fall * through * lit_by);
        lit = lit + added;
        flames = flames + added;
    }

    // And the sun, which is one direction rather than a place: no distance and no
    // falloff, only whether anything stands between this pixel and the sky.
    var sun_through = 0.0;
    if lighting.sun_color.w > 0.0 {
        sun_through = sunlight(at, tile, stance, owner);
        lit = lit + lighting.sun_color.rgb * (lighting.sun_color.w * sun_through);
    }

    if view != VIEW_LIT {
        return vec4<f32>(
            debug_color(
                view, place, at, sub, lit, flames, reached, nearest, nearest_through, sun_through,
                share,
            ),
            color.a,
        );
    }

    // Multiplicative, so a flame brightens whatever is under it — ground, wall,
    // the body standing in it — rather than laying a coloured film over it. A
    // pixel the art left black stays black, which is what makes the light look
    // like it is falling on the scene instead of floating above it.
    //
    // **Through `shade`, which is the whole of `docs/lighting_rebuild.md`'s
    // phase 1**: the art is decoded out of sRGB, the multiplication happens in
    // linear radiance, and the result is curved and encoded once. What this
    // replaced was `min(color.rgb * lit, 1.0)` — a multiplication of stored
    // bytes, in which two equal flames are not twice one flame and a surface at
    // half the distance is not four times as bright, so no reference renderer
    // could agree with this pass even in principle.
    //
    // `Lighting::NONE` is no longer a texel-for-texel copy of the world image
    // and cannot be: a light of exactly `1.0` still goes through the curve, and
    // a curve that left its input alone would not be one. The frame test that
    // asserted the copy now asserts `crate::tonemap::shade_u8` of it, which is a
    // stronger statement — it catches a blit that shifts by a texel *and* a
    // colour pipeline that drifts from its own twin.
    //
    // Alpha carried through untouched, not written as `1.0`: this pass shades an
    // image and lighting is about how bright a pixel is, not about whether it is
    // there.
    return vec4<f32>(shade(color.rgb, lit), color.a);
}
