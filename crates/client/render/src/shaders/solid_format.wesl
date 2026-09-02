// **Which solid of the frame's occlusion grid a fragment is a point of**, in
// the fourth channel of the G-buffer's position plane —
// `crate::gbuffer::POSITION_FORMAT`.
//
// The one number the shadow walk compares (`docs/render/design_model.md` phase 4):
// a candidate box whose id equals this is the very surface the fragment is a
// point of, and no part of a surface shadows another part of it.
//
// # Why it is carried rather than looked up
//
// `blit.wgsl` used to find it, per fragment, by scanning the fragment's own
// cell for a solid with the drawn static's owner and a shape its stance agreed
// with (`own_solid`). That is exact for everything that stands **one** shape per
// owner — a wall's panel, a floor's lid, a body's tile — and ambiguous for the
// one thing that does not: a fitted climbable stands one box per *tread*, every
// one of them `Edges::ANY` and all of them under one `Owner`, so the scan
// returned whichever tread the cell's list happened to hold first. Every pixel
// of a flight then claimed to be a point of its bottom step, and the treads
// above it shadowed themselves in hard triangular wedges.
//
// That was invisible while `docs/archive/render/gbuffer.md` step 4c's mesh pass drew over every
// climbable static, because a mesh face is one primitive by construction and its
// own row named it outright. `docs/render/design_model.md` phase 6d took that pass
// off real statics — a static's position and normal come from the impostor
// meeting its own boxes now — and the ambiguity was what the pass had been
// covering.
//
// So it is carried, and by the pass that genuinely knows: `impostor::Volume`
// has held the name of each box since phase 6c, `statics.wgsl` meets a view ray
// with exactly those boxes, and the box it lands on *is* the answer. This is
// also strictly better than the scan it replaces for a **corner** static, whose
// two panels the scan told apart by the resolved stance and this tells apart by
// the box the ray met.
//
// # The encoding
//
// A `SolidId` is three bytes (`crate::occlusion::SolidId`, and `blit.wgsl`'s
// `id_at` is why), so every real id is at most `0xFF_FFFF` — and `f32`
// represents every integer up to `2^24` exactly. The round trip is therefore
// lossless by construction rather than by tolerance.
//
// `SolidId::NOBODY` does **not** fit, and it does not need to: a negative
// channel is the whole of "a point of no solid at all", which is what the
// ground, a mobile, a gump and a static this frame's grid refused all write.
// `crate::gbuffer::pack_solid` and `unpack_solid` are the Rust twins.

// No solid at all — `crate::occlusion::SolidId::NOBODY`, the word rather than
// the channel. It cannot collide with a real id: an id carries in three bytes
// and this is strictly greater than all of them.
const SOLID_NOBODY: u32 = 0xFFFFFFFFu;

// And what that is as a channel. Any negative float decodes to `SOLID_NOBODY`;
// this is the one every producer writes, so a plane read by a person is a plane
// with one spelling of "nobody" in it.
const SOLID_NONE: f32 = -1.0;

// One id as the channel it rides in.
fn pack_solid(solid: u32) -> f32 {
    if solid == SOLID_NOBODY {
        return SOLID_NONE;
    }
    return f32(solid);
}

// And back. Negative is "a point of no solid"; everything else is exact — see
// the header for why the truncation cannot lose a bit.
fn unpack_solid(channel: f32) -> u32 {
    if channel < 0.0 {
        return SOLID_NOBODY;
    }
    return u32(channel);
}
