// The statics pass: one sprite-sized quad per static, drawn at its own scale.
//
// Simpler than the ground in every way that matters. A static has one picture,
// one position and one height, so there is no shape to choose and nothing to
// stretch: the quad is the sprite's own rectangle in viewport pixels, and a
// texel is a pixel. What it does have, and ground does not, is transparency —
// a sprite is a picture with a shape rather than a diamond with a known one, so
// the fragment shader discards on alpha.
//
// **The picture is the artist's and the geometry is ours.**
// `docs/render/design_model.md` phase 6: where a fragment *is* and which way its
// surface looks are not guessed back from the stance the art was read as — they
// are found by meeting this pixel's own view ray with the boxes the occlusion
// grid holds for **this instance**. See `package::impostor` for the meeting and
// `statics::push_volumes` for the association. So there is one silhouette, the
// sprite's; a pixel is already one instance's, because that instance's own quad
// drew it; and a static whose art overhangs its own volume produces a *number*
// (`Meeting::outside`) rather than a hole for a border constant to cover.
//
// Vertex-buffer instancing, one uniform block, one sampled texture and one
// storage buffer — which the header once said was kept out for WebGL2's ceiling,
// a stale objection: the crate's ceiling is **WebGPU** (see `crate` docs, and
// `docs/archive/render/lighting.md` decision 30.5) and `blit.wgsl` beside this reads eleven of
// them.
//
// The depth comes in per instance and is written whole. It is not derived here
// because it is one ordering shared with the ground pass, and a second copy of
// the formula is a second chance to disagree with it — see `crate::depth`.

import package::place_format::{
    KIND_MASK, KIND_MOBILE, STANCE_FLAT, STANCE_FACE_NORTH, STANCE_FACE_EAST,
    STANCE_FACE_SOUTH, STANCE_CORNER, IDS_EDGE_ART, IDS_EDGE_BOX, pack_ids,
};
import package::normal_format::pack_normal;
import package::solid_format::{SOLID_NOBODY, pack_solid};
import package::impostor::{
    FRINGE_DISCARD, FRINGE_VOLUME, Meeting, billboard_at, billboard_normal, billboard_offset, hit,
    meets, presented_face, ray_from,
};

struct Viewport {
    // The target's size in *real* pixels — see `ground.wgsl`, which documents
    // the same three fields at length; this pass reads them identically and
    // deliberately, because two passes that scaled differently would draw two
    // pictures rather than one wrong one.
    size: vec2<f32>,
    // Real pixels per virtual pixel.
    scale: f32,
    // What a fragment whose ray met no box is answered with — `0` the clamp,
    // `1` not drawn, `2` the volume's presented face. `crate::impostor::Fringe`
    // is the enum, and this slot was the block's padding until it wanted a
    // switch. A whole number carried as an `f32` because the block is floats and
    // three exact small integers survive it.
    fringe: f32,
    // The virtual-pixel point that lands in the middle of the target.
    origin: vec2<f32>,
    // Uniform blocks are sized in multiples of 16 bytes.
    // Reserved G-buffer id bits.  Map rows use zero; server items use the
    // dynamic-item bit so deferred readers address their separate buffer.
    id_bits: f32,
    _tail: f32,
};

@group(0) @binding(0) var<uniform> viewport: Viewport;
@group(0) @binding(1) var atlas_texture: texture_2d<f32>;
@group(0) @binding(2) var atlas_sampler: sampler;
// `hues.mul`'s ramps, `crate::hue::HueRamp::width()` wide by one row a hue —
// see that module for why the index rides in the atlas texture's own red
// channel instead of a second one.
@group(0) @binding(3) var hue_ramp: texture_2d<f32>;
@group(0) @binding(4) var hue_sampler: sampler;

// One box a static stands as — `crate::impostor::Volume`, and the frame's whole
// list of them: every drawn instance names its own run of this by the two words
// at the tail of its row. A fragment is met against **its own** run and no
// other, which is what makes the silhouettes agree by construction rather than
// by a border.
//
// Both spare words are the vectors' own alignment rather than fields anybody
// paid for. `solid` is the frame's own name for this box, and this pass
// **writes it into the G-buffer** — the fragment stage carries the name of
// whichever box its view ray landed on, which is what `blit.wgsl` compares a
// shadow candidate against. It rode here unread for a phase, in a word the
// vector's alignment paid for anyway; see `solid_format.wesl` for what asking a
// cell scan instead cost. `edges` rides in the other one and says whether this
// box's faces are surfaces the art drew at all. And see
// `crate::impostor::Volume::write`, which is the one place this layout is
// spelled on the other side.
struct Volume {
    lo: vec3<f32>,
    edges: u32,
    hi: vec3<f32>,
    solid: u32,
};

// All four sides named at once — `crate::occlusion::Edges::ANY`, the same four
// bits `blit.wesl` masks with its own `EDGE_MASK`.
//
// **What it means is that the art named none of them.** `edges_of(None)` is
// where it comes from, and nowhere else: a thing that stands up whose facing the
// silhouette detector would not say. A literal here and pinned from this source
// by `grids.rs` — `docs/render/design_pixel_spaces.md` rule 6 — because there is no compiler on
// either side of this wire.
//
// **A climbable's treads used to be in that set and are not.** They were,
// because this word carried `occlusion::boxes_of`'s mask, and that branch
// overrides the art's answer with `Edges::ANY` deliberately — to pick the exact
// slab test a solid takes over a lid's crossing test. Read here it said the
// opposite of what the picture says: a flight's treads and risers are planes
// `facing::Prism` measured, and every staircase in the world came out with no
// facing and no shading of its own. `occlusion::named_edges` is what fills this
// word now — what the *art* named, which is a different question from what the
// grid occludes with.
const EDGES_ANY: u32 = 15u;

@group(0) @binding(5) var<storage, read> volumes: array<Volume>;

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    // Zero is "no hue"; otherwise the wire value `Hue` carries, index and
    // partial-flag both, untouched until the fragment shader needs them.
    @location(1) @interpolate(flat) hue: u32,
    // Where in the world this sprite stands, flat across the instance — see
    // `crate::place`. A wall's picture is 44 pixels above the tile it stands on,
    // and this is what says so: the lighting pass reads the tile rather than
    // guessing it back from the pixel's position on the screen.
    @location(2) @interpolate(flat) place: vec2<u32>,
    // Where this fragment is up the screen, and where the sprite's bottom edge
    // is, both in viewport pixels. Their difference is the *height* the pixel
    // stands at: a sprite is a picture of something vertical, four pixels of it
    // are one unit of `z`, and its bottom edge sits on the diamond's bottom
    // vertex. Without this a wall is lit as one flat thing from its base tile,
    // which reads as a lantern shining on a signboard.
    @location(3) pixel_y: f32,
    @location(4) @interpolate(flat) bottom_y: f32,
    // Where this fragment is across the screen, and where the sprite's middle
    // is. Their difference is how far the pixel is from the column its tile
    // stands in, and what that *means* is the stance's business: for a floor it
    // is half of the inverse projection, and for a wall it is how far along the
    // one edge of the diamond the wall stands on. See `crate::place::Stance`.
    @location(5) pixel_x: f32,
    @location(6) @interpolate(flat) middle_x: f32,
    // This instance's own index — what the attachment now carries in place of
    // `x`/`y`, and what `blit.wgsl` looks up in its own copy of this pass's
    // instance buffer, bound a second time as storage. See
    // `docs/archive/render/gbuffer.md` decision 2 and step 3.
    @location(7) @interpolate(flat) id: u32,
    // The row of this instance's other face, for a corner — `0` and unread
    // for anything else. See `crate::sprite::SpriteQuad::twin` and
    // `docs/archive/render/gbuffer.md` step 4.
    @location(8) @interpolate(flat) twin: u32,
    // Which run of `volumes` is this instance's own: `(offset, count)`, and
    // `(_, 0)` for a picture the grid holds no shape for. See
    // `crate::impostor::Range`.
    @location(9) @interpolate(flat) volumes: vec2<u32>,
    // Where this sprite sits in the atlas, as the vertex stage was handed it —
    // `(u, v, du, dv)`. The fragment stage reads it for one thing only: a
    // neighbouring *texel* of the art may belong to the sprite packed next to
    // this one, and the art's own alpha edge is exactly the line where this
    // sprite's rectangle ends. Without the rectangle, `art_edge` would ask a
    // stranger's alpha and answer about the atlas's packing.
    @location(10) @interpolate(flat) region: vec4<f32>,
    @location(11) @interpolate(flat) opacity: f32,
    // [`SpriteQuad::with_screen_rotation`] borrows the twin word only for a
    // `Place::NOWHERE` row, where a twin has no meaning. It is the clockwise
    // rotation to undo while sampling the rotated picture's bounding quad.
    @location(12) @interpolate(flat) rotation: f32,
    // The bounding quad's dimensions before the rotation is undone. The atlas
    // region gives the original picture's dimensions, not these expanded ones.
    @location(13) @interpolate(flat) bound_size: vec2<f32>,
};

// Virtual pixels one unit of height lifts a sprite up the screen —
// `camera::Z_STEP`, and the same number `ground.wgsl` is handed in its uniform
// block. A constant here because this pass is not given the world's grid, only
// rectangles somebody else placed in it.
//
// Read by the *no volume* case alone now: a picture nothing measured a shape for
// is a billboard, and what runs down a billboard is height. Everything with a
// box gets its height from the box.
const Z_STEP: f32 = 4.0;

// Half a tile's height: how far above the sprite's bottom edge the tile's own
// centre is, since that edge stands on the diamond's bottom vertex. The one
// thing this pass still spells about the projection — `impostor::ray_from` is
// the rest of it.
const HALF_TILE_HEIGHT: f32 = 22.0;

// **Which stance a met box's face is**, and the whole of it: `meets` only ever
// names a *camera-facing* face, so the answer is one of three and never
// `FACE_NORTH` or `FACE_WEST`. That is not a gap — a fragment is drawn on the
// side of a box the camera can see, and for a panel standing on its tile's north
// edge that side is the box's `hi.y`, which is what `FACE_SOUTH` names. The
// stance a picture's *art* was read as says which edge of the tile the thing
// stands on; this says which surface the fragment is actually a point of, and
// `blit.wesl` wants the second — `lit_plane` is a plane, and a plane the
// fragment is not in is the wrong one to excuse a candidate against.
//
// `docs/render/design_model.md` phase 6g. What made this visible is 6d taking the
// mesh pass — whose rows carried an honest per-face stance — off real statics:
// a flight of steps is read as a **corner** by the silhouette detector (see
// `occlusion::boxes_of`, which says why), so every pixel of a stair tread was
// carrying the face of a corner panel it is nowhere near, resolved by which half
// of the sprite it was drawn on. The wedge that put on the screen has a vertical
// edge down the sprite's own middle column, which is what `across > 0.0` is.
fn stance_of(normal: vec3<f32>) -> u32 {
    if normal.z == 1.0 {
        return STANCE_FLAT;
    }
    if normal.x == 1.0 {
        return STANCE_FACE_EAST;
    }
    return STANCE_FACE_SOUTH;
}

// `crate::place::Stance`, in the four bits `Place::packed` writes it to — the
// *instance* word's own bits, a different word from the G-buffer's own
// `IDS_STANCE_SHIFT`. The numbers here are pinned on the Rust side; nothing
// but a person reading both files can check that these agree with them.
const STANCE_SHIFT: u32 = 16u;
const STANCE_MASK: u32 = 15u;

// Which box of an instance's own run a view ray landed on, and the meeting
// itself.
//
// The pair travels together because `meets` answers about a pair of corners and
// has no notion of whose they were: re-finding the winner afterwards by
// comparing `t`s would be a second rule that could disagree with the one that
// chose it. `crate::impostor::nearest` is the Rust twin, and returns the same
// two things for the same reason.
struct Choice {
    met: Meeting,
    // Which row of `volumes` — an absolute index, not an offset into the run.
    at: u32,
};

// **Which of this instance's boxes the ray through `start` belongs to.**
//
// `impostor::nearest`, spelled here because the boxes are a binding rather than
// an argument. The box in *front* wins among those the ray genuinely goes
// through — the largest `t` along the view direction — and if it goes through
// none of them, the one it passes closest to. That last case is the sprite
// overhanging its own volume, and `Meeting::outside` is how far; the caller
// decides what to do about it.
//
// A function and not an inlined loop because there are four more callers than
// there used to be: `box_edge` below asks the same question of this fragment's
// four screen neighbours, and a second copy of the selection rule is a second
// chance to disagree with it — which is the same argument the pairing above
// carries one level down.
fn nearest(start: vec3<f32>, run: vec2<u32>) -> Choice {
    var best = meets(start, volumes[run.x].lo, volumes[run.x].hi);
    var best_at = run.x;
    for (var k = 1u; k < run.y; k = k + 1u) {
        let volume = volumes[run.x + k];
        let met = meets(start, volume.lo, volume.hi);
        // A real hit always beats a miss, however near the miss came; two hits
        // are decided by which is in front, and two misses by which came closer.
        // Ties go to the box the list holds first, which for a flight is the
        // grid's own order — a tread's top before the tread above it.
        var better = false;
        if hit(best) == hit(met) {
            if hit(met) {
                better = met.t > best.t;
            } else {
                better = met.outside < best.outside;
            }
        } else {
            better = hit(met);
        }
        if better {
            best = met;
            best_at = run.x + k;
        }
    }
    var choice: Choice;
    choice.met = best;
    choice.at = best_at;
    return choice;
}

// The four screen neighbours a silhouette test asks about, as `(across, down)`
// steps. Diagonals are left out on purpose: an outline that steps by a whole
// texel and one that steps by a whole fragment are both 4-connected, and adding
// the corners would mark the inside of every staircase as an edge.
const NEIGHBOURS: array<vec2<f32>, 4> = array<vec2<f32>, 4>(
    vec2<f32>(-1.0, 0.0),
    vec2<f32>(1.0, 0.0),
    vec2<f32>(0.0, -1.0),
    vec2<f32>(0.0, 1.0),
);

// **Is this fragment on the art's own alpha edge** — does a neighbouring *texel*
// of the sprite fail the alpha test this fragment passed?
//
// The quantum here is a texel and not a pixel, and that is the point rather than
// an approximation: the quad is the sprite's own rectangle in virtual pixels and
// the sampler is `nearest`, so one texel is `Projection::scale` real pixels
// square and the art's outline *cannot* step more finely than that. At `4x` the
// band this marks is four real pixels wide, which is exactly the stair a person
// sees. `docs/render/design_silhouettes.md`.
//
// Off the end of the sprite's own rectangle counts as an edge. The atlas packs
// other graphics against this one, so the neighbour texel there belongs to a
// stranger — and a rectangle's last column is an outline whether or not the art
// bothered to draw a transparent margin round itself.
//
// `textureLoad` and not `textureSample`: an exact texel is wanted, this is the
// only sampling in the pass that does not want the sampler's opinion, and a load
// has no uniformity condition to satisfy.
fn art_edge(uv: vec2<f32>, region: vec4<f32>) -> bool {
    let dims = vec2<f32>(textureDimensions(atlas_texture));
    // The sprite's own rectangle, in texels. `round` and not `floor`: the region
    // is a rectangle of whole texels expressed as a fraction of the atlas, so
    // the product is a whole number to within a rounding, and flooring it would
    // land one short whenever the division came out just under.
    let lo = round(region.xy * dims);
    let hi = lo + round(region.zw * dims);
    let here = floor(uv * dims);
    for (var k = 0u; k < 4u; k = k + 1u) {
        let there = here + NEIGHBOURS[k];
        if any(there < lo) || any(there >= hi) {
            return true;
        }
        if textureLoad(atlas_texture, vec2<i32>(there), 0).a < 0.5 {
            return true;
        }
    }
    return false;
}

// **And is it on the box's edge** — does a neighbouring *fragment*'s view ray
// miss every box this instance stands as, where this one hit?
//
// The other half of the pair, at the other resolution. A box's silhouette is
// decided by `nearest` per fragment, so it steps once a *real* pixel — which is
// why the step is `1 / viewport.scale` virtual pixels rather than one. The two
// numbers this pass hands the fragment stage are in virtual pixels; the picture
// a person is looking at is in real ones.
//
// Only asked of a static the grid holds a shape for. A mobile and a static with
// no volume are art-bounded by construction — there is no box to end them —
// which is what makes them this view's own positive control.
fn box_edge(tile: vec2<f32>, base: f32, across: f32, down: f32, run: vec2<u32>) -> bool {
    let step = 1.0 / viewport.scale;
    for (var k = 0u; k < 4u; k = k + 1u) {
        let offset = NEIGHBOURS[k] * step;
        let start = ray_from(tile, base, across + offset.x, down + offset.y);
        if !hit(nearest(start, run).met) {
            return true;
        }
    }
    return false;
}

// What one fragment of a world pass writes: the picture, and where in the world
// it came from.
struct FragmentOut {
    @location(0) color: vec4<f32>,
    // What drew this fragment and which row it came from —
    // `crate::gbuffer::IDS_FORMAT`.
    @location(1) ids: u32,
    // And where in the world that fragment is — `crate::gbuffer`. The point
    // itself, and not the tile-local fraction and sixteenths of a `z` unit the
    // id word used to carry beside the id.
    @location(2) position: vec4<f32>,
    // And which way this fragment's surface looks — `crate::gbuffer::
    // NORMAL_FORMAT`, one octahedral word, `normal_format.wesl`. This pass is
    // where a **corner** is taken apart, which is the whole reason the normal is
    // written here rather than derived by the reader: a corner's two faces are
    // two halves of one picture and one pixel is on one of them, and by the time
    // anything downstream sees this fragment that decision has already been made
    // and thrown away.
    @location(3) normal: u32,
};

@vertex
fn vs_main(
    // The unit quad: (0,0) top-left to (1,1) bottom-right, in screen axes this
    // time rather than the map's.
    @location(0) corner: vec2<f32>,
    // Per instance: the sprite's top-left corner, in viewport pixels.
    @location(1) origin: vec2<f32>,
    // Per instance: its size in pixels.
    @location(2) size: vec2<f32>,
    // Per instance: where it sits in the atlas, as (u, v, du, dv).
    @location(3) region: vec4<f32>,
    // Per instance: where it sorts. Smaller is nearer.
    @location(4) depth: f32,
    // Per instance: the wire hue, or 0 for none.
    @location(5) hue: u32,
    // Per instance: the tile and height, packed as
    // `crate::place::Place::packed` writes it.
    @location(6) place: vec2<u32>,
    // Per instance: a corner's paired shadow row, or `0`.
    @location(7) twin: u32,
    // Per instance: which run of `volumes` is its own, as
    // `crate::impostor::Range` wrote it.
    @location(8) volumes: vec2<u32>,
    @builtin(instance_index) instance_index: u32,
) -> VertexOut {
    let pixel = origin + corner * size;
    // Virtual pixels to real ones, the same single line the ground pass ends on
    // — `floor` included, and `ground.wesl` carries the account of why the
    // centre of an odd-sized image may not be half a pixel. This pass is the
    // one the defect showed in, because it is the one that meets a box.
    let real = (pixel - viewport.origin) * viewport.scale + floor(viewport.size * 0.5);

    // Pixels to clip space, the same way the ground does it: `y` flips because
    // the viewport counts down from the top and clip space counts up from the
    // middle.
    let ndc = vec2<f32>(
        real.x / viewport.size.x * 2.0 - 1.0,
        1.0 - real.y / viewport.size.y * 2.0,
    );

    var out: VertexOut;
    out.clip = vec4<f32>(ndc, depth, 1.0);
    // The quad is the sprite's own size in pixels, so the corner coordinates
    // *are* the region's edges and a fragment's centre lands on a texel's
    // centre. No half-texel inset: unlike a stretched land texture, nothing
    // here samples the far edge of the region at a vertex.
    out.uv = region.xy + corner * region.zw;
    out.region = region;
    out.hue = hue & 0xffffu;
    let encoded_opacity = (hue >> 16u) & 0xffu;
    out.opacity = select(1.0, f32(encoded_opacity) / 255.0, encoded_opacity != 0u);
    // `Place::NOWHERE` is `(0, 128)`, not zeroed bytes: `Place::packed`
    // offsets its zero height by 128 even when its kind is `Nothing`.
    out.rotation = select(0.0, bitcast<f32>(twin), all(place == vec2<u32>(0u, 128u)));
    out.bound_size = size;
    out.place = place;
    out.pixel_y = pixel.y;
    out.bottom_y = origin.y + size.y;
    out.pixel_x = pixel.x;
    // The sprite is centred on the tile's column — `statics::stand_on`, which is
    // `View.DrawStatic`'s `x -= (width >> 1) - 22`. Half a pixel out on an
    // odd-width sprite, exactly as it is there.
    out.middle_x = origin.x + size.x * 0.5;
    out.id = instance_index | u32(viewport.id_bits);
    out.twin = twin;
    out.volumes = volumes;
    return out;
}

// The bits of a wire hue that are the index into `hues.mul`, and the bit
// asking for a partial (grey-pixels-only) tint. `openshard_uofiles::hues`
// is the port these came from — see `Hues::get` and `is_partial`.
const HUE_INDEX_MASK: u32 = 0x3FFFu;
const HUE_PARTIAL_FLAG: u32 = 0x8000u;

@fragment
fn fs_main(in: VertexOut) -> FragmentOut {
    var uv = in.uv;
    if in.rotation != 0.0 {
        // `in.uv` first names a point in the enclosing axis-aligned bound.
        // Turn it back into the original picture before sampling. Texture
        // dimensions are the original sprite's on-screen dimensions here: the
        // ordinary static path maps one atlas texel to one virtual pixel.
        let bound = (in.uv - in.region.xy) / in.region.zw;
        let point = (bound - vec2<f32>(0.5, 0.5)) * in.bound_size;
        let sin_rotation = sin(in.rotation);
        let cos_rotation = cos(in.rotation);
        let original = vec2<f32>(
            cos_rotation * point.x + sin_rotation * point.y,
            -sin_rotation * point.x + cos_rotation * point.y,
        );
        let original_size = abs(in.region.zw) * vec2<f32>(textureDimensions(atlas_texture));
        let source = original / original_size + vec2<f32>(0.5, 0.5);
        if any(source < vec2<f32>(0.0, 0.0)) || any(source > vec2<f32>(1.0, 1.0)) {
            discard;
        }
        uv = in.region.xy + source * in.region.zw;
    }
    let color = textureSample(atlas_texture, atlas_sampler, uv);

    // The sprite's shape. Discarding rather than blending is what keeps the
    // pass independent of draw order — a discarded fragment writes no depth
    // either, so the ground behind a tree's gaps stays visible.
    if color.a < 0.5 {
        discard;
    }

    var rgb = color.rgb;
    if in.hue != 0u {
        let partial = (in.hue & HUE_PARTIAL_FLAG) != 0u;
        // A partial hue leaves anything already coloured alone — skin, metal,
        // whatever the art painted with its own hue rather than in grey — and
        // only a full hue always replaces the pixel. Equality rather than a
        // tolerance: nearest sampling reproduces the atlas's stored bytes
        // exactly, with no filtering to blur two channels apart.
        if !partial || (color.r == color.g && color.g == color.b) {
            // The art's red channel is not a colour here, it is the index
            // `get_rgb(color.r, hue)` reads — see `crate::hue`'s docs for why
            // `round(r * 31.0)` recovers the file's 5-bit value exactly.
            let index = i32(round(color.r * 31.0));
            let row = i32((in.hue & HUE_INDEX_MASK) - 1u);
            rgb = textureLoad(hue_ramp, vec2<i32>(index, row), 0).rgb;
        }
    }

    var out: FragmentOut;
    out.color = vec4<f32>(rgb, in.opacity);

    // The tile, the height and the kind, taken apart the way
    // `crate::place::Place::packed` put them together. A discarded fragment
    // never reaches this line, so what the attachment holds is what is visible.
    let base = f32(in.place.y & 0xFFu) - 128.0;

    // Where this pixel is on the screen relative to its tile: `across` from the
    // column the sprite is centred on, `down` from the tile's own centre row.
    // Both are what the projection is inverted through below.
    let across = in.pixel_x - in.middle_x;

    // A **corner** is two faces in one picture — the client draws every
    // building's corner that way — and which of them a pixel is a pixel *of* is
    // which half of the tile's column it was drawn on. So the corner is resolved
    // here, per fragment, and nothing downstream ever sees one: what goes into
    // the id word is a single face, which is the side of the tile `blit.wgsl`'s
    // own-run test and `own_solid` ask about. Before this a corner was `Upright`
    // — a flat 44-pixel band between two continuous runs of wall, lit
    // identically on the side turned towards the flame and the side turned away.
    // See `crate::facing::Facing::on_half`.
    //
    // **The screen half and not the box the ray met**, which is the one join
    // this phase left standing: the two panels of a corner are two boxes and the
    // impostor does pick between them for the *normal*, so a fragment could name
    // its own panel outright. What stops it is the id — the left half takes
    // `in.twin`, the row `split_corners` appended, and the box carries no row
    // number to choose one by. See `docs/render/design_model.md` phase 6c.
    // A corner's two faces need two ids, not one shared between them — see
    // `docs/archive/render/gbuffer.md` decision 3 and step 4. The right half (`across > 0.0`)
    // keeps this instance's own id; the left half takes `in.twin`, the row
    // `crate::sprite::split_corners` appended for exactly this face. Every
    // other stance never reaches the branch that changes it.
    var id = in.id;
    var stance = (in.place.y >> STANCE_SHIFT) & STANCE_MASK;
    if stance >= STANCE_CORNER {
        let offset = stance - STANCE_CORNER;
        if across > 0.0 {
            stance = STANCE_FACE_NORTH + (offset >> 1u);
        } else {
            stance = STANCE_FACE_SOUTH + (offset & 1u);
            id = in.twin;
        }
    }
    // How far this pixel is below the row its tile's own centre projects to.
    // With `across` beside it, the two numbers `impostor::ray_from` needs.
    let down = in.pixel_y - (in.bottom_y - HALF_TILE_HEIGHT);

    // The tile the picture stands on, off the instance's own first word — the
    // one `blit.wgsl` fetches back out of `face_instances[id]`.
    let tile = vec2<f32>(f32(in.place.x & 0xFFFFu), f32(in.place.x >> 16u));

    // **Where this fragment is, and which way its surface looks.**
    //
    // Both come from meeting this pixel's own view ray with the boxes *this
    // instance* stands as. What that replaced was an inverse projection guessed
    // from the stance the art was read as: a floor's two fractions taken from the
    // pixel's offset from the tile's centre, a wall's one fraction pinned to the
    // edge its face names and the other run along it, a height recovered from
    // `pixel_y` at four pixels a unit — five branches, each stating a plane the
    // geometry already knew, and every one of them clamped a
    // hundred-and-twenty-seventh of a tile inside the tile so that a reader
    // flooring the result would not land in the neighbour. Nothing floors it: the
    // walk starts from the tile the instance carries, which is a whole number a
    // pass knew exactly.
    //
    // The default is the **no volume** case, and it is two different states
    // wearing one branch — which is why the answer below is not one answer.
    // Neither has a facing at all, which is what `Stance::Upright` has always
    // meant and is what phase 7's own open question is about.
    //
    // A **mobile** has no volume *by construction*: `docs/render/design_model.md`
    // says so in as many words — "a billboard is no volume, so it casts nothing"
    // — and nothing is ever going to measure one. So it is a billboard, and a
    // billboard is a **plane**: the sprite is drawn on a vertical plane through
    // its tile's centre turned towards the camera, and a fragment of it is where
    // its own view ray meets that plane. `impostor::billboard_at` is the
    // derivation and phase 7 is where it belongs; the height it answers with is
    // the one this pass already drew, to the bit.
    //
    // **It stood at the tile's centre until then**, one point for every pixel of
    // a screen row, and a person could see both halves of that: a figure lit
    // flat across, since nothing about the light varied along a row, and
    // horizontal bands over it, since `blit.wesl`'s `dither` turns each row's
    // sample pattern by an angle that belongs to the position.
    //
    // A **static with no box** is the other state, and it is a measurement that
    // is missing rather than one that cannot exist: a picture the grid refused, a
    // text glyph. It keeps the tile's centre, and that is not conservatism —
    // `a_floor_spreads_across_its_tile_and_a_wall_stands_up_it` states the reason
    // and would go red for the other choice. What a wall's picture runs along is
    // the world axis the wall is built on, a screen *diagonal*, and the tiledata
    // does not say which of the two it is; the billboard's plane runs along
    // `x - y`, the one direction no wall runs. Guessing a plane for a wall is
    // therefore wrong in a way that guessing one for a person is not.
    var at = vec3<f32>(tile + vec2<f32>(0.5), base - down / Z_STEP);
    let is_mobile = ((in.place.y >> 8u) & KIND_MASK) == KIND_MOBILE;
    if is_mobile {
        // `in.twin` carries no corner for a mobile — its stance is always
        // `Stance::Upright` — so it is the fraction of a tile the body's
        // drawn position has walked past `tile`, mid-step. Without adding it
        // back the plane `billboard_at` builds stands at the tile the body is
        // arriving at rather than where its picture is drawn, up to a whole
        // tile off. See `crate::mobiles::billboard_offset`.
        at = billboard_at(tile + billboard_offset(in.twin), base, across, down);
    }
    // **A billboard's normal is the plane's own, not the zero vector.**
    // `docs/render/design_model.md` phase 7: a mobile has no volume for `meets` to
    // land on, so before this it fell into the same branch as a static missing
    // a *measurement* (below) and read as lit from every side — flat across a
    // figure, since nothing about the light could then vary along a row. A
    // billboard is not a missing measurement, though; it is a plane, by
    // construction, and the plane has exactly one normal. Stating it is the
    // "facing the camera" candidate this phase picked: never wrong, and cheap
    // enough to owe nothing to the inflated-silhouette alternative it was
    // weighed against.
    //
    // **And which solid of the grid it is a point of**, which is the box's own
    // name and not a second thing to look up. `solid_format.wesl` carries the
    // argument: the scan `blit.wgsl` used to do instead is exact for everything
    // that stands one shape per owner and ambiguous for a fitted climbable,
    // which stands one box per tread under one owner. A mobile and a static the
    // grid holds nothing for keep `SOLID_NOBODY`, which is the honest answer —
    // a point of nothing, exempt from nothing.
    var normal = vec3<f32>(0.0);
    var mine = SOLID_NOBODY;
    // **The two edges this frame draws, at their two resolutions** — the art's
    // alpha at one *texel* a step, the box's rim at one *fragment* a step.
    // `docs/render/design_silhouettes.md` phase Z1, and `place_format.wesl` carries the account
    // of what each one is and why this pass is the only place either can be
    // measured.
    //
    // The art half is asked of every fragment. Its edge is a fact about the
    // picture, and every one of these is a picture whether or not a shape was
    // ever measured for it — a mobile and a static the grid refused included,
    // which is what makes those two this view's own positive control: they can
    // carry the art bit and never the box one.
    var edges = 0u;
    if art_edge(in.uv, in.region) {
        edges = edges | IDS_EDGE_ART;
    }
    if is_mobile {
        normal = billboard_normal();
    } else if in.volumes.y != 0u {
        let start = ray_from(tile, base, across, down);
        let choice = nearest(start, in.volumes);
        let best = choice.met;
        let best_at = choice.at;
        // **The nearest box wins outright, hit or miss** — the pixel is a
        // picture of *this* static, the grid holds volumes for it, so which
        // volume is a question the ray answers and whether the ray landed inside
        // one is not a question about whether anything was measured.
        //
        // This decision has been reversed three times and each reason is worth
        // keeping, because the third is the two earlier ones read together.
        //
        // `docs/render/design_model.md`'s "One silhouette" clamped a miss onto its
        // box, and a clamp names whichever face exits first — along a silhouette
        // that is a *side* one, so a lattice of wall-shaded dots appeared on
        // floors and roofs, one per tile corner. `discard` replaced the clamp on
        // a measurement of 4460 pixels of 187086 in one frame, 2.38%.
        //
        // **That measurement counted the wrong set**: pixels that *changed in
        // one frame with the roof cut*, and a discarded pixel with another
        // static behind it changes nothing. Counted per picture instead —
        // `examples/discard_census.rs`, `docs/render/design_footprints.md`'s S4 — the discard
        // throws away 11.09% of every panel's art and 32.44% of every whole-tile
        // one. A display case (`0x0B06`) read as a corner of two walls stands as
        // two `PANEL_THICKNESS` panels and its whole top meets neither: it left
        // the screen, and a person reported the furniture with a hole in it. So
        // a miss became a *measurement that is missing* — the tile's centre and
        // the zero normal, which `blit.wesl` lights from every side.
        //
        // **And that state, lit from every side, is brighter than the measured
        // surface beside it.** On a floor it drew as a lit grid: a dashed line
        // along every tile seam where the sample fell a pixel outside the box it
        // was plainly a pixel of. `impostor::FRAGMENT` took the seam; what it
        // left was the case for keeping the clamp at all, since the two states
        // differ by which lie a pixel tells — a position off by the overhang, or
        // a facing that is every facing.
        //
        // The clamp is back, and the lattice that killed it the first time is
        // gone at its own root rather than avoided: `shows_a_side` refuses a
        // face thinner than the grid that reads it, so a flat box answers with
        // its lid to its own rim and there is no side face to be handed. Counted
        // before the change: 338 pixels over Britain's 121×121 would have taken
        // one.
        // **And the switch over the three answers a miss has ever had** —
        // `crate::impostor::Fringe`, a knob rather than a setting. `FRINGE_CLAMP`
        // is what every plane downstream is written against; the other two are
        // here so the decision above can be checked by looking at a frame, which
        // is this renderer's own acceptance instrument.
        if !hit(best) && viewport.fringe == FRINGE_DISCARD {
            // The clipping this pass did for one session: honest about position,
            // and it takes the top off a display case.
            discard;
        }
        at = best.at;
        var face = best.normal;
        if !hit(best) && viewport.fringe == FRINGE_VOLUME {
            face = presented_face(volumes[best_at].lo, volumes[best_at].hi);
        }
        mine = volumes[best_at].solid;
        // **And the stance the met face is**, which overrides the art's
        // reading above for every static the grid holds a shape for. See
        // `stance_of`: the corner branch is a statement about the *picture*,
        // and a fragment that has met a box has a better one about itself.
        // What is left of the branch is the `id` — a corner's two halves
        // address two instance rows, and a box carries no row number to
        // choose between them, which is this phase's own remaining join.
        stance = stance_of(face);
        // **A box whose art named no side has no facing to give a fragment.**
        //
        // `meets` always answers with a face, because a ray leaving a box leaves
        // through one — and for a **panel** or a **lid** that face is a plane
        // somebody drew, measured off the picture by the silhouette detector.
        // For a **body** it is not: the box is the tile's own walls, handed to a
        // graphic the art would not name a side for, and every fragment of a
        // thin pole standing in the middle of it was being told it looks the way
        // the tile's south or east wall does.
        //
        // Two things a person reported are that one sentence. A lamp post came
        // out **black in its own light**: `light::place` burns at the tile's
        // centre, which is *inside* the body, so the camera-facing plane the
        // impostor answered its own sprite with has `N · L <= 0` on every
        // visible pixel. And a whole-tile static read **darker and striped**
        // where it used to be even, which is the same claim seen from outside.
        // `docs/render/design_model.md` phase 6c's own backlog filed the two as one
        // finding, and this is the candidate it named: keep the measured normal
        // where the art really does say which way a surface looks, and write
        // none where it does not.
        //
        // **Not an exemption, and not about light sources.** It is a
        // measurement that was never taken, said so — `normal_format.wesl`'s
        // middle state, which `blit.wesl` lights from every side. The same rule
        // is already written one module over: `light::mounted_at` refuses to
        // move a flame off an `Edges::ANY` cell because "there is no direction
        // in it to move along and a guess would be a wrong one".
        //
        // The **stance** is taken from the met face regardless, and that is not
        // an oversight: it names *which face of which box* the ray left by, for
        // `blit.wesl`'s `on_the_lit_surface` and `own_solid`, and that is a fact
        // about the geometry whether or not the art drew a plane there. The
        // facing is the only thing this refuses to claim.
        normal = face;
        if volumes[best_at].edges == EDGES_ANY {
            normal = vec3<f32>(0.0);
        }
        // The box half of the silhouette pair stays behind the `hit`, and that
        // is the one thing here the clamp does not change: the bit marks the rim
        // of the region a box genuinely covers, so a fragment reaching it from
        // outside is behind that rim rather than on it. `docs/render/design_silhouettes.md`.
        if hit(best) && box_edge(tile, base, across, down, in.volumes) {
            edges = edges | IDS_EDGE_BOX;
        }
    }

    // The stance rides into the id word above the kind, and **it is not a
    // facing any more.** The normal is measured now — it is the face of the box
    // the view ray met, where the art named that face at all — and what is left
    // of the stance is the thing no geometry states: *which side of its own
    // tile* a panel stands on. `blit.wgsl` reads
    // it for exactly that, twice — `own_side` for the shadow walk's own-run test,
    // and `own_solid`, which narrows a corner instance's owner to one of its two
    // panels — plus the sentinel that routes a mesh face's id to its own buffer.
    // See `crate::place::Place`.
    //
    // Nothing else rides here. The **tile**'s `x`/`y` went to this pass's own
    // instance buffer at `docs/archive/render/gbuffer.md` step 3, read back by `blit.wgsl` as
    // `face_instances[id]`, since the whole tile is the same two numbers on
    // every one of a sprite's pixels; the height and the fraction went to the
    // position plane at `docs/render/design_model.md` phase 2, since both are this
    // fragment's own and both were being quantised on the way.
    out.ids = pack_ids(id, stance, (in.place.y >> 8u) & KIND_MASK) | edges;
    // The fourth channel is the solid this fragment is a point of, not a
    // homogeneous `1.0` — `solid_format.wesl`, and the only place in this pass
    // that a *name* rides beside a position.
    out.position = vec4<f32>(at, pack_solid(mine));
    // `pack_normal` carries the two bits that separate the three answers: a zero
    // vector is a real one here — a picture with no measured volume has no side,
    // and neither has a body, whose box the art named no side of — so "nothing
    // drew this pixel" needs a bit of its own to be distinguishable from it.
    out.normal = pack_normal(normal);
    return out;
}
