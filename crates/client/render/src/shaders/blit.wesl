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
    KIND_NOTHING, KIND_LAND, KIND_STATIC,
    ids_kind, ids_stance, ids_id, ids_edges, IDS_EDGE_ART, IDS_EDGE_BOX,
    IDS_DYNAMIC_ITEM, IDS_COMPOSITE_MAP,
    STANCE_MESH_FACE, STANCE_FLAT, STANCE_FACE_NORTH,
    STANCE_FACE_EAST, STANCE_FACE_SOUTH, STANCE_FACE_WEST,
};

import package::normal_format::unpack_normal;

// Which solid a fragment is a point of, and how it rides in the position
// plane's fourth channel — written by `statics.wgsl`, read here, and the whole
// of what `own_solid`'s cell scan used to guess at. See that file's own header.
import package::solid_format::{SOLID_NOBODY, unpack_solid};

// The world's one metric, declared where the view ray needs it and imported
// here rather than restated — `docs/lighting_rebuild.md`'s *One metric*.
import package::impostor::Z_PER_TILE;

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
// What drew each pixel of `world`: the kind, the stance and an id into one of
// the four instance buffers below — `crate::gbuffer::IDS_FORMAT`, one `u32`,
// and `place_format.wesl` is the layout. An id and not a literal tile since
// `docs/gbuffer.md` steps 3 and 7 moved it there for every kind. Never sampled
// — an integer texture has no filtering, and two ids averaged name a third row
// nothing was drawn from.
@group(0) @binding(3) var ids_of: texture_2d<u32>;
// And where that pixel's fragment actually is: `(x, y, z, 1)` in world
// coordinates, `crate::gbuffer::POSITION_FORMAT`. Never sampled either, and
// this one for a reason of its own — a position filtered against its
// neighbour's is a point on neither surface, floating in the air between a
// wall and the ground behind it.
@group(0) @binding(13) var position_of: texture_2d<f32>;
// And which way its surface looks: one octahedral word, or a word with no
// facing bit for a surface with no known one — `crate::gbuffer::NORMAL_FORMAT`,
// and `normal_format.wesl` is the layout. Never sampled — an integer texture has
// no filtering, and it would be wrong here anyway: the average of two unit
// vectors is not a unit vector, and the average of a wall's normal and the
// ground's behind it points into the seam between them.
//
// **This used to be `outward(stance)`, computed here.** A fragment's facing is
// something the pass that drew it knows — `statics.wesl` resolves a corner's
// two faces per fragment, and the mesh pass has a measured normal per face —
// and the lighting was recovering a rounded copy of it from four bits at the
// far end of a `u16`. `docs/lighting_rebuild.md` phase 2, the same move the
// position plane is.
@group(0) @binding(14) var normal_of: texture_2d<u32>;
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
// And the primitives themselves — one struct each, indexed by a `SolidId`
// outright. `crate::occlusion::Occlusion::primitive_bytes`.
//
// **A storage buffer and not three textures**, `docs/occluders.md`'s D8. What
// stood here was `solids` (opacity and flags), `footprints` (a fraction of a
// tile, a byte an axis) and `solid_z` (sixteen bits an end), three planes
// indexed by one number and each a different encoding of one box — because
// decision 30.5's ceiling was WebGL2, where a list a shader can index has to be
// a texture. Phase 6a settled the ceiling as WebGPU, this file already reads
// eleven storage buffers, and a primitive is a struct.
//
// **Its two corners are absolute world coordinates**, D1: no cell is their base
// and there is no fraction of one anywhere in this pass. A box wider than a
// tile, one standing between two, and one a thousandth of a tile deep are the
// same six numbers.
struct Primitive {
    lo: vec3<f32>,
    // PRESENT | HOLED | edges, in the padding a `vec3<f32>` leaves behind.
    flags: u32,
    hi: vec3<f32>,
    opacity: u32,
};

@group(0) @binding(6) var<storage, read> primitives: array<Primitive>;
// The hole in each of those solids, indexed by the same number —
// `occlusion::Aperture`. A list beside the primitives rather than four more
// fields of one, because the primitives are what the walk reads in a loop and a
// hole is what almost nothing has: the `HOLED` bit is what makes this read at
// all, so an ordinary wall pays one bit test and no fetch.
//
// **Absolute world coordinates and nothing quantised**, `docs/occluders.md`'s
// S6. It was an `Rgba8Uint` texel — `(near, far, z_bottom + 128, z_top + 128)`,
// the run pair in two-hundred-and-fifty-fifths of a tile and the two heights
// clamped into a signed byte — which made this the last plane indexed by a
// `SolidId` and the last quantised number in the pass.
struct Aperture {
    // Where the hole starts and ends along the panel's own run, on whichever
    // world axis that is: `x` for a north or south face, `y` for an east or west
    // one. `along_the_run` picks between them.
    near: f32,
    far: f32,
    // And its two `z` ends, absolute, in the map's own whole units.
    bottom: f32,
    top: f32,
};

@group(0) @binding(7) var apertures: texture_2d<f32>;
// And the references between the cells and the solids, one texel each and folded
// the same way: (id & 255, id >> 8, id >> 16, unused). A cell counts through a
// run of these and each one names a solid — decision 38's ownership, which is
// what lets one box be referenced by every cell it stands over.
@group(0) @binding(8) var solid_ids: texture_2d<u32>;

// The broad phase: `crate::occlusion::bvh::Bvh` as two buffers —
// `docs/occluders.md`'s S5, and the same tree both CPU walks read.
//
// **A hierarchy over the primitives and no longer the grid of tiles.** What the
// grid could not do is hold a primitive wider than the cell it was registered on
// without either listing it twice (and then double-counting it, since `through`
// is a product) or losing the ray that crosses only the overhang. A leaf holds a
// primitive whatever its size, exactly once.
//
// The layout is `Occlusion::node_bytes`, and it is `Primitive`'s own shape: two
// corners with a `u32` in the padding each `vec3<f32>` leaves behind.
struct Node {
    lo: vec3<f32>,
    // Where a ray that **misses** this node continues: the first node after this
    // whole subtree, in the depth-first order. A hit continues at `at + 1`, which
    // is this node's own first child — that is the whole of what depth-first
    // buys, and the reason there is no stack in this file.
    escape: u32,
    hi: vec3<f32>,
    // The primitives, for a leaf: `first << 3 | count`, a run of `bvh_order`.
    // A count of zero is an inner node, which holds none — and a leaf holds at
    // most four, so three bits is the whole of the count.
    leaf: u32,
};

@group(0) @binding(15) var<storage, read> bvh_nodes: array<Node>;
// Every primitive of the frame exactly once, permuted so a leaf's own are
// contiguous. `Occlusion::order_bytes`, and a `SolidId` a word.
@group(0) @binding(16) var<storage, read> bvh_order: array<u32>;

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
    // Which picture to draw: `crate::debug::View` in the first component,
    // `shadow_rays` in the second, whether this is a ghost's frame in the
    // third — `light::Lighting::dead`, `docs/combat.md`'s D9 — and whether the
    // fourth selects a short lighting path: zero is the full path, one is an
    // identity and two is flat ambient only. The first component is zero for the lit
    // frame and every other value is a diagnostic drawn from the values this
    // pass lit it with — see `docs/lighting.md`, decision 8. The numbers are
    // pinned in a test on the Rust side.
    view: vec4<u32>,
    // Which way the sun is — `x` and `y` in tiles, `z` in tiles as well — and in
    // the fourth component the height above which nothing in this frame's grid
    // can stop it. A ray that has climbed past that is in the sky and stops
    // walking, which is what makes a daylit frame affordable.
    sun: vec4<f32>,
    // Its colour, then how much it adds. An intensity of zero is "no sun", and
    // it is the only thing tested: a night frame never enters the walk.
    sun_color: vec4<f32>,
    // The final alpha composition. The opaque world writes one; the private
    // cutaway layer writes its policy opacity and this shader premultiplies its
    // lit result before the source-over pipeline consumes it.
    composite: vec4<f32>,
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
const VIEW_NORMAL: u32 = 11u;
const VIEW_SOLID: u32 = 12u;
const VIEW_NORMAL_GEOMETRY: u32 = 13u;
const VIEW_NORMAL_SPRITES: u32 = 14u;
const VIEW_SILHOUETTE_ART: u32 = 15u;
const VIEW_SILHOUETTE_BOX: u32 = 16u;

@group(0) @binding(2) var<uniform> lighting: Lighting;

fn with_composite_opacity(color: vec4<f32>) -> vec4<f32> {
    let opacity = lighting.composite.x;
    return vec4<f32>(color.rgb * opacity, color.a * opacity);
}

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
// `owner` *is* read here, unlike `twin`: which occluder of its own tile the
// static drawn on a pixel is. **It is a key and no longer the comparison** —
// `docs/lighting_rebuild.md` phase 4 compares a `SolidId`, and a sprite instance
// covers a corner's two panels, so `own_solid` narrows this owner by the
// fragment's own stance to name one of them. `crate::sprite::SpriteQuad`.
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
// static stands on, which way that face really faces, and **which solid of the
// grid it is the drawn form of** — a mesh face has no picture, so it shares no
// field with `FaceInstance` and lives in its own buffer instead.
// `crate::mesh_face::MeshFaceRow`, laid out identically.
//
// `solid` was an owner until `docs/lighting_rebuild.md` phase 4. A mesh face is
// one primitive by construction, so it can name it outright, which is what lets
// a flight's treads stop shadowing each other without a height being read
// anywhere — see `own_solid` for the sprite side, which cannot.
struct MeshFaceInstance {
    tile: u32,
    stance: u32,
    solid: u32,
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
// by `ground.wgsl` and carried in the position plane already, not a fact the
// row holds.
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
@group(0) @binding(17) var<storage, read> item_instances: array<FaceInstance>;

// **`OWNER_NONE` stood here**, and it went with `own_solid` — the last thing in
// this pass that compared an *owner* at all. What a fragment is a point of is a
// `SolidId` (`docs/lighting_rebuild.md` phase 4) and it arrives now in the
// position plane's fourth channel, written by the pass that met the box:
// `solid_format.wesl` carries the whole argument, and `SOLID_NOBODY` is
// imported from it rather than spelled a second time here.
// `FaceInstance::owner` and `mobile_instances`' own copy of it stay in their
// structs for the stride's sake, unread — the same reason `twin` is there.

// The stance rides in the id word above the kind — `ids_stance`, and
// `statics.wgsl` writes it. **It is no longer read here for a normal**:
// `normal_of` carries the vector itself, and what is left of the stance for
// this pass is two things. Which instance buffer an id addresses, since a mesh
// face's row lives in its own (`STANCE_MESH_FACE`); and `own_side`, the shadow
// walk's own-run test, which asks which *edge of its tile* a fragment's surface
// stands on rather than which way it looks. `docs/lighting_rebuild.md` phase 4
// retires the second. `STANCE_FLAT` is no longer imported at all, and that is
// the measure of what the normal plane took away: the one flat value this pass
// used to read was for a facing.

// How much of a flame at `toward` reaches a surface facing `normal`:
// `max(N · L, 0)`, and nothing else.
//
// Textbook Lambert, with no wrap, no band and no width knob — `light::lit_from`'s
// own doc is the argument, and `docs/lighting_rebuild.md` is the decision.
//
// **The one thing a fraction cannot say.** A wall's two faces are one tile and
// one plane — the same tile, the same fraction, the same height — so without a
// facing a torch inside a room lights the outside of the house exactly as
// brightly as the inside. That was reported from the client as a wall behaving
// like glass, and it is what this fixes.
//
// `toward` is the fragment-to-flame offset in tiles and **it is normalised
// here**: fed unnormalised, the same expression answered about a distance along
// the normal rather than about an angle. `docs/lighting_rebuild.md` phase 3.
//
// `light::lit_from`, and the two are one formula.
fn lit_from(normal: vec3<f32>, toward: vec3<f32>) -> f32 {
    let span = length(toward);
    // A fragment standing exactly on the flame has no direction to be lit from,
    // and full is the answer that does not put a black dot in a pool's middle.
    if span <= 0.0 {
        return 1.0;
    }
    return clamp(dot(normal, toward / span), 0.0, 1.0);
}

// **`MAX_WALK_STEPS` stood here and went with the grid** —
// `docs/occluders.md`'s S5, which deleted `light::MAX_WALK_STEPS` at the same
// time. It was how many cells of the grid one ray could look at, and it had to
// exist because a cell count is the *ray's* own length: widen a radius and the
// walk gets longer, so a loop over data would have become unbounded.
//
// The tree's traversal is bounded by the tree, and there is nothing left to
// size: a miss moves to that node's escape and a hit to `at + 1`, both strictly
// forward, so each node is visited at most once. See `walk`'s own loop.

// **`FLAME_SPREAD`, `SOFT_CROSSING_MIN`, `SOFT_CROSSING_MAX` and `FLAME_DEPTH`
// stood here** and went at `docs/lighting_rebuild.md` phase 5, with
// `light.rs`'s own four. They were the analytic penumbra: the flame's size in
// tiles, times `t / (1 - t)` for how far along the ray the occluder stands,
// bounded at both ends, converted into `z` by a flame's height. One ray
// reporting what eight would have found — the similar-triangles answer, which
// costs one division rather than a second ray, and which is a *picture of* a
// penumbra rather than a flame with a size.
//
// Eight rays are cast now and each of them answers about itself. See
// `light::FLAME_RADIUS`.

// How big a flame is: the radius of the sphere `shadow` samples, in tiles.
//
// **On the wire and no longer a constant** — `lighting.ground.w`, which is
// `light::Lighting::flame_radius`. A frame draws `light::FLAME_RADIUS`, an
// eighth of a tile, and the art is what says so: a torch's drawn flame is eight
// or ten screen pixels tall, four pixels to a `z`, so two and a half `z` of a
// tile's eleven. What the field buys is a *comparison* that can ask for zero —
// eight rays at a sphere are an estimate, and a gate laid against a path tracer
// otherwise reports that estimate's noise as a disagreement. See that field.
//
// A channel of the header that held a stated zero until a reader existed for it,
// which is this file's own rule for claiming one.
fn flame_radius() -> f32 {
    return lighting.ground.w;
}

// How many rays a fragment casts at each flame.
//
// **On the wire and no longer a constant**, exactly as `flame_radius` above is
// and for the same reason: it is a number a person turns while looking at the
// frame. It rides in the second word of `view`, which was three words of
// padding the block needed anyway — `light::Lighting::shadow_rays`, and
// `light::ShadowRays` is what clamps it to something a loop can survive. The
// spiral's spacing is stated in it (`sqrt((ray + 0.5) / rays)`), so the count is
// not a truncation of one pattern but a pattern of its own.
fn shadow_rays() -> u32 {
    return lighting.view.y;
}

// Whether the player's own body is a ghost. `light::Lighting::dead`, the
// third word of the same padding `shadow_rays` claims above — `docs/combat.md`
// D9: one uniform, one branch, and the whole lit frame desaturates.
fn dead() -> bool {
    return lighting.view.z != 0u;
}

// Turns of the sample spiral between one point of a flame and the next: the
// golden angle, `π(3 − √5)`, which is what makes a Vogel spiral of any length
// evenly spread rather than spoked. `light::GOLDEN_ANGLE`, written out on both
// sides so the two are the same bits.
const GOLDEN_ANGLE: f32 = 2.3999632;
// And a whole turn, for the per-fragment rotation. `std::f32::consts::TAU`.
const TAU: f32 = 6.2831855;

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
// And the bit that says this surface has a hole in it, so that its entry in the
// aperture list means something. `occlusion::HOLED`.
const HOLED: u32 = 64u;

// **`RUN_STEPS` stood here** — 255, how many parts of a tile one step of a
// hole's run coordinate was worth — and went with the quantisation at
// `docs/occluders.md`'s S6. Both sides read the same byte and divided it by this
// number, which is what made them agree exactly; they read the same `f32` now,
// which is the same guarantee with nothing to divide. The constant still exists
// as `occlusion::RUN_STEPS`, where the *art* measures a hole off a picture —
// `facing::Hole`'s own quantum, on the CPU, once, at build time.

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

// **`pierces` stood here** and went at `docs/lighting_rebuild.md` phase 5, with
// `light::pierces`. It was a panel's own span softened over a band `tall` `z`
// units wide, centred on the top edge and hanging below the bottom one — the one
// asymmetry it had, and not a cosmetic one: a wall is based at the height of the
// ground it stands on, and the ray a person actually looks at (a torch and a
// floor, both at `z = 0`) runs exactly along that bottom edge, so a band centred
// there too would have let every wall in the frame pass half its light along the
// ground.
//
// Its *hard* half was already answered before it was ever called: a caller
// reaches it only with a `ray_vs_solid` hit against the very box whose `z` bounds
// it was handed, at that crossing's own midpoint. So what it added was the band,
// and the band is eight rays now.

// **`crosses` lived here, and `docs/parity.md`'s P4 step 1 deleted it** — with
// `light::crosses`, its twin, which is the only way a formula in two languages
// goes away at all.
//
// It answered whether a **lid** was in the way of a ray, as a crossing of a plane
// rather than a passage through a box: 1 where the ray went through and out the
// other side, 0 where it stayed on one side. It had to exist because a lid *was*
// a plane — a floor is `height 0` in `tiledata.mul`, 4,534 of the 4,647 lids over
// the block of Britain `artscan`'s `column` example reads — so the length rule
// beside it got zero out of every floor in the world, which is what lit the
// storey above a torch through its own floorboards (`scene::storey_over_a_torch`).
// Its strictness was the second half: a ray running exactly along the top of a lid
// — a candle standing on the floor it lights — has not gone through anything, and
// counting that touch would have laid half a floor's shadow across every room lit
// from inside it.
//
// A lid is a box a `LID_THICKNESS` deep now (`occlusion::Solid::box_of`), so both halves
// are `ray_vs_solid`'s: a ray that leaves the top face never enters the box below
// it, and one that goes from the storey above to the storey below genuinely
// crosses a volume. A plane's worth of special pleading in two shading languages
// is what giving a floor real bounds bought.

// **`STAND_OFF` and `ON_TOP` lived here**, and `docs/lighting_rebuild.md` phase 4
// deleted them: **the bias is zero.** They were two steps of the seven-bit
// fraction the retired `place` attachment carried and well under the one `z` unit
// it quantised a height to — numbers off a byte layout, standing in for a ray
// that did not start inside the surface it was drawn on. Identity does that
// exactly now, and phase 2's exact position plus a per-solid `ray_vs_solid`
// removed the second thing they bought. `light.rs`'s own note where the pair
// stood has the whole argument.

// **`drawn_on` lived here**, and `docs/lighting_rebuild.md` phase 4 retired it
// along with its twin — see `light.rs`'s own note where the Rust one stood. It
// asked whether a lid was a plane the fragment was drawn at the height of, which
// is how one static's several lids were told apart when identity was per static.
// A fragment names its own solid now, and `walk`'s `raw_start.z` is no longer
// carried through two functions to be compared against a span.

// **`inside` stood here** and went at phase 5 with `light::inside`. It was
// `pierces` with the asymmetry taken out — a soft interval, a band wide at each
// edge — and its one caller was `hole`, because a window's edges are in the
// middle of a surface and no ray runs along them by construction, so they
// softened both ways where a wall's own top softened one way. Both bands are
// eight rays now.

// Which world coordinate runs **along** a panel: a point of it, on the axis its
// own hole is measured in.
//
// A panel on a north or south side lies in a plane of constant `y`, so what runs
// along it is `x`; an east or west one is the other way round. That is the whole
// of a surface's own coordinate system, and it is why only a *named* panel may
// have a hole: a lid and a body have no run to measure one along.
//
// It was `run_v` and returned `along - floor(along)`, the fraction of whichever
// tile the crossing landed in — `docs/occluders.md`'s S6, and the last `floor`
// in this file. `light::along_the_run`.
fn along_the_run(edges: u32, px: f32, py: f32) -> f32 {
    return select(py, px, (edges & (EDGE_NORTH | EDGE_SOUTH)) != 0u);
}

// Whether a surface is **missing** where a ray goes through it: 1 inside the
// hole, 0 outside it.
//
// Both spans have to hold, which is what an opening in a wall is. They met with
// `min` of two soft intervals until phase 5 — deliberately `min` and not a
// product, so that a point halfway into the hole across *and* halfway up read as
// one corner's own softening rather than as a quarter of a hole. With no band on
// either span the `min` is a conjunction and says so. `light::hole`.
fn hole(cut: vec4<f32>, along: f32, z: f32) -> f32 {
    let across = along >= cut.x && along <= cut.y;
    let up = z >= cut.z && z <= cut.w;
    return select(0.0, 1.0, across && up);
}

// How much of a surface stands in the way at the point a ray goes through its
// plane: all of it, less the hole in it.
//
// The whole of `docs/lighting.md`'s step 21.3, and the reason it is this small is
// decision 30.7 — a panel was already *pierced at a point* rather than travelled
// through, so the point was already being computed and a window is what that
// point is asked about. `id` is the solid's own place in the list, which is
// where its hole is; `cross` is where the ray goes through the plane, in all
// three, because the height alone cannot say whether the ray went through the
// window or through the wall beside it.
//
// **The span itself is not asked about any more** — phase 5, and the `pierces`
// call that stood here. A caller reaches this only with a `ray_vs_solid` hit
// against the box `low`/`high` were the `z` bounds of, at that crossing's own
// midpoint, so `cross.z` was inside the span by construction and the band was the
// whole of what the call added. `light::pierced`.
fn pierced(stands: Primitive, id: u32, cross: vec3<f32>) -> f32 {
    if (stands.flags & HOLED) == 0u {
        return 1.0;
    }
    let along = along_the_run(stands.flags & EDGE_MASK, cross.x, cross.y);
    let width = textureDimensions(apertures).x;
    let aperture_at = vec2<i32>(i32(id % width), i32(id / width));
    return 1.0 - hole(textureLoad(apertures, aperture_at, 0), along, cross.z);
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

// Which axis the plane a stance lies in runs across (0 `x`, 1 `y`, 2 `z`), and
// which end of a box that plane is — `.y` of `1.0` for the box's `hi` corner,
// `-1.0` for its `lo`. `.x` is `-1.0` for a stance that has no plane at all,
// which is `STANCE_UPRIGHT`: a tree, a billboard, a static whose art named no
// side. `light::lit_plane`, and the same absence.
fn lit_plane(stance: u32) -> vec2<f32> {
    switch stance {
        // A lid looks up, and the plane it looks out of is its box's `hi` corner
        // — the surface the art drew, with the floor's own invented depth
        // hanging below it.
        case STANCE_FLAT: { return vec2<f32>(2.0, 1.0); }
        case STANCE_FACE_NORTH: { return vec2<f32>(1.0, -1.0); }
        case STANCE_FACE_SOUTH: { return vec2<f32>(1.0, 1.0); }
        case STANCE_FACE_WEST: { return vec2<f32>(0.0, -1.0); }
        case STANCE_FACE_EAST: { return vec2<f32>(0.0, 1.0); }
        default: { return vec2<f32>(-1.0, 0.0); }
    }
}

// `on_the_lit_surface` is the other half of this and lives below `solid_at`,
// which it reads.

// **`starting_cell` stood here, and `docs/occluders.md`'s S4 deleted it** — with
// the `tile` parameter of `walk`, `arrival` and `sunlight`, which carried the
// place attachment's own tile down three functions for this one reader. A walk
// seeds itself from `floor(start.xy)` now, in the one line that needs it.
// `light.rs`'s grave note above `light::dda_walk` has the census that licensed
// the cut and the argument for why the tie it used to break has one answer.

// **`same_run` stood here, and `docs/occluders.md`'s S4 deleted it**, together
// with the `on_surface` height gate that was inside it and had no other reader.
// `light::same_run`'s own grave note has the argument in full: it was
// `on_the_lit_surface` said as arithmetic over cells, and what kept it alive was
// three fixtures asking the walk about a fragment that named no solid, so the
// rule that reads the fragment's own box could never fire.

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

// One primitive of the list: its own two corners, its flags and its opacity.
//
// An array index and no folding at all, where this used to be a `textureLoad`
// into a row `LIST_ROW` wide — the whole of what `docs/occluders.md`'s D8 buys
// on the read side. **The box is here and nowhere else**: there is no second
// plane to fetch for a height and none for a footprint, so a solid's geometry
// arrives in one read and cannot be half-read.
fn solid_at(id: u32) -> Primitive {
    return primitives[id];
}

// **`aperture_at` stood here**, and `docs/occluders.md`'s S6 deleted it with the
// folding it was for: the holes were an `Rgba8Uint` plane `LIST_ROW` wide, so a
// `SolidId` had to be taken apart into a row and a column and the four numbers
// put back together from channels. They are a storage buffer now, indexed by the
// same `SolidId` as the primitives, so `apertures[id]` is the whole of it — the
// same thing D8 did for the box at S1, arriving at the last plane a solid had.

// **`own_solid` stood here**, and what replaced it is a channel rather than a
// function: `statics.wgsl` writes the name of the box its view ray met into the
// position plane's fourth word, and this pass reads it. `solid_format.wesl`
// carries the argument in full; the short of it is that the scan was exact for
// every static that stands **one** shape per owner — a wall's panel, a floor's
// lid, a body's tile, `Builder::add` pushes never two of a kind — and ambiguous
// for the one that does not. A fitted climbable stands one box per *tread*, all
// of them `Edges::ANY` under one `Owner`, so `(owner, shape)` named a set and
// the loop returned whichever member the cell's list held first: every pixel of
// a flight claimed to be a point of its bottom step.
//
// That cost nothing while `docs/gbuffer.md` step 4c's mesh pass drew over every
// climbable static, because its row named one primitive outright and this
// function was never asked. `docs/lighting_rebuild.md` phase 6d took the pass
// off real statics and the ambiguity came out from under it as triangular
// wedges of self-shadow down every staircase in the world.
//
// Two things the channel is better at besides. A **corner** static's two panels
// were told apart here by the resolved stance and are told apart now by the box
// the ray actually met. And the scan was a loop over a cell's whole reference
// list, once a fragment, which is gone from the pass entirely.

// **`docs/occluders.md`'s D2**: whether a candidate box is part of the very
// surface the lit fragment is a point of, and so must not shadow it.
//
// `light::on_the_lit_surface` states the theorem at length and this is its twin,
// one rule in two spellings with no compiler between them. In short: a candidate
// whose extent along the fragment's own normal axis *ends* on the fragment's own
// plane lies wholly in the closed half-space behind that plane, so a ray leaving
// the surface has nothing there to cross — the discarded set is empty of real
// crossings, which is what makes this a theorem and not a bias.
//
// The plane comes from the box the fragment is a point of (`mine`) and not from
// its interpolated position: the two are the same number — measured, 39,930
// fragments, `traced.rs`'s `a_face_fragments_own_plane_is_the_primitives_own_
// number` — and reading it from the list puts both sides of the comparison in one
// precision, so the equality is exact by construction.
//
// `delta` is the ray, and it is here because the theorem's precondition is
// `d·N >= 0`: the ray has to be *leaving* the plane. A flame behind the surface
// sends the ray into the half-space the candidate really occupies, and the
// reference tracer reads this walk's answer with no cosine to hide it — 4,017
// interior pixels of `line_scene`, the hour this rule went in without the sign
// test. `light::on_the_lit_surface` has the measurement.
fn on_the_lit_surface(stance: u32, mine: u32, lo: vec3<f32>, hi: vec3<f32>, delta: vec3<f32>) -> bool {
    if mine == SOLID_NOBODY {
        return false;
    }
    let plane = lit_plane(stance);
    if plane.x < 0.0 {
        return false;
    }
    let axis = u32(plane.x);
    let own = solid_at(mine);
    // `>= 0` and not `> 0`: a ray lying in the plane is the graze this exists for.
    if delta[axis] * plane.y < 0.0 {
        return false;
    }
    if plane.y > 0.0 {
        return hi[axis] == own.hi[axis];
    }
    return lo[axis] == own.lo[axis];
}

// One tile's surfaces as one box: the union of the spans, the largest opacity
// and the union of the sides, with `edges` of zero for a tile that stands
// nothing.
//
// `Occlusion::at`'s merged view, and for the same readers — a picture of the
// grid rather than a step of the walk. The walk stopped asking it when the list
// arrived, so nothing in the hot loop pays for this.
//
// The span is the primitive's own `lo.z`/`hi.z` and not a rounded copy of it.
// Its one reader is a *diagnostic* picture, which is exactly where phase 1 was
// bitten: `plan.rs`'s elevation view kept its own copy of a format, rounded the
// height, and drew the staircase the plan was about into the instrument meant to
// show it.
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
    let first = solid_at(id_at(run.x).x);
    var merged = MergedCell(first.lo.z, first.hi.z, first.opacity, first.flags);
    for (var i = 1u; i < run.y; i = i + 1u) {
        let stands = solid_at(id_at(run.x + i).x);
        merged = MergedCell(
            min(merged.low, stands.lo.z),
            max(merged.high, stands.hi.z),
            max(merged.opacity, stands.opacity),
            merged.edges | stands.flags,
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
// **One walk for both the flame and the sun, and now there is nothing left to
// tell them apart but their ends.** The `spread` that stood here was how big the
// source is, and it drew that source's penumbra analytically; a sunbeam passed
// `0` and got the hard edge a point casts. Phase 5 casts eight rays at a flame's
// own sphere instead, so a flame's rays are a sun's rays and the size is in where
// they end. The `skip_last` beside it — the flame's own tile, excused because a
// sconce stands *on* a wall — went at phase 4, which moves such a flame clear of
// that wall (`light::mounted_at`) rather than excusing it.
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

// **A box is no longer built here at all.** `box_of` stood in this place and
// reconstructed one from `(cell, bottom, top, footprint)` — a tile plus a
// fraction of it across and sixteen bits an end up — because that is all the
// wire carried. `docs/occluders.md`'s D1 took the reconstruction away with the
// quantisation: `solid_at(id).lo`/`.hi` are the primitive's own corners in
// absolute world coordinates, so what used to be a function is now two fields.
//
// **`over_footprint` stood here and went with the no-horizontal-run shortcut in
// `walk`** — `docs/occluders.md`'s S4, and `light.rs`'s own grave note where the
// Rust twin was states the measurement. It was `ray_vs_solid`'s parallel-axis
// branch on the two horizontal axes alone, spelled again because that shortcut
// could not call `ray_vs_solid` outright while a vertical ray's height answer
// was `crosses`'s *soft* one. Phase 5 made `crosses` a hard crossing test, so
// the two halves became the whole function and the split bought nothing.
//
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

// How much **one primitive** stops `walk`'s ray: its own exact `ray_vs_solid`
// interval and the per-surface rule its shape asks for, `light::walk_primitives`'
// own closure restated as a function, since WGSL has none.
//
// **A primitive and no longer a cell** — `docs/occluders.md`'s S5. What stood
// here was `cell_stopped`, which looped over a tile's own references and took the
// **largest** of what its surfaces stopped: two panels of one tile are the two
// faces of one corner, and a ray crossing both was said to have gone through one
// thing once. S5 deleted the cell that statement was about, so there is nothing
// left to group by and nothing left to take a maximum over — what crosses a
// segment is a volume, and a segment that crosses two volumes is stopped by both.
// The product is `walk`'s, one factor a primitive. `light::walk_primitives`'s own
// doc comment has the census that licensed it: across the whole suite a second
// solid of one cell stops a ray 1,359 times and every one of those is two opaque
// stoppers, where `max(1, 1)` and `1 - 0·0` are the same number.
//
// **And the walk's own tie-break probe went with the grid.** It was a second
// `cell_stopped` on the neighbour a DDA step does not take, there so a ray
// grazing a corner still met what stood on it; a tree has no tie to break, a node
// being hit or missed by the same slab test a primitive is. Measured before it
// was removed: deleting the probe outright changed no pixel of any test in this
// crate — `docs/occluders.md`'s backlog, which is where that silence is recorded
// as a gap rather than as a licence.
fn primitive_stopped(
    id: u32,
    // Which solid of the grid the lit end is a point of, or `SOLID_NOBODY` for a
    // point of none. `light::LitEnd`'s `solid`, and the whole of the self-shadow
    // rule.
    mine: u32,
    stance: u32,
    start: vec3<f32>,
    finish: vec3<f32>,
    delta: vec3<f32>,
) -> f32 {
    let lit = start;
    let stands = solid_at(id);
    let edges = stands.flags & EDGE_MASK;
    // The solid's own `z` span — the primitive's two corners, which is
    // where every height in this pass comes from. `light::wire_span`.
    let low = stands.lo.z;
    let high = stands.hi.z;
    // And the exact interval this ray crosses the primitive's own box in —
    // `light::ray_vs_solid`. The box is read, not rebuilt: `docs/occluders.md`'s
    // D1.
    let hit = ray_vs_solid(start, finish, stands.lo, stands.hi);
    if !hit.hit {
        return 0.0;
    }
    // **A surface does not shadow itself, and that is one comparison of two
    // names** — `docs/lighting_rebuild.md` phase 4, `light.rs`'s own inline
    // copy of it in `walk_primitives`. Everything that used to stand in for it
    // here is gone: the owner, the `own_cell` gate an owner needed to be
    // meaningful, the `match` on `edges` that asked each shape the one
    // question the wire could answer about it, and `flame_end` — the far end's
    // own height test, which `mounted_at` made unnecessary. `SOLID_NOBODY`
    // matches no primitive, so a fragment that is a point of no occluder is
    // exempt from nothing.
    if mine == id {
        return 0.0;
    }
    // **And a surface does not shadow itself when it is cut into more than one
    // primitive** — `docs/occluders.md`'s D2. The rule above answers for the
    // fragment's own box and could never answer for the box abutting it, which is
    // a different primitive of the same wall, floor or landing. `same_run` stood
    // in for it and only for a *panel* on the same row or column; a **body** got
    // no exemption at all, which is the seam this cures — and this rule having
    // cured it is why S4 could delete that one outright.
    if on_the_lit_surface(stance, mine, stands.lo, stands.hi, delta) {
        return 0.0;
    }
    // **A ray that only touches a solid at the point it starts from has not
    // gone through it** — `light::walk_primitives`'s own copy states the
    // argument at length, and it is `crosses`'s strictness said about a box
    // instead of a plane. No tolerance: the interval is exactly `0..0`.
    //
    // The case is a tread's outer corner. A riser is a plane on the climb
    // axis and a tread's lid stops exactly at it, so a fragment on that lip
    // stands in the riser's own plane at exactly the riser's top, and every
    // ray it sends touches that box at `t = 0` and nowhere else. Identity
    // cannot excuse it — the riser is genuinely a different primitive.
    if hit.entered == 0.0 && hit.leaves == 0.0 {
        return 0.0;
    }
    // Where along the ray this crossing happens, and therefore where in the
    // world — the exact crossing's own midpoint. A panel needs the *point*, to
    // ask whether the ray went through a window.
    let middle = (hit.entered + hit.leaves) * 0.5;
    let opacity = f32(stands.opacity) / 255.0;
    if edges == 0u || edges == EDGE_MASK {
        // A **body** — a whole tile that stands up and whose art would
        // not say which way, **or a lid**: a floor, a rug, a plank, which is a
        // body a `LID_THICKNESS` deep and no longer a rule of its own.
        // `ray_vs_solid` is an exact slab test, so a hit already means the
        // segment genuinely passed through the box over
        // `hit.entered..hit.leaves`: occlusion is the primitive's own opacity
        // outright. `light::walk_primitives`'s own mirror; see
        // `docs/lighting_raymarch.md`'s "hard shadows" decision for why the
        // length-and-pierce grading this used to do (and the widened-corner
        // graze feeding it) are both gone.
        //
        // **A lid took `crosses(lit.z, lit.z + delta.z, low, high)` here**, and
        // `docs/parity.md`'s P4 step 1 is what retired it: a lid was a plane in
        // `z`, so a hit said only that the ray touched it, and at a corner of the
        // footprint it said so at one `t` where both ends of the `z` interval
        // were one number — a floor leaking one bright point per corner of the
        // grid, reported at `(1492, 1642)`, `z 28`. `Solid::box_of` gives a lid a
        // span of its own now, and the strictness `crosses` was protecting
        // — a candle standing on the floor it lights, both ends of every ray on
        // that floor's own plane — is what the geometry says by itself: those
        // rays leave the top face and never enter the box beneath it.
        return opacity;
    }
    // A **panel** — a wall standing on one side of its tile. A
    // *surface*, and what a surface does to a ray is decided where
    // the ray pierces it: one sample, at the exact crossing's own
    // midpoint. There was an `edges & ~same_run` gate here, and
    // `docs/occluders.md`'s S4 is where it went — the run it excused is
    // `on_the_lit_surface`'s plane, tested above with the other
    // exemptions.
    return opacity * pierced(stands, id, start + delta * middle);
}

// **`ahead` is gone from this signature and from `sunlight`'s.** It was the lit
// end's own outward normal, and the only thing it was ever spent on was
// `STAND_OFF`; with the bias at zero there is nothing here that asks which way a
// surface looks. `docs/lighting_rebuild.md` phase 4.
fn walk(
    start: vec3<f32>,
    finish: vec3<f32>,
    stance: u32,
    // Which solid of the grid the lit end is a point of — `own_solid` for a
    // sprite fragment, `mesh_instances[id].solid` for a mesh face, `SOLID_NOBODY`
    // for the ground and a mobile. Resolved once by the caller, per fragment.
    mine: u32,
) -> f32 {
    // **Both ends stand exactly where they are, and the bias is zero** —
    // `docs/lighting_rebuild.md` phase 4. A pair of nudges off the retired
    // attachment's byte layout stood here; identity is what keeps a ray from
    // being stopped by the surface it left, and it does not need the ray moved.
    let delta = finish - start;

    // **A stackless traversal of the tree, and the DDA over the tile grid is
    // gone** — `docs/occluders.md`'s S5, `light::candidates`' own mirror. What
    // stood here stepped from cell to cell along the nearer of its two axis
    // boundaries, probed the neighbour it did not step into, and asked each cell
    // for the largest of what its surfaces stopped. Three of this file's own
    // defects went with it: a cell listed a primitive once (so a box wider than
    // its registration tile was invisible to a ray crossing only the overhang),
    // listing one from two cells would have double-counted it, and a `floor()`
    // decided which cell a point on a boundary was in.
    //
    // **Where a traversal ends is the root's own escape**, not the length of the
    // buffer: `blit.rs` grows these buffers and never shrinks them, so what is
    // past this frame's tree is last frame's and must not be walked. A frame with
    // no occluder uploads one node of zeros, whose escape is zero — so the loop
    // below runs no iterations at all and needs no case for an empty world.
    let end = bvh_nodes[0].escape;
    var through = 1.0;
    var at = 0u;
    while at < end {
        let node = bvh_nodes[at];
        // The same slab test a primitive gets, against a box built out of the
        // same `f32` corners — `bvh::Node::space` has why a node whose corners
        // were the record's `f64` could round inward of a primitive's and lose
        // it.
        let hit = ray_vs_solid(start, finish, node.lo, node.hi);
        var next = node.escape;
        if hit.hit {
            let count = node.leaf & 7u;
            let first = node.leaf >> 3u;
            for (var k = 0u; k < count; k = k + 1u) {
                let stopped = primitive_stopped(
                    bvh_order[first + k], mine, stance, start, finish, delta,
                );
                through = through * (1.0 - stopped);
                if through <= 0.004 {
                    // Under a byte's worth of light: nothing further can matter,
                    // and a wall is the common case. The value is the same either
                    // way — a product that has fallen under the cutoff cannot
                    // climb back out of it — so this is a cost and not a rule.
                    // `light::walk_primitives` applies the same cutoff after its
                    // traversal instead, because it also has a *stopper* to name
                    // and that must not depend on the order leaves come back in.
                    return 0.0;
                }
            }
            // A hit steps to this node's own first child, which is the very next
            // node. A leaf's escape *is* `at + 1`, so the two agree there and
            // there is no case for it.
            next = at + 1u;
        }
        // **The loop's own bound, and there is no constant in it**: a well-formed
        // tree always moves forward, so this visits each node at most once and
        // cannot run longer than the frame's own node count. `MAX_WALK_STEPS`
        // stood in the DDA's loop for exactly the opposite reason — a cell count
        // is the *ray's* length, which a widened radius grows. A tree that does
        // not move forward is malformed, which nothing on this side writes, and
        // stopping is the one thing to do with it that is neither a hang nor a
        // number somebody has to size.
        if next <= at {
            break;
        }
        at = next;
    }
    return through;
}

// A number in `0.0..1.0` that belongs to a point of the world and to no other:
// the rotation `shadow` turns its sample pattern by.
//
// **World space and integers, so that both backends produce the same number.**
// The obvious thing — a hash of `texel` — cannot be spelled at all on the CPU
// side, which has no pixel; and the float hash a shader usually carries
// (`fract(sin(dot(…)))`) is exactly the arithmetic two backends are least likely
// to agree about. This quantises the position to a hundred-and-twenty-eighth of
// a tile, which is under a screen pixel at 1:1, and mixes the three integers with
// a plain bit-avalanche. Being stable *in the world* rather than on the screen is
// worth having on its own: a panning camera does not make a penumbra crawl.
//
// `bitcast` and not `u32(…)` for the sign: the two are the same by the spec's own
// modulo-2³² rule, and one of them says so. `light::dither`.
fn dither(at: vec3<f32>) -> f32 {
    let quantised = vec3<i32>(floor(at * 128.0));
    var hash = (bitcast<u32>(quantised.x) * 0x8DA6B343u)
        ^ (bitcast<u32>(quantised.y) * 0xD8163841u)
        ^ (bitcast<u32>(quantised.z) * 0xCB1AB31Fu);
    hash = hash ^ (hash >> 16u);
    hash = hash * 0x7FEB352Du;
    hash = hash ^ (hash >> 15u);
    hash = hash * 0x846CA68Bu;
    hash = hash ^ (hash >> 16u);
    // The top 24 bits over 2²⁴: a float's own mantissa, so every value this can
    // produce is representable and none of them is 1.
    return f32(hash >> 8u) / 16777216.0;
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

// What one flame's body sends to one fragment, and how much of that body the
// fragment can see — `light::Arrival`, and the two are one pair of answers.
struct Arrival {
    // Λ: the mean over the flame's sample points of
    // `visibility × max(N · L, 0) × falloff² × beam`. Everything the flame's
    // colour and intensity are multiplied by, in one number.
    delivered: f32,
    // The share of the flame's body this fragment can see, with no cosine, no
    // falloff and no beam in it. `VIEW_SHADOW` is this and nothing else.
    visible: f32,
}

// What the flame at `to` delivers to the fragment at `at`, and how much of it
// the fragment sees: `SHADOW_RAYS` rays at the flame's sphere, with **every term
// of the sum taken at the sample point**.
//
// **This is the whole of the soft shadow** — `docs/lighting_rebuild.md` phase 5 —
// **and the whole of the shading of a flame besides**, which is its phase 5b. A
// shadow's edge is a gradient because a fragment in it can see part of the flame
// and not the rest, which is what a penumbra *is*; nothing here softens
// anything, and every one of the rays is the hard, exact walk the sun's ray is.
//
// The points are a Vogel spiral on the disc the sphere presents to this fragment
// — the silhouette, not the surface, which is the same choice
// `pathtrace::Emitter::Sphere` makes and for the same reason: what a receiver can
// be occluded from is the disc it sees. `sqrt` of the index spaces them by equal
// *area* rather than equal radius, so the middle of the flame is not sampled
// eight times over. The disc is laid out in **tile space**, `z` divided by
// `Z_PER_TILE` and multiplied back on the way out — the metric the sphere is
// round in, and the one the falloff and the cosine are stated in.
//
// **The sample point is the only place this flame has a position.** The cosine,
// the falloff and the beam are functions of it, and `to` appears below only as
// the centre the disc is laid around. A flame that had a body for visibility and
// a point for the cosine drew a *wedge* of shadow at every join: a lamp lower
// than its own radius above a floor puts half its sphere below that floor's
// plane, those rays leave the fragment's own primitive near a join and come back
// blocked, and one centre cosine cannot cancel them. `max(N · L_p, 0)` does,
// exactly, because the set of rays a join blocks and the set below the horizon
// are the same set.
//
// **A sample whose cosine is not positive is not walked**, unless `every_sample`
// asks for it. That is an exact skip and not a tolerance — such a sample
// contributes zero to `delivered` whatever stands in its way — and it is up to
// half the rays in the grazing configurations that cost the most. What
// `every_sample` is for is `visible`: a debug view reads it as a complete answer
// and pays the rays for one, while the lit path spends nothing on a number it
// does not use. `delivered` is the same either way, to the bit.
//
// `light::arrival`, and the two are one arrangement.
fn arrival(
    at: vec3<f32>,
    to: vec3<f32>,
    stance: u32,
    mine: u32,
    normal: vec3<f32>,
    beam: vec4<f32>,
    reach: f32,
    every_sample: bool,
) -> Arrival {
    let toward = vec3<f32>(to.x - at.x, to.y - at.y, (to.z - at.z) / Z_PER_TILE);
    let span = length(toward);
    let body_radius = flame_radius();
    let point_source = body_radius <= 0.0;
    var across = vec3<f32>(1.0, 0.0, 0.0);
    var up = vec3<f32>(0.0, 1.0, 0.0);
    if span >= 1.0e-6 && !point_source {
        let axis = toward / span;
        // Two directions across the ray, built the textbook branching way: away
        // from whichever axis the ray is most nearly along, so the cross product
        // is never near zero. Any consistent pair does — the pattern is rotated
        // per fragment anyway — and what matters is that `light::arrival` builds
        // the same one.
        let aside = select(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), abs(axis.x) > 0.9);
        across = normalize(cross(aside, axis));
        up = cross(axis, across);
    }
    var phase = 0.0;
    if !point_source {
        phase = dither(at) * TAU;
    }
    // PERFORMANCE REGRESSION GUARD: a zero-radius flame has no body to sample;
    // all requested Vogel points are exactly `to`. With the live close-zoom
    // repro (`flame_radius = 0`, `shadow_rays = 32`, `reach = 4`) walking those
    // 32 identical BVH segments cost 27–30 ms for one carried light. One unique
    // ray draws the same point source and measured below 1 ms. The CPU twin is
    // `ShadowRays::for_radius`, pinned by
    // `a_point_source_walks_the_bvh_once`.
    let rays = select(shadow_rays(), 1u, point_source);
    var result = Arrival(0.0, 0.0);
    for (var ray = 0u; ray < rays; ray = ray + 1u) {
        var point = to;
        if span >= 1.0e-6 && !point_source {
            // The fragment is inside the flame when this is false. Every point of
            // the sphere is then as good as any other and none of them has a
            // direction, so every ray is the ray to the centre — the one with no
            // length.
            let angle = phase + GOLDEN_ANGLE * f32(ray);
            let radius = body_radius * sqrt((f32(ray) + 0.5) / f32(rays));
            let offset = (across * cos(angle) + up * sin(angle)) * radius;
            point = vec3<f32>(to.x + offset.x, to.y + offset.y, to.z + offset.z * Z_PER_TILE);
        }
        let along = vec3<f32>(point.x - at.x, point.y - at.y, (point.z - at.z) / Z_PER_TILE);
        // The zero vector means "no facing at all" — a billboard, the ground
        // before phase 3 — and it is a value a producer writes on purpose rather
        // than an absence. Such a surface is lit from every side.
        var cosine = 1.0;
        if any(normal != vec3<f32>(0.0)) {
            cosine = lit_from(normal, along);
        }
        if cosine <= 0.0 && !every_sample {
            continue;
        }
        let through = walk(at, point, stance, mine);
        result.visible = result.visible + through;
        if cosine <= 0.0 {
            continue;
        }
        // The falloff of *this* ray, clamped where the centre's never had to be:
        // the cull is conservative, so a fragment inside the pool by the near
        // edge of the sphere can be past the reach of its far side, and
        // `(1 - d)²` of a negative `1 - d` would be a bright ring at the rim.
        let fall = max(1.0 - length(along) / reach, 0.0);
        // And which way the flame is pointing at this point of itself. `along`
        // runs from the fragment to the flame and a beam's axis runs the other
        // way, so the sign flips here.
        var lit_by = 1.0;
        if beam.w > -1.0 {
            lit_by = cone(beam, -along);
        }
        result.delivered = result.delivered + through * cosine * fall * fall * lit_by;
    }
    result.delivered = result.delivered / f32(rays);
    result.visible = result.visible / f32(rays);
    return result;
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
fn sunlight(at: vec3<f32>, stance: u32, mine: u32) -> f32 {
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
    return walk(at, at + step * tiles, stance, mine);
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
    // Which solid of the grid the fragment is a point of — `SOLID_NOBODY` for a
    // point of none. See `VIEW_SOLID`.
    mine: u32,
    // Which of the two masks ended this fragment's outline, as the id word's own
    // two top bits carry it — `place_format.wesl`'s `ids_edges`. See the pair of
    // silhouette views below.
    edges: u32,
    kind: u32,
    tile: vec2<f32>,
    at: vec3<f32>,
    normal: vec3<f32>,
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
    if view == VIEW_NORMAL {
        // The G-buffer's normal plane, drawn. Each axis is a channel, mapped
        // from `-1..1` into `0..1` the way every normal map is read — so a
        // south-facing wall is green, an east-facing one red, and anything
        // lying flat is blue.
        //
        // A surface with **no** facing is the zero vector, which lands on the
        // grey in the middle of all three — and that is the shade to look for:
        // it is the honest one for a mobile and for a tree, and phases 6 and 7
        // are the work of leaving less of it in a frame. Its being a *neutral*
        // grey rather than a colour is the point: nothing about it reads as a
        // direction, because it is not one.
        return normal * 0.5 + vec3<f32>(0.5);
    }
    if view == VIEW_NORMAL_GEOMETRY || view == VIEW_NORMAL_SPRITES {
        // `VIEW_NORMAL` cut in two by **where the vector came from**, because one
        // picture of it cannot answer that: a fragment carries one normal and the
        // plane draws every one of them the same way, so a measured box face and a
        // billboard's plane are two colours with nothing to say which is which.
        //
        // Four producers write the plane and they fall in two groups:
        //
        // - *measured* — the land patch's own normal (`ground.wesl`), a mesh
        //   face's (`mesh_face.wesl`), and the face of the box a static's view ray
        //   actually met (`statics.wesl` through `package::impostor`). Each is a
        //   statement about a shape that exists, and each is a **static or the
        //   ground** by construction: no mobile is ever met against anything.
        // - *the picture's* — a mobile's billboard plane, and a static the grid
        //   holds no volume for, whose zero vector says only that nothing was
        //   measured. Neither is geometry; both are a rectangle of art.
        //
        // A fragment is one or the other and never both — the G-buffer keeps one
        // answer per pixel — so a sprite drawn over geometry is a *hole* in the
        // geometry layer with the same shape filled in the sprite layer. That is
        // the whole of what these two views are for: a fringe along a silhouette
        // is either in one picture or the other, and which one it is names the
        // producer to go and look at.
        //
        // What is not in the layer is **black**, and black is not a normal: it
        // would need `(-1, -1, -1)`, whose length is `sqrt(3)`, so no unit vector
        // maps onto it and nothing here reads as a direction it is not.
        //
        // **The mark is the vector itself and not the solid it names.** The first
        // rule written here was `mine != SOLID_NOBODY`, and a picture of it is how
        // it was caught: a static may meet a real box that the grid holds no
        // *solid* for — a floor's own volume stands with `edges 0b0000` and stops
        // no light — so a measured face read as a sprite. What separates the two
        // groups without a second record is `statics.wesl`'s own invariant: the
        // one branch there that leaves the vector zero is the static the grid
        // holds no volume for at all, and a fragment that met a box or came off
        // the mesh pass always carries a face's cardinal. So for a static a
        // non-zero normal *is* the mark of a measured surface — and a producer
        // that ever gives an unmeasured static a facing has to be counted here.
        //
        // **That branch narrowed and the invariant held**: a fragment whose ray
        // misses every box is answered by the box it came nearest now rather
        // than left at the tile's centre, so the zero vector means a static with
        // *no boxes at all* and nothing else. Measured on a client dump before
        // and after — 2.07% of one frame carried a zero normal, and none does.
        let measured = kind == KIND_LAND || (kind == KIND_STATIC && any(normal != vec3<f32>(0.0)));
        if measured == (view == VIEW_NORMAL_GEOMETRY) {
            return normal * 0.5 + vec3<f32>(0.5);
        }
        return vec3<f32>(0.0);
    }
    if view == VIEW_SILHOUETTE_ART || view == VIEW_SILHOUETTE_BOX {
        // **The two edges a magnified frame draws, each in its own picture** —
        // `docs/silhouettes.md`, and the pair is one instrument rather than two
        // views that happen to sit next to each other.
        //
        // The art's alpha steps once a *texel*, which is `Projection::scale` real
        // pixels square and cannot be finer; the impostor box's rim is decided per
        // *fragment*, so it steps once a real pixel. Neither is a defect. What a
        // person sees is the two of them adjacent — a crisp line against a
        // four-pixel stair on the same wall — and until these two views there was
        // no picture that said which line was which.
        //
        // **They are not two halves of one outline**, and this pair is how that
        // was established rather than assumed. Since a box miss stopped being
        // discarded, the picture's silhouette is entirely the art's; the box bit
        // marks a seam *inside* it, between the part of a sprite something was
        // measured for and the part carrying on unmeasured. Both are still lines
        // on the screen at two resolutions, which is why both are still drawn.
        //
        // **White is both**, and it is the same white in both views on purpose:
        // the seam reaching the outline is where the two resolutions genuinely
        // meet along one line, and a rule that folded such a fragment into one
        // layer would delete the subject to satisfy a partition.
        //
        // The other mask is drawn dim rather than black. A line is read by
        // comparing it against the one beside it, and a layer that blacked the
        // other one out would leave a person matching two pictures by eye across
        // a keypress. Dim enough that nothing in it can be mistaken for this
        // layer's own answer, present enough to locate it.
        //
        // Only `statics.wesl` stamps these bits, so the ground and a mesh face
        // read as interior — which is honest: a land patch has no silhouette of
        // its own, and a mesh face's whole outline is geometry. See
        // `place_format.wesl`.
        let art = (edges & IDS_EDGE_ART) != 0u;
        let box_ = (edges & IDS_EDGE_BOX) != 0u;
        if art && box_ {
            return vec3<f32>(1.0);
        }
        let mine_ = select(box_, art, view == VIEW_SILHOUETTE_ART);
        if mine_ {
            // Orange for the art's own quantum, blue for the fragment grid's, so
            // that a person flicking between the two views is comparing two
            // colours and not two greys.
            return select(vec3<f32>(0.25, 0.55, 1.00), vec3<f32>(1.00, 0.35, 0.10), view == VIEW_SILHOUETTE_ART);
        }
        if art || box_ {
            return vec3<f32>(0.16, 0.16, 0.20);
        }
        // Drawn, and on neither outline: the inside of a sprite, the ground, a
        // mesh face. Dark rather than black so that the frame's own extent is
        // still visible under the two outlines — a silhouette with nothing behind
        // it is a shape nobody can place.
        return vec3<f32>(0.05, 0.05, 0.06);
    }
    if view == VIEW_SOLID {
        // Which primitive this pixel is a point of, as a colour — see
        // `debug::View::Solid` for why this plane deserves a picture. Black for
        // a point of none, which is the honest answer for the ground, for a
        // mobile and for a static the grid holds no shape for, and it has to be
        // told apart from every id: hence the `+ 1` inside the moduli, so no
        // real id lands on black.
        if mine == SOLID_NOBODY {
            return vec3<f32>(0.0);
        }
        let n = f32(mine + 1u);
        // Three primes, so that ids one apart are far apart in all three
        // channels: the only question a picture of an identity answers is
        // *same or different*, and neighbouring primitives are exactly the
        // pair a seam is made of.
        return vec3<f32>(fract(n / 7.0), fract(n / 13.0), fract(n / 29.0)) * 0.85 + vec3<f32>(0.15);
    }
    if view == VIEW_PLACE {
        // A checkerboard of tiles with the sub-tile fraction laid over it: the
        // squares say which tile a pixel claims, and the gradient inside each
        // one says the fraction is being written at all. A wall reads as one
        // flat square with its sprite standing out of it — which is exactly the
        // property the lighting depends on.
        //
        // The tile itself, off the row the fragment's id names — which is what
        // this view always claimed to draw and did not: the checker used to be
        // taken from the two halves of the *id*, so a frame's squares counted
        // instance rows rather than tiles. It went unnoticed because a
        // diagnostic is read for whether a gradient is *there*, and both
        // versions have one.
        let checker = f32((u32(tile.x) + u32(tile.y)) & 1u) * 0.35 + 0.15;
        return vec3<f32>(sub.x, sub.y, checker);
    }
    if view == VIEW_KIND {
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

    // What drew this pixel. `textureLoad` and not a sampler, and the texel is
    // the nearest one whatever the zoom: an id is an identity, and half of one
    // is nobody.
    let size = vec2<f32>(textureDimensions(ids_of));
    let texel = vec2<i32>(clamp(in.uv * size, vec2<f32>(0.0), size - vec2<f32>(1.0)));
    let ids = textureLoad(ids_of, texel, 0).x;

    let view = lighting.view.x;

    // Nothing was drawn here — the cleared background, or the letters over a
    // speaker's head, which are a message and not a thing standing in the
    // street. Neither is lit and neither is dimmed. See `crate::place::Kind`.
    //
    // The diagnostics leave it alone too, and deliberately: a view that painted
    // the background would make the world's own silhouette hard to find, and
    // every one of these is read by comparing a shape against the picture it
    // came from.
    //
    // **Five exceptions, and every one of them is a view whose subject is which
    // pixels are which.** The kind view's whole content is that question. And the
    // two normal layers make a claim a passed-through pixel breaks: each says it
    // holds *its own category and nothing else*, so a speaker's letters left
    // standing over the geometry layer are not a landmark, they are the layer
    // being wrong about itself. This was reported from a client dump — the text
    // over a head, drawn in a picture that had promised to be measured surfaces.
    //
    // The two silhouette layers are the same claim about a different partition:
    // they draw the *inside* of a drawn thing as a near-black of their own, and a
    // passed-through background pixel that kept the world image would be a shade
    // nothing in the layer's own vocabulary spells.
    if ids_kind(ids) == KIND_NOTHING {
        if view == VIEW_KIND
            || view == VIEW_NORMAL_GEOMETRY
            || view == VIEW_NORMAL_SPRITES
            || view == VIEW_SILHOUETTE_ART
            || view == VIEW_SILHOUETTE_BOX
        {
            return with_composite_opacity(vec4<f32>(0.0, 0.0, 0.0, color.a));
        }
        return with_composite_opacity(color);
    }

    // `Lighting::NONE` still has to run the colour curve, but it has no reason
    // to find a tile, fetch an instance row, inspect the sky field, or touch an
    // occluder. In a dense house those data-dependent reads cover most of the
    // screen even when there is not one flame; this uniform branch keeps that
    // case to the one world sample and one ID load it actually needs. It comes
    // after the clear-background exception above so that text and untouched
    // pixels retain the exact behaviour of the full path.
    if lighting.view.w == 1u {
        return with_composite_opacity(vec4<f32>(shade(color.rgb, vec3<f32>(1.0)), color.a));
    }
    if lighting.view.w == 2u {
        return with_composite_opacity(vec4<f32>(shade(color.rgb, lighting.ground.rgb), color.a));
    }

    // Which *tile* this pixel's picture came from is still an id into a
    // per-kind instance buffer — `docs/gbuffer.md` step 3 for a static or a
    // mobile, step 7 for the ground — because the tile is what the shadow walk
    // starts its cell stepping from, and a whole number a pass knew exactly is
    // not something to recover by flooring anything.
    let kind = ids_kind(ids);
    // Read before the id lookup below, not after: a mesh face's sentinel
    // (`STANCE_MESH_FACE`) decides *which* buffer the id addresses, and the
    // real stance a mesh face's normal names lives in that buffer's own row,
    // not in this word — see the branch below and
    // `crate::place::Stance::MeshFace`'s own doc.
    var stance = ids_stance(ids);
    let id = ids_id(ids);
    var tile = vec2<f32>(0.0);
    // Where the fragment itself is, and **which solid of the grid it is a point
    // of** — one read, because the second rides in the first's fourth channel
    // (`solid_format.wesl`). The position used to be `tile + fraction` with the
    // fraction taken out of the place attachment's fourth channel in
    // hundred-and-twenty-sevenths, and a height taken out of its third as eight
    // whole units plus four bits of sixteenths. Every one of the three was exact
    // in the pass that computed it; `docs/lighting_rebuild.md` phase 2 is the
    // decision to carry the number instead of the fields, and its second root —
    // "a fragment's own position is not exactly known, so a shadow ray must
    // start away from where it really is" — is the bill that packing ran up.
    let place = textureLoad(position_of, texel, 0);
    let at = place.xyz;
    // The solid is the one number the shadow walk compares
    // (`docs/lighting_rebuild.md` phase 4), and every producer states it: the
    // impostor's own box for a static, `SOLID_NOBODY` for the ground, for a
    // mobile and for a static this frame's grid holds nothing for. Only a mesh
    // face overrides it below, out of its own row.
    var mine = unpack_solid(place.w);
    if (id & IDS_COMPOSITE_MAP) != 0u {
        // Cached map pixels retain an exact world point, but no frame-local
        // row.  Position is the source of truth for their tile on every later
        // camera frame.
        tile = floor(at.xy);
    } else if kind == KIND_LAND {
        tile = vec2<f32>(
            f32(ground_instances[id].place0 & 0xFFFFu),
            f32(ground_instances[id].place0 >> 16u),
        );
    } else if kind == KIND_STATIC && stance == STANCE_MESH_FACE {
        // `docs/gbuffer.md` step 4c: a mesh face has no picture of its
        // own, so its row lives in `mesh_instances`, not
        // `face_instances` — and carries the *real* stance the
        // attachment's sentinel stood in for.
        //
        // And its solid outright: a mesh face is one primitive by construction,
        // so the pass that drew it already knew which — as a *row*, which is why
        // that pass writes `SOLID_NONE` in the channel every other one states
        // its answer in. See `MeshFaceInstance` and `mesh_face.wesl`.
        let row = mesh_instances[id];
        tile = vec2<f32>(f32(row.tile & 0xFFFFu), f32(row.tile >> 16u));
        stance = row.stance;
        mine = row.solid;
    } else {
        var row_place: vec2<u32>;
        if kind == KIND_STATIC {
            let row = id & ~(IDS_DYNAMIC_ITEM | IDS_COMPOSITE_MAP);
            row_place = select(face_instances[row].place, item_instances[row].place, (id & IDS_DYNAMIC_ITEM) != 0u);
        } else {
            row_place = mobile_instances[id].place;
        }
        tile = vec2<f32>(f32(row_place.x & 0xFFFFu), f32(row_place.x >> 16u));
    }
    // Where in its tile that leaves it. Nothing computes the fraction any more;
    // this is the difference between two things already known, and only the
    // diagnostics below read it.
    let sub = at.xy - tile;
    // And which way the surface drawn here looks — read, like the position, and
    // no longer recovered from four bits of the place attachment. Zero for the
    // ground, for a mobile and for anything standing up whose art would not name
    // an edge: none of those has a side to be lit from, and a zero vector is the
    // value that says so.
    //
    // The distinction between land and a wall's flat cap — both `STANCE_FLAT`,
    // and only one of them wanting the retired half-space gate — used to be made
    // here, by testing the kind. It is made by the *producer* now: `ground.wesl`
    // writes the bilinear patch's own normal, and `statics.wesl` writes the face
    // of the box this fragment's own view ray met (`package::impostor`,
    // `docs/lighting_rebuild.md` phase 6c). Neither is a table any more, which is
    // what retired `place_format.wesl`'s `outward` and the two rows of it that
    // were wrong.
    let normal = unpack_normal(textureLoad(normal_of, texel, 0).x);

    // The ambient this *tile* has: the sky's share of it scaled by how much of
    // the sky the column over the tile can see, plus the floor everything gets.
    // `docs/lighting_world.md`, decision 1, and it is the whole of why a room is
    // darker than the road outside it before anything burns.
    //
    // **The tile the pass carried, not `floor(at.x/y)`** — the same reason
    // `first` below is, and the same class of defect: a surface may lie exactly
    // on a tile boundary, and since `docs/lighting_rebuild.md` phase 6c meets a
    // fragment with its own volume rather than clamping it a
    // hundred-and-twenty-seventh of a tile inside, a south or east wall's face
    // does. Flooring one of those reads the *neighbour* tile's sky, so a street
    // wall would take the room's ambient along one of its two sides and not the
    // other.
    let share = sky_at(i32(tile.x), i32(tile.y));
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
        let reach = max(light.place.w, 0.001);
        let d = length(offset) / reach;
        // **The one thing still asked about the flame's centre, and it is
        // therefore conservative.** Outside the pool entirely, which most
        // fragments are for most flames: no ray is walked and nothing is sampled,
        // which is why the grid walk below is affordable at all. That makes it a
        // *broad phase*, and a broad phase is forbidden to change the answer — so
        // it culls on the **near side** of the sphere, since a fragment the centre
        // says is out of reach can be reached by the near edge of a body that has
        // one. `docs/lighting_rebuild.md` phase 5b.
        if length(offset) - flame_radius() >= reach {
            continue;
        }
        // Eight rays at the flame's own sphere, and what they brought back:
        // visibility, the cosine, the falloff and the beam, each taken at the
        // point its own ray ends on. There is nothing left out here to multiply
        // the result by — a cosine at the flame's *centre* is what drew a wedge of
        // shadow at every join, and `docs/lighting_rebuild.md` phase 5b is that
        // removed by construction rather than compensated for.
        //
        // A debug view asks for every sample, because `VIEW_SHADOW` is visibility
        // and a ray the lit path has no use for is one that view still has to
        // report. `Arrival::delivered` is the same number either way.
        let sent = arrival(
            at, vec3<f32>(to.x, to.y, to.z), stance, mine,
            normal, light.beam, reach, view != VIEW_LIT,
        );
        // Recorded before the delivery is tested, so that a fragment inside a pool
        // and behind a wall is *nearest, stopped* rather than indistinguishable
        // from open ground nothing reaches. That difference is the one the whole
        // shadow view exists to show — and it is `visible` and not `delivered`,
        // because a shadow view that dimmed with the cosine would answer two
        // questions with one grey.
        if d < nearest {
            nearest = d;
            nearest_through = sent.visible;
        }
        if sent.delivered <= 0.0 {
            continue;
        }
        reached = reached + 1u;
        let added = light.color.rgb * (light.color.w * sent.delivered);
        lit = lit + added;
        flames = flames + added;
    }

    // And the sun, which is one direction rather than a place: no distance and no
    // falloff, only whether anything stands between this pixel and the sky.
    var sun_through = 0.0;
    if lighting.sun_color.w > 0.0 {
        sun_through = sunlight(at, stance, mine);
        lit = lit + lighting.sun_color.rgb * (lighting.sun_color.w * sun_through);
    }

    if view != VIEW_LIT {
        return with_composite_opacity(vec4<f32>(
            debug_color(
                view, mine, ids_edges(ids), kind, tile, at, normal, sub, lit, flames, reached, nearest,
                nearest_through, sun_through, share,
            ),
            color.a,
        ));
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
    let shaded = shade(color.rgb, lit);

    // D9's grey: a ghost's own screen loses its colour, not its brightness — the
    // same lit value, collapsed to the luma ClassicUO's own black-and-white
    // effect uses (Rec. 601), so a dark corner is still a dark corner and a
    // torch's pool is still brighter than the street, just colourless. One
    // branch, after everything else this pass computes, because death changes
    // what a pixel is *shown as* and nothing about what lit it.
    if dead() {
        let luma = dot(shaded, vec3<f32>(0.299, 0.587, 0.114));
        return with_composite_opacity(vec4<f32>(luma, luma, luma, color.a));
    }
    return with_composite_opacity(vec4<f32>(shaded, color.a));
}
