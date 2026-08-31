//! The attachments a world pass writes beside the picture.
//!
//! One thing per pixel is not enough and never was. A frame's lighting asks
//! where a fragment is, which way its surface looks, what colour it is and what
//! object it belongs to — four different questions, and
//! `docs/lighting_rebuild.md` phase 2 is the decision to answer each of them
//! with *data* rather than with something reconstructed downstream from a
//! packed byte.
//!
//! Three planes: the position below, the normal beside it, and the id plane
//! that says what a fragment *is*. Albedo is the fourth and it is phase 6's —
//! a mesh face has none today, and the day it does the picture stops being a
//! separate attachment because the albedo *is* the picture.
//!
//! This module exists so that adding one is not thirty edits. Every fixture
//! that draws a frame — the app, four examples, four test harnesses — used to
//! create the `place` texture by hand, take a view of it, and hand that view to
//! [`Target`](crate::renderer::Target) and to [`Frame`](crate::blit::Frame) as a
//! separate argument. A second attachment repeats all of that; a fourth repeats
//! it three times more, and a fixture that forgot one does not fail to compile
//! until the shader reads a plane nobody bound. A [`Gbuffer`] is the whole set,
//! created once and passed whole.
//!
//! **Owning the textures and lending the views** is the split, and it is forced
//! rather than chosen: `wgpu::Texture::create_view` returns an owned
//! `TextureView`, and a render pass borrows one for as long as it runs. So a
//! caller holds a [`Gbuffer`] for the frame's lifetime and a [`Views`] for the
//! pass's, which is exactly the two lifetimes the hand-written code had — only
//! now they are named.
//!
//! # What a G-buffer costs to bind, and the limit it fits under exactly
//!
//! A world pass writes the picture and every plane here in one draw, and WebGPU
//! bounds the *total* bytes a fragment may write across a pass's colour
//! attachments: `maxColorAttachmentBytesPerSample`, whose guaranteed minimum is
//! **32**. The set comes to exactly that. See [`required_limits`], where the
//! arithmetic is written out, and [`NORMAL_FORMAT`] for the twelve bytes that
//! had to be given back to get there.

/// The device limits this crate's pipelines need, above the guaranteed
/// minimum — one spelling, because eleven call sites request a device and a
/// tenth of them getting this right is a pipeline that fails to create on one
/// machine and not on another.
///
/// **Only `max_color_attachment_bytes_per_sample`, and only because deferred
/// shading is what it bounds.** A world pass writes the picture and every plane
/// of the G-buffer in one draw, and WebGPU counts the bytes a fragment writes
/// across all of them together. [`ATTACHMENT_BYTES_PER_SAMPLE`] is the sum,
/// spelled out and pinned by a test, so that a plane added or a format widened
/// moves a number a person can read rather than turning into a validation error
/// naming no cause.
///
/// **This asks for nothing above the floor today, and it is kept because the
/// next plane will.** The set came to 44 while the normal plane was three
/// floats and a coverage channel, which meant plainly that a device reporting
/// only the guaranteed minimum could not run this client; [`NORMAL_FORMAT`] is
/// one packed word now and the sum is exactly 32. Phase 6's target layout —
/// position, normal, albedo and ids, with no separate picture beside them
/// because the albedo *is* the picture — is the same 32, so the arithmetic
/// below is what says that phase still fits rather than something a person
/// re-derives when it lands.
///
/// The two steps that got there, in order, because each is a different kind of
/// saving. The id plane bought four: [`IDS_FORMAT`] is an `R32Uint` where the
/// `place` attachment it replaced was an `Rgba16Uint`, and the eight bytes that
/// plane charged were mostly a height and a fraction the position plane now
/// carries exactly. The normal plane bought twelve, and bought them without
/// giving anything up — see its own doc.
pub fn required_limits() -> wgpu::Limits {
    wgpu::Limits {
        max_color_attachment_bytes_per_sample: attachment_bytes_per_sample(),
        ..wgpu::Limits::default()
    }
}

/// What one fragment of a world pass writes, in bytes: the picture plus every
/// plane of a [`Gbuffer`].
///
/// Summed from [`ATTACHMENTS`] rather than written down, so that a plane added
/// or a format widened moves the limit with it and cannot be forgotten. What is
/// written down is [`ATTACHMENT_BYTES_PER_SAMPLE`], the number this is expected
/// to come to — the two are compared by `a_g_buffer_costs_what_it_says`, which
/// is what makes a change to the set show up as a number a person reads rather
/// than as a silently wider request.
///
/// Not a count of the bytes the *formats* obviously are — WebGPU charges each
/// attachment a per-format cost of its own, rounded for alignment, so an
/// `Rgba8Unorm` picture costs 8 rather than 4.
fn attachment_bytes_per_sample() -> u32 {
    ATTACHMENTS
        .iter()
        .map(|format| {
            format
                .target_pixel_byte_cost()
                .expect("every attachment of a world pass is colour-renderable")
        })
        .sum()
}

/// What that sum is expected to be — and it is WebGPU's guaranteed minimum
/// exactly, not a number under it with room to spare. See
/// [`attachment_bytes_per_sample`].
pub const ATTACHMENT_BYTES_PER_SAMPLE: u32 = 32;

/// Every colour attachment a world pass writes, in the order it binds them.
///
/// Here rather than in the test that sums them so the list is beside the number
/// it justifies — and so that the day the picture stops being a separate
/// attachment (phase 6's impostor, where the albedo *is* the G-buffer) the
/// deletion is one line in one place.
const ATTACHMENTS: [wgpu::TextureFormat; 4] = [
    crate::blit::WORLD_FORMAT,
    IDS_FORMAT,
    POSITION_FORMAT,
    NORMAL_FORMAT,
];

/// What drew each pixel: its [`crate::place::Kind`], its
/// [`crate::place::Stance`], and the instance row the picture came from.
///
/// `R32Uint`, and the layout is [`pack_ids`]. Integers because these are three
/// identities: a kind is an enumeration, a stance is an enumeration, and an id
/// is a row number. `R32Uint` is colour-renderable in WebGL2, which is the
/// ceiling this crate draws under — see the crate docs.
///
/// **This is what is left of the `place` attachment**, which was four `u16`
/// channels holding all of the above plus a height in whole units and
/// sixteenths and seven bits of tile-local `x` and `y`.
/// `docs/lighting_rebuild.md` phase 2 moved the height and the fraction into
/// [`POSITION_FORMAT`] and the facing they stood in for into
/// [`NORMAL_FORMAT`]; what was left fitted in six bits and an id, so eight
/// bytes a fragment became four. See [`required_limits`] for what that bought.
///
/// One `u32` and not four narrower channels because there is nothing to split:
/// a reader wants the whole word and asks it three questions, and three
/// channels of a `Rgba8Uint` would cap the id at 255. Nor is it folded into
/// the position plane's fourth channel, which is a *coverage* bit — a float
/// that has to round-trip an integer id is a format with a silent ceiling,
/// and this one has an honest one.
pub const IDS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Uint;

/// What an untouched pixel of [`IDS_FORMAT`] is left as: zero, which is
/// [`crate::place::Kind::Nothing`] with no stance and no row.
///
/// **The one clear value in this module that is read.** The other two planes'
/// are never looked at — the lighting has already passed a `Kind::Nothing`
/// pixel through untouched by the time it would reach them — and that test is
/// this word. So the kind lives at the bottom of [`pack_ids`]'s layout, where a
/// zero word and a stamped-as-nothing word are the same number;
/// `nothing_drawn_and_nothing_cleared_are_one_kind` is the other half of it.
pub const IDS_CLEAR: wgpu::Color = wgpu::Color {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 0.0,
};

/// Where a [`crate::place::Stance`] rides in an id word, above the kind's two
/// bits: four bits, which is what its eleven values need.
///
/// `place_format.wesl`'s `IDS_STANCE_SHIFT`, and nothing but a person reading
/// both files can compare them — [`pack_ids`] below is what everything on this
/// side goes through.
pub const IDS_STANCE_SHIFT: u32 = 2;

/// The stance's mask: `place_format.wesl`'s `IDS_STANCE_MASK`.
pub const IDS_STANCE_MASK: u32 = 15;

/// And where the row's own number rides, above both: twenty-four bits, under
/// the two edge bits at the very top.
///
/// Not a budget anyone is near — an id is a row in one of four instance
/// buffers and the widest frame this client has drawn is in the low thousands
/// — but a real ceiling rather than an unstated one, which is why
/// [`pack_ids`] asserts against it.
pub const IDS_ID_SHIFT: u32 = 6;

/// How much of the word above [`IDS_ID_SHIFT`] is the row: twenty-four bits,
/// `place_format.wesl`'s `IDS_ID_MASK`.
///
/// It was twenty-six until the two bits below took the top of the word. A
/// *stated* ceiling rather than "whatever is left", so that a reader of
/// [`ids_id`] can see where the row stops and the edges begin.
pub const IDS_ID_MASK: u32 = 0x00FF_FFFF;

/// A static-row id belongs to the server-item instance buffer rather than the
/// immutable-map buffer.  The two buffers are deliberately distinct: cached
/// map composites never carry a frame-local row, while items must.
pub const IDS_DYNAMIC_ITEM: u32 = 0x0040_0000;

/// A cached map composite owns this pixel.  Its position and normal are still
/// ordinary G-buffer facts, but lighting and selection resolve its tile from
/// that position instead of indexing a transient instance buffer.
pub const IDS_COMPOSITE_MAP: u32 = 0x0080_0000;

/// **The two edges a magnified frame draws** — `docs/silhouettes.md`, and
/// `place_format.wesl`'s `IDS_EDGE_ART` / `IDS_EDGE_BOX`, which is where the
/// argument for the layout lives.
///
/// They are drawn at two resolutions: the art's alpha steps once a texel, the
/// impostor box's rim once a fragment. Only the statics pass sets either bit,
/// because it is the one place both masks are alive at once. The ground and the
/// mesh pass leave both clear, which is the honest answer and not a gap — see
/// [`crate::debug::View::SilhouetteArt`].
///
/// Both set is a real state and the one the plan is about: the seam reaching the
/// outline, where the two resolutions meet along one line.
pub const IDS_EDGE_ART: u32 = 0x4000_0000;

/// See [`IDS_EDGE_ART`].
pub const IDS_EDGE_BOX: u32 = 0x8000_0000;

/// One id word, from the three things it is made of — the Rust twin of
/// `place_format.wesl`'s `pack_ids`.
///
/// For the two things on this side that build a G-buffer without drawing a
/// frame: [`crate::plan`]'s diagnostic pictures and the fixtures in `tests/`.
/// See [`Fragment`], which is what they actually call.
pub fn pack_ids(id: u32, stance: crate::place::Stance, kind: crate::place::Kind) -> u32 {
    debug_assert!(
        id <= IDS_ID_MASK,
        "an instance id past what an id word holds: {id}",
    );
    kind as u32 | (stance as u32) << IDS_STANCE_SHIFT | id << IDS_ID_SHIFT
}

/// The kind an id word names — the twin of `place_format.wesl`'s `ids_kind`,
/// for a test that reads a rendered plane back.
///
/// `None` for a word no [`crate::place::Kind`] spells, which the two bits
/// cannot produce; it is a `match`'s honest arm rather than a case a caller
/// has to handle.
pub fn ids_kind(word: u32) -> Option<crate::place::Kind> {
    use crate::place::Kind;

    match word & 3 {
        0 => Some(Kind::Nothing),
        1 => Some(Kind::Land),
        2 => Some(Kind::Static),
        3 => Some(Kind::Mobile),
        _ => None,
    }
}

/// The stance it names, and the row it names: `place_format.wesl`'s
/// `ids_stance` and `ids_id`.
pub fn ids_stance(word: u32) -> u32 {
    (word >> IDS_STANCE_SHIFT) & IDS_STANCE_MASK
}

/// See [`ids_stance`].
pub fn ids_id(word: u32) -> u32 {
    (word >> IDS_ID_SHIFT) & IDS_ID_MASK
}

/// And which of the two masks ended this fragment's outline: [`IDS_EDGE_ART`],
/// [`IDS_EDGE_BOX`], both, or neither.
///
/// `place_format.wesl`'s `ids_edges`, for a test that reads a rendered plane
/// back — which is the only reader on this side, the pictures being
/// [`crate::debug::View::SilhouetteArt`]'s business.
pub fn ids_edges(word: u32) -> u32 {
    word & (IDS_EDGE_ART | IDS_EDGE_BOX)
}

/// Where each pixel's fragment is in the world, and **which solid of the grid it
/// is a point of**: `(x, y, z, solid)`.
///
/// `Rgba32Float`, and the four channels are not a packing — they are the number
/// itself. What this replaces is a `tile` fetched from an instance row, a
/// seven-bit fraction of it read out of the `place` attachment's fourth
/// channel, and a height read out of the third as eight whole units and four
/// sixteenths. Every one of those three is exact in the pass that computed it
/// and quantised on the way here, and every constant on the height track — see
/// `docs/lighting_rebuild.md`'s three roots — exists to compensate for the fact
/// that a fragment's own position was not exactly known.
///
/// Full `f32` and not a smaller float because the thing stored is a map
/// coordinate: `Rgba16Float` has ten bits of mantissa, so a tile at 7,000 —
/// inside a real facet — resolves to about four tiles. The plane is 16 bytes a
/// pixel at the world image's size, which is under 4 MB at any zoom this client
/// draws.
///
/// `z` is in **`z` units**, the map's own, exactly as
/// [`crate::place::unpacked_height`] returned it. `docs/lighting_rebuild.md`'s
/// "one metric" — `z` divided into tiles once, where the map is read — is the
/// next step and not this one: the occlusion grid, every solid's span and the
/// whole shadow walk are stated in `z` units, and a G-buffer that alone counted
/// differently would be a second metric rather than one.
///
/// **The fourth channel is not a homogeneous `1`.** It is the
/// [`crate::occlusion::SolidId`] the fragment is a point of, or [`SOLID_NONE`]
/// for a point of none — see `solid_format.wesl`, which carries the argument,
/// and [`pack_solid`]/[`unpack_solid`], which are its Rust twins. It is the one
/// number the shadow walk compares, and it rides here because the pass that
/// knows it is the pass that met the box: an id fits a `f32` exactly and this
/// channel was a constant.
pub const POSITION_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;

/// What an untouched pixel of [`POSITION_FORMAT`] is left as.
///
/// Never read: a pixel nothing drew is [`crate::place::Kind::Nothing`], which
/// the blit passes through before it looks at any of this. The fourth channel
/// is negative where a fragment wrote no solid and `0` here, so a plane read on
/// its own — by a diagnostic, or by a test — can still tell "nothing drew this"
/// from "a point of nothing" without a second fetch.
pub const POSITION_CLEAR: wgpu::Color = wgpu::Color {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 0.0,
};

/// A point of no solid at all, as [`POSITION_FORMAT`]'s fourth channel holds it
/// — `solid_format.wesl`'s `SOLID_NONE`.
///
/// Negative because [`crate::occlusion::SolidId::NOBODY`] is `u32::MAX` and that
/// is the one id an `f32` cannot carry exactly. Everything a real id can be —
/// three bytes, see [`crate::occlusion::SolidId`] — is under `2^24` and
/// therefore exact.
pub const SOLID_NONE: f32 = -1.0;

/// One [`crate::occlusion::SolidId`] word as the channel it rides in:
/// `solid_format.wesl`'s `pack_solid`.
pub fn pack_solid(solid: u32) -> f32 {
    match solid == crate::occlusion::SolidId::NOBODY {
        true => SOLID_NONE,
        false => solid as f32,
    }
}

/// And back — `solid_format.wesl`'s `unpack_solid`.
///
/// The pair is here for the same reason [`pack_normal`] and [`unpack_normal`]
/// are: a test that reads a plane back off the GPU asserts the *word* the shader
/// wrote against the word this side computes, so the two spellings of one format
/// are compared rather than each trusted.
pub fn unpack_solid(channel: f32) -> u32 {
    match channel < 0.0 {
        true => crate::occlusion::SolidId::NOBODY,
        false => channel as u32,
    }
}

/// Which way each pixel's surface looks: one octahedral word, and the layout is
/// [`pack_normal`].
///
/// `R32Uint`, four bytes a fragment where this was sixteen — three `f32`s of
/// unit vector and an `f32` of coverage. Those twelve bytes were the whole of
/// what stood between this client and WebGPU's guaranteed
/// `maxColorAttachmentBytesPerSample`; see [`required_limits`], which asks for
/// nothing above it now.
///
/// **The zero vector is a value here and not an absence**, and it is what made
/// the obvious encodings unavailable. A billboard has no side — a tree, a body,
/// a wall whose art named no edge — so every flame that reaches it lights it,
/// and `blit.wesl` branches on exactly that. It is transitional: phase 6 gives
/// an upright static an impostor normal and phase 7 gives a mobile one, and
/// when they do, this stops having a third case. Until then it is a *state*,
/// carried in a bit of its own — [`NORMAL_FACING`] — rather than inferred from
/// the id word beside it, so that the plane still means something read alone.
///
/// **Not `Rg16Snorm`, octahedral, which is what `docs/lighting_rebuild.md`'s
/// own table said.** Every 16-bit norm format is behind
/// `wgpu::Features::TEXTURE_FORMAT_16BIT_NORM`, which is native-only and not in
/// WebGPU's core set — the ceiling this crate draws under (see the crate docs).
/// Nor `Rgba16Float`, the nearest compact renderable float format: this plane
/// is *written from the CPU* by [`crate::plan`]'s diagnostic pictures and by
/// the fixtures in `tests/`, and there is no `f16` on that side, so it would
/// mean a hand-rolled encoder — a second spelling of a float format with no
/// compiler comparing the two, which is the class of defect this crate has
/// already paid for twice.
///
/// An integer word has neither problem, and it turns the encoding into the
/// thing this crate already knows how to keep honest: the same integers on both
/// sides, compared by a test that renders a face and reads the word back. See
/// [`pack_normal`].
pub const NORMAL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Uint;

/// What an untouched pixel of [`NORMAL_FORMAT`] is left as: zero, which is
/// [`NORMAL_DRAWN`] clear.
///
/// The bit is what separates "nothing drew here" from "something drew here and
/// has no facing" — the job the fourth channel did while this was four floats,
/// kept for the same reason and at no cost, since the fifteen bits an axis
/// needs leave two over. The lighting never asks: it has already passed a
/// [`crate::place::Kind::Nothing`] pixel through untouched. `View::Normal` and
/// the tests that copy this plane back do.
pub const NORMAL_CLEAR: wgpu::Color = wgpu::Color {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 0.0,
};

/// Bits of one axis of the octahedral pair — `normal_format.wesl`'s
/// `NORMAL_AXIS_BITS`, and nothing but a person reading both files can compare
/// them. [`pack_normal`] below is what everything on this side goes through.
pub const NORMAL_AXIS_BITS: u32 = 15;

/// That axis's mask: `normal_format.wesl`'s `NORMAL_AXIS_MASK`.
pub const NORMAL_AXIS_MASK: u32 = (1 << NORMAL_AXIS_BITS) - 1;

/// How many steps an axis is quantised into — **even**, and one short of what
/// [`NORMAL_AXIS_MASK`] allows.
///
/// The evenness is the whole of why cardinal normals survive this exactly.
/// `-1`, `0` and `+1` are the three values almost every normal this renderer
/// writes is made of — every wall face, every lid, every level tile — and an
/// odd span puts zero half a step off a code, which would move all of them by a
/// ten-thousandth for nothing. With an even one they land on `0`, the middle
/// code and the top code, and come back bit-identical.
pub const NORMAL_AXIS_SPAN: f32 = 32766.0;

/// The bit that says this fragment has a facing at all —
/// `normal_format.wesl`'s `NORMAL_FACING`. See [`NORMAL_FORMAT`] for the three
/// states this and [`NORMAL_DRAWN`] spell between them.
pub const NORMAL_FACING: u32 = 1 << 30;

/// And the bit that says a fragment was drawn here: `normal_format.wesl`'s
/// `NORMAL_DRAWN`, and the one [`NORMAL_CLEAR`] leaves clear.
pub const NORMAL_DRAWN: u32 = 1 << 31;

/// Which side of zero a number is on, counting zero as the positive side — the
/// twin of `normal_format.wesl`'s `axis_sign`.
///
/// **Neither `f32::signum` nor a `sign` intrinsic.** WGSL's `sign` answers `0`
/// at zero, and the octahedron's fold multiplies by this: a zero there collapses
/// a whole edge of the square onto the origin. `f32::signum` does not have that
/// problem and has a different one — it is a third spelling, and the two halves
/// of this packing agreeing is the only thing keeping them one format.
fn axis_sign(value: f32) -> f32 {
    if value >= 0.0 { 1.0 } else { -1.0 }
}

/// One word of [`NORMAL_FORMAT`] from one direction — the Rust twin of
/// `normal_format.wesl`'s `pack_normal`.
///
/// A zero vector means "no facing" and is the one input that is not a
/// direction; everything else is expected to be a unit vector, and the mapping
/// is octahedral — see `normal_format.wesl`, which carries the argument for it
/// once.
///
/// **The two sides are compared by rendering, not by reading.**
/// `two_mesh_faces_carry_their_own_two_normals` hands the mesh pass a normal,
/// reads the plane back and asserts the word equals this function's — so the
/// gate is an integer computed by the GPU against an integer computed here,
/// with no tolerance and nothing in between. That is what a hand-rolled `f16`
/// encoder could not have offered.
pub fn pack_normal(normal: [f32; 3]) -> u32 {
    let [x, y, z] = normal;
    let sum = x.abs() + y.abs() + z.abs();
    if sum == 0.0 {
        return NORMAL_DRAWN;
    }
    // Onto the octahedron, which is what makes the two components below add up
    // to a known total and therefore recoverable from each other.
    let (x, y, z) = (x / sum, y / sum, z / sum);
    // The lower half folds outwards into the corners of the square.
    let oct = if z < 0.0 {
        [(1.0 - y.abs()) * axis_sign(x), (1.0 - x.abs()) * axis_sign(y)]
    } else {
        [x, y]
    };
    // Clamped before the scale rather than after: the fold is exact in
    // principle and a rounding error of one bit at a corner would otherwise
    // produce a code past the mask, which wraps rather than saturates.
    let step = |value: f32| {
        let steps = (value.clamp(-1.0, 1.0) * 0.5 + 0.5) * NORMAL_AXIS_SPAN;
        (steps.round() as u32) & NORMAL_AXIS_MASK
    };
    NORMAL_DRAWN | NORMAL_FACING | step(oct[0]) | step(oct[1]) << NORMAL_AXIS_BITS
}

/// And back: the unit vector a word names, or a zero vector for a fragment with
/// no facing. `normal_format.wesl`'s `unpack_normal`.
///
/// A word nothing drew answers zero as well — a reader that must tell those two
/// apart tests [`NORMAL_DRAWN`], exactly as one that must tell an unlit
/// fragment from an unreached one matches on the variant rather than on the
/// number.
pub fn unpack_normal(word: u32) -> [f32; 3] {
    if word & NORMAL_FACING == 0 {
        return [0.0; 3];
    }
    let axis =
        |steps: u32| f32::from(u16::try_from(steps).expect("fifteen bits")) / NORMAL_AXIS_SPAN * 2.0 - 1.0;
    let x = axis(word & NORMAL_AXIS_MASK);
    let y = axis((word >> NORMAL_AXIS_BITS) & NORMAL_AXIS_MASK);
    // The third component is what the octahedron's own equation leaves: it is
    // negative exactly on the folded half, which is how the fold is undone
    // without a bit saying it happened.
    let z = 1.0 - x.abs() - y.abs();
    let [x, y] = if z < 0.0 {
        [(1.0 - y.abs()) * axis_sign(x), (1.0 - x.abs()) * axis_sign(y)]
    } else {
        [x, y]
    };
    let length = (x * x + y * y + z * z).sqrt();
    [x / length, y / length, z / length]
}

/// The attachments of one frame, at one size.
///
/// Created with the render target and dropped with it: these are the size of
/// the world image, which changes on a resize and a zoom step and on nothing
/// else. See [`Gbuffer::views`] for what a pass is actually handed.
#[derive(Debug)]
pub struct Gbuffer {
    /// What drew each pixel and which row it came from — [`IDS_FORMAT`].
    ids:      wgpu::Texture,
    /// Where that pixel's fragment is, exactly — [`POSITION_FORMAT`].
    position: wgpu::Texture,
    /// And which way its surface looks — [`NORMAL_FORMAT`].
    normal:   wgpu::Texture,
}

impl Gbuffer {
    /// Create the set at a render target's size.
    ///
    /// Here rather than in the caller for the reason
    /// [`depth_texture`](crate::renderer::depth_texture) is: a plane's format
    /// is this crate's decision, and one created with another fails at
    /// pipeline-bind time with an error that names neither side.
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        Self {
            ids:      plane(device, "ids", IDS_FORMAT, width, height),
            position: plane(device, "position", POSITION_FORMAT, width, height),
            normal:   plane(device, "normal", NORMAL_FORMAT, width, height),
        }
    }

    /// The views a pass attaches and a reader binds.
    ///
    /// A fresh set per call rather than a field: a view is cheap to make, and
    /// one cached inside the struct that also owns the textures is a borrow
    /// with no good answer the moment a caller wants both.
    pub fn views(&self) -> Views {
        Views {
            ids:      self.ids.create_view(&wgpu::TextureViewDescriptor::default()),
            position: self.position.create_view(&wgpu::TextureViewDescriptor::default()),
            normal:   self.normal.create_view(&wgpu::TextureViewDescriptor::default()),
        }
    }

    /// The id plane itself, for a test that copies it back and decodes a
    /// pixel — which is the only way to know a producer stamps the bits it
    /// claims to, rather than merely drawing something plausible.
    pub fn ids(&self) -> &wgpu::Texture {
        &self.ids
    }

    /// The position plane itself, on the same terms: the phase's own "done
    /// when" is a test that reads this back and finds the world position the
    /// mesh pass computed, to the float.
    pub fn position(&self) -> &wgpu::Texture {
        &self.position
    }

    /// The normal plane itself, on the same terms again: the other half of the
    /// phase's "done when" is a test that reads this back and finds the
    /// geometry's own normal, and nothing derived from a stance.
    pub fn normal(&self) -> &wgpu::Texture {
        &self.normal
    }
}

/// One view per plane of a [`Gbuffer`], borrowed by a pass for as long as it
/// runs.
#[derive(Debug)]
pub struct Views {
    /// [`Gbuffer::ids`]'s.
    pub ids:      wgpu::TextureView,
    /// [`Gbuffer::position`]'s.
    pub position: wgpu::TextureView,
    /// [`Gbuffer::normal`]'s.
    pub normal:   wgpu::TextureView,
}

/// One texel of a G-buffer, written by hand.
///
/// Two things build a G-buffer without drawing a frame: [`crate::plan`]'s
/// diagnostic pictures, which say "every pixel is flat ground on the tile above
/// it" and light that, and the fixtures in `tests/` that upload an attachment
/// they chose rather than rendering sprites into one — which is what lets a test
/// about the lighting exist without a client install and without art.
///
/// Before this they each spelled the planes out: a `[u16; 4]` with the kind in
/// two bits and the fraction in fourteen, a packed height beside it, and — the
/// day a second plane arrived — a position that had to agree with all of that.
/// Three copies of one format, and the failure mode is not a compile error but a
/// picture of a surface nobody would have drawn. This is the format once, taking
/// the numbers a producer actually knows.
///
/// **The tile is here and the id is not**, which is the one asymmetry and it is
/// the caller's shape rather than this type's: a fixture describes a surface by
/// where it stands, and the id is a row number it can only hand out once it has
/// seen every fragment it means to draw. So the tile is a field and the id is
/// [`Fragment::ids`]'s argument — see [`crate::plan`]'s own two passes over its
/// texels for what that looks like.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Fragment {
    /// Which tile the thing drawn here stands on.
    pub tile:   (u16, u16),
    /// Where in that tile, each axis in `0..1` — [`crate::place::Stance`]
    /// decides what the pair *means*, and this is the number itself.
    pub sub:    (f32, f32),
    /// How high, in `z` units.
    pub z:      f32,
    /// What drew it.
    pub kind:   crate::place::Kind,
    /// Which way its surface looks.
    pub stance: crate::place::Stance,
    /// **Which solid of the grid it is a point of**, or [`None`] for a point of
    /// none — the ground, a mobile, a surface a fixture has no grid behind.
    ///
    /// A field and not a derivation, unlike [`Fragment::normal`]'s stance table:
    /// the real pass reads it off the box its view ray met and there is nothing
    /// about *where* a fragment is that says which primitive it belongs to. It
    /// is the same fact [`crate::light::Spot::solid`] carries on the CPU side,
    /// and a fixture that means to compare the two walks has to state it once
    /// for both — which is exactly what it could not do while this rode in a
    /// cell scan keyed on an owner.
    pub solid:  Option<crate::occlusion::SolidId>,
}

impl Fragment {
    /// The one word of [`IDS_FORMAT`]'s texel, given the row this fragment's
    /// picture came from. See [`pack_ids`].
    pub fn ids(self, id: u32) -> u32 {
        pack_ids(id, self.stance, self.kind)
    }

    /// The four floats of [`POSITION_FORMAT`]'s.
    ///
    /// The *unquantised* pair, which is the whole point of the plane: what the
    /// place attachment rounded to a hundred-and-twenty-seventh of a tile and a
    /// sixteenth of a `z` unit, this states outright — and the solid in the
    /// fourth channel, [`pack_solid`].
    pub fn position(self) -> [f32; 4] {
        [
            f32::from(self.tile.0) + self.sub.0,
            f32::from(self.tile.1) + self.sub.1,
            self.z,
            pack_solid(crate::occlusion::SolidId::word(self.solid)),
        ]
    }

    /// The one word of [`NORMAL_FORMAT`]'s texel. See [`pack_normal`].
    ///
    /// Derived from the stance rather than carried as a fifth field, and **the
    /// real pass stopped doing that at `docs/lighting_rebuild.md` phase 6c**:
    /// `statics.wesl` writes the face of the box a fragment's own view ray met
    /// now, not a table read off the stance. This is a *fixture's* producer —
    /// `plan.rs`'s diagnostic pictures and the scenes in `tests/`, which have no
    /// boxes and state a surface by naming it — so it keeps the table, with
    /// [`crate::place::Stance::normal`]'s own doc for what that table gets wrong
    /// and why it does not matter here.
    ///
    /// The one place the two part company is **land**, which carries
    /// [`crate::place::Stance::Flat`] and answers no facing here — where
    /// `ground.wesl` writes the bilinear patch's own normal per fragment.
    /// That is not this method being wrong: a fixture states a surface by its
    /// stance and a stance cannot hold a hillside's direction, which is
    /// `docs/lighting_rebuild.md`'s own backlog entry about the CPU's four
    /// fixed normals, seen from the fixture side. What a fixture gets is level
    /// land's answer with the facing bit clear rather than `(0, 0, 1)`, so a
    /// parity scene lights it from every side — which is what it did before
    /// this plane existed at all, and what keeps a margin from moving for a
    /// reason that is not under test.
    ///
    /// The day a *fixture* wants a surface this enum cannot name — a hillside, a
    /// billboard rounded off by phase 7 — this grows a field and the derivation
    /// goes.
    pub fn normal(self) -> u32 {
        match self.kind {
            crate::place::Kind::Land => NORMAL_DRAWN,
            _ => pack_normal(self.stance.normal()),
        }
    }
}

/// Create one plane of the set — all three share every flag, and naming them
/// once is what keeps a fourth from arriving with a usage the others have and
/// it does not.
fn plane(
    device: &wgpu::Device,
    label: &str,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width:                 width.max(1),
            height:                height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            // So a test can read a fragment's position back and compare it
            // against the position the pass that drew it computed.
            | wgpu::TextureUsages::COPY_SRC
            // And so a fixture can write one: the parity frames upload a
            // G-buffer they built by hand rather than drawing sprites, which
            // is what lets a test about the lighting exist without an install
            // and without art. See `crate::place::texture`, which says the
            // same of its own plane.
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`ATTACHMENT_BYTES_PER_SAMPLE`] is the sum of what the attachments
    /// actually cost, and that sum fits WebGPU's guaranteed minimum.
    ///
    /// The number is a device limit this crate asks an adapter for, so getting
    /// it wrong is not a wrong picture: it is `request_device` refusing on one
    /// machine, or a pipeline failing to create with an error that names a total
    /// and no attachment. Summed from wgpu's own table rather than from the
    /// widths a person reads off the format names — WebGPU charges for
    /// alignment, and an `Rgba8Unorm` picture costs eight bytes and not four,
    /// which is exactly the sort of thing nobody guesses right.
    ///
    /// The second assertion is the one that changed direction. It used to read
    /// `total > floor` and say "if a plane is ever shrunk back under the floor,
    /// this is the line that says so" — which is what happened, so it says the
    /// opposite now: the set fits every adapter, and a plane added or widened
    /// past the floor has to argue for itself here rather than arrive.
    #[test]
    fn a_g_buffer_costs_what_it_says() {
        let total = attachment_bytes_per_sample();
        assert_eq!(total, ATTACHMENT_BYTES_PER_SAMPLE);
        assert_eq!(required_limits().max_color_attachment_bytes_per_sample, total);
        assert!(
            total <= wgpu::Limits::default().max_color_attachment_bytes_per_sample,
            "the G-buffer asks an adapter for more than WebGPU guarantees",
        );
    }

    /// An id word round-trips: the three things put into it come back out, and
    /// none of them reaches into another's bits.
    ///
    /// The layout is a contract with `place_format.wesl`, which no compiler
    /// reads, so what can be pinned on this side is that the four functions
    /// here are consistent with each other and that the fields are disjoint.
    /// The case worth the test is the **top** one: an id is shifted six bits
    /// left, so a row number near the ceiling is exactly where a field would
    /// start losing its high end silently — a wrong picture rather than a
    /// fault. Since `docs/silhouettes.md`'s two edge bits took the top of the
    /// word, that ceiling has a neighbour above it, and the case worth the test
    /// is that the two do not reach into each other.
    #[test]
    fn an_id_word_holds_three_things_and_gives_all_three_back() {
        use crate::place::{
            Kind,
            Stance,
        };

        for id in [0, 1, 4095, IDS_ID_MASK] {
            for stance in [Stance::Upright, Stance::Flat, Stance::MeshFace] {
                for kind in [Kind::Land, Kind::Static, Kind::Mobile] {
                    let word = pack_ids(id, stance, kind);
                    assert_eq!(ids_id(word), id, "{id}/{stance:?}/{kind:?}");
                    assert_eq!(ids_stance(word), stance as u32, "{id}/{stance:?}/{kind:?}");
                    assert_eq!(ids_kind(word), Some(kind), "{id}/{stance:?}/{kind:?}");
                    assert_eq!(ids_edges(word), 0, "{id}/{stance:?}/{kind:?}");
                    // And the edges on top of a full row, which is the pair the
                    // narrowing was for: neither field may read the other's bits.
                    for edges in [IDS_EDGE_ART, IDS_EDGE_BOX, IDS_EDGE_ART | IDS_EDGE_BOX] {
                        let marked = word | edges;
                        assert_eq!(ids_edges(marked), edges, "{id}/{stance:?}/{kind:?}/{edges:#x}");
                        assert_eq!(ids_id(marked), id, "{id}/{stance:?}/{kind:?}/{edges:#x}");
                        assert_eq!(
                            ids_stance(marked),
                            stance as u32,
                            "{id}/{stance:?}/{kind:?}/{edges:#x}"
                        );
                        assert_eq!(
                            ids_kind(marked),
                            Some(kind),
                            "{id}/{stance:?}/{kind:?}/{edges:#x}"
                        );
                    }
                }
            }
        }
        // And the invariant every reader's first branch rests on: a pixel
        // nothing drew and a pixel stamped as nothing are the same word.
        assert_eq!(pack_ids(0, Stance::Upright, Kind::Nothing), 0);
        assert_eq!(ids_kind(0), Some(Kind::Nothing));
    }

    /// A fragment's normal comes off its stance, except for land, which has
    /// none — the pair `blit.wgsl` used to make with a `select` on the kind and
    /// the producers make now.
    #[test]
    fn a_land_fragment_has_no_facing_and_a_floor_does() {
        let at = |kind, stance| {
            Fragment {
                tile: (100, 200),
                sub: (0.25, 0.75),
                z: 3.5,
                kind,
                stance,
                // A point of no solid: this is about the normal plane, and
                // nothing here has a grid behind it to name one from.
                solid: None,
            }
            .normal()
        };
        // The ground and a rug lying on it are the same stance and the same
        // shape of pixel, and only one of them is gated by which side of its own
        // plane a flame is on. See `ground.wesl`, which has the measurement.
        let land = at(crate::place::Kind::Land, crate::place::Stance::Flat);
        assert_eq!(land & NORMAL_FACING, 0, "a fixture's land claims a facing");
        assert_eq!(unpack_normal(land), [0.0; 3]);
        let floor = at(crate::place::Kind::Static, crate::place::Stance::Flat);
        assert_eq!(unpack_normal(floor), [0.0, 0.0, 1.0], "a floor does not look up");
        // A billboard has no side, and says so with a clear facing bit — which
        // is why the two states have a bit each and are not one zero vector.
        let body = at(crate::place::Kind::Mobile, crate::place::Stance::Upright);
        assert_eq!(body & NORMAL_FACING, 0, "a body claims a facing");
        // And all three were drawn, which is the other bit and the thing that
        // separates any of them from a pixel nothing touched.
        for (word, what) in [(land, "land"), (floor, "a floor"), (body, "a body")] {
            assert_eq!(word & NORMAL_DRAWN, NORMAL_DRAWN, "{what} reads as undrawn");
        }
    }

    /// Every direction survives the octahedral packing to within a fraction of
    /// a degree, and the ones this renderer actually writes survive it exactly.
    ///
    /// Two claims, and the second is the one worth the constant: **a cardinal
    /// normal round-trips bit-for-bit.** Every wall face, every lid and every
    /// level tile is one of six vectors, so an encoding that moved them by a
    /// ten-thousandth would be moving nearly every fragment in a frame for
    /// nothing — see [`NORMAL_AXIS_SPAN`], whose evenness is what buys it.
    ///
    /// The first is a bound rather than an equality, and it is tied to what a
    /// frame can show rather than to the mapping: at `0.01°` the cosine a
    /// shading term multiplies by moves by under `2e-4`, which is a twentieth
    /// of one step of a 255-level channel. The worst measured over the sweep
    /// below is **`0.0068°`**, so the bound is not a margin sized to fit.
    #[test]
    fn a_direction_survives_the_normal_packing() {
        // The angle between two nearly-parallel unit vectors, **from the chord
        // and not from the dot product**. `acos` of a dot is the obvious
        // spelling and it cannot measure this at all: near zero its derivative
        // is infinite, so a dot carrying `f32`'s own `1e-7` of error comes back
        // as `sqrt(2e-7)`, which is `0.026°` — larger than anything this test is
        // looking for. Measured: the first version of this test reported the
        // packing losing `0.028°` and the packing was not what it was reading.
        // The chord is well conditioned instead: subtracting two nearby `f32`s
        // is exact, and `2·asin(|a − b| / 2)` is the same angle.
        let angle = |a: [f32; 3], b: [f32; 3]| {
            let chord: f64 = (0..3)
                .map(|i| {
                    let d = f64::from(a[i]) - f64::from(b[i]);
                    d * d
                })
                .sum::<f64>()
                .sqrt();
            2.0 * (chord / 2.0).clamp(-1.0, 1.0).asin().to_degrees()
        };
        // The six a producer in this crate actually writes: `Stance::normal`'s
        // four faces, a lid, and level land's own.
        for cardinal in [
            [1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, -1.0],
        ] {
            assert_eq!(
                unpack_normal(pack_normal(cardinal)),
                cardinal,
                "a cardinal normal did not come back exactly: {cardinal:?}",
            );
        }
        // And a sweep over the whole sphere, including the folded lower half
        // and the diagonals where the octahedron's own edges are — which is
        // where a fold spelled two different ways would show.
        let mut worst = 0.0f64;
        for i in 0..97 {
            for j in 0..89 {
                let theta = std::f64::consts::PI * f64::from(i) / 96.0;
                let phi = std::f64::consts::TAU * f64::from(j) / 89.0;
                let direction = [
                    (theta.sin() * phi.cos()) as f32,
                    (theta.sin() * phi.sin()) as f32,
                    theta.cos() as f32,
                ];
                let length =
                    (direction[0] * direction[0] + direction[1] * direction[1] + direction[2] * direction[2])
                        .sqrt();
                let unit = [
                    direction[0] / length,
                    direction[1] / length,
                    direction[2] / length,
                ];
                let back = unpack_normal(pack_normal(unit));
                worst = worst.max(angle(unit, back));
            }
        }
        assert!(
            worst < 0.01,
            "the packing loses {worst}°, which a channel can show"
        );
    }

    /// The two bits above the direction are three states, and none of them is
    /// reachable from another.
    ///
    /// The case that matters is the middle one: "drawn, with no facing" is what
    /// a billboard writes, and if it decoded to a *direction* every tree in a
    /// frame would be lit from one side by an angle nobody measured.
    #[test]
    fn a_normal_word_says_which_of_three_answers_it_is() {
        let nothing = 0;
        let no_facing = pack_normal([0.0; 3]);
        let facing = pack_normal([0.0, 0.0, 1.0]);

        assert_eq!(nothing & NORMAL_DRAWN, 0, "the clear reads as drawn");
        assert_eq!(
            no_facing, NORMAL_DRAWN,
            "a billboard's word is more than the one bit"
        );
        assert_eq!(
            facing & (NORMAL_DRAWN | NORMAL_FACING),
            NORMAL_DRAWN | NORMAL_FACING
        );
        // Both of the first two answer the zero vector, and that is deliberate:
        // a reader telling them apart tests the bit, exactly as one telling an
        // unlit fragment from an unreached one matches on the variant.
        assert_eq!(unpack_normal(nothing), [0.0; 3]);
        assert_eq!(unpack_normal(no_facing), [0.0; 3]);
        assert_eq!(unpack_normal(facing), [0.0, 0.0, 1.0]);
    }
}
