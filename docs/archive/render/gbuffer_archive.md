# G-buffer archive: reasoning, investigation trails, session history

Companion to [`gbuffer.md`](gbuffer.md) — that file is the current-state
reference (format, formulas, what is built, what is open); this file is the
full argument behind every choice in it: each decision's reasoning, every
"Not settled" item's investigation trail (including questions still open —
the trail so far is history even when the question is not closed), and the
session-by-session narrative of how the render side of the G-buffer was
built. Split out 2026-08-07 so the live doc could read as a plain reference
instead of a decision log. Nothing below was rewritten from the original
plan document — content was relocated under topic headings mirroring the
new doc's sections; wording is preserved.

Read [`gbuffer.md`](gbuffer.md) first for what is actually true today. Come
here for *why*, or for the mechanism behind a step or a backlog entry.

## Origin

Born from a dead end, not from a survey: [`lighting.md`](lighting.md) decision
40 needed a tread's own tilted normal to reach `blit.wgsl`, and `place::Stance`
had four bits and ten values already in them — no room, and no clean way to
make room, because the four bits were never a normal in the first place, they
were an index into ten hand-picked ones. Pushing on *why* the attachment is
shaped that way at all is what this plan is; decision 40's steps 4 and 5 are
now tracked here instead of there.

**Not a novel idea — an overdue one.** `lighting.md` decisions 36 and 38.3
already argued this, for the *occlusion* grid rather than the render
attachment: "the same box gives the shading half its answer for free: a
pixel's normal is the normal of the face it landed on, which is what
`place::Stance`'s nine values are a hand-rolled enumeration of" (36), and
`place::Stance`'s values are "a derived answer instead of a taxonomy to
extend" once a pixel's face comes from the same slab test occlusion already
runs (38.3). Its own backlog said outright that decision 22's one-sidedness
and `place::Stance` "should be *removed*... rather than carried alongside the
thing that replaces them" (backlog, "found while giving the world a spill",
lines 3676–3682) — and left it for whenever something forced the question.
Decision 40 is that something. This plan is that backlog item, executed for
the render side.

**Checked against `occlusion::Solid` before assuming it, not after:** the
struct exists (`crates/client/render/src/occlusion.rs:490`, at the time this
was written) but carried no `normal_of_face` or equivalent then — 36/38.3's
"for free" was the argument for building one, not a description of something
already there.

Written against `crates/client/render/src/place.rs`, `select.rs`, `light.rs`,
`statics.wgsl`, `ground.wgsl`, `mobiles.wgsl`, `blit.wgsl`, `renderer.rs`
(file names as they were before the WESL migration — see
`docs/lighting_raymarch.md`'s backlog for that move; the same files are now
`.wesl` sources).

### Where it stood, and why it was tight

[`lighting.md`](lighting.md) decisions 1–2 put a second colour target behind
every world pass: `place`, `Rgba16Uint`, `(x, y, z + 128, kind)` with the
stance riding in four spare bits of the third channel — the tile a pixel
belongs to, the height it was drawn at, what kind of thing wrote it, and which
way it faces. `blit.wgsl` reads it to light in world coordinates instead of
screen ones; `select.rs` reads it a second time, for the ground a selected
thing stands on.

Every bit of that channel word was spoken for except the four decision 40
wanted. Widening the format to make room for a real vector was not a fix, it
was the same design one texture format wider — the questions that follow were
about whether the payload should be shaped this way at all, not about finding
four more bits.

**Two things that payload could not do, independently of decision 40:**

- **It could not name what it came from.** `select.rs`'s own doc said so: two
  statics on one tile write the same tile, the same stance, the same height —
  the attachment could not tell them apart, which is why picking *which*
  static was never asked of it, and why the selected-thing wash needed a
  second, separate silhouette pass (`SpriteRenderer::render_mask`) just to
  know which one to shade.
- **It repeated per pixel what a whole sprite shares.** A wall's `(x, y)` is
  one tile — the same two numbers, written unchanged into every one of the
  maybe few thousand pixels its picture covers. Most of the channel's width
  was spent on a fact that was already known once, at the instance, before a
  single pixel of it was drawn.

### Picking does not use this buffer, and that was the control group

Worth stating because it looks like it should and does not: `statics::pick`,
`items::pick`, `mobiles::pick` (`crates/client/render/src/{statics,items,mobiles}.rs`)
answer "what is under the cursor" entirely on the CPU, by replaying each
candidate's own placement and testing the atlas for an opaque texel — no
readback of `place`, no GPU round-trip. `docs/client.md`'s M5 backlog notes
the four near-duplicate walkers this makes (`statics`/`items`/`mobiles`/`gump`)
as worth a common shape later; that is a separate, smaller cleanup and was not
blocking on anything here.

So `place` had exactly two readers — `blit.wgsl` and `select.rs` — and both
were about *what is here*, never *what did this*. That was the seam this plan
cut along.

## The place attachment — reasoning

**Decision 1 (the payload shrinks to a depth and an id, not explicit
attributes).** The algebra is the argument. The projection (`camera.rs`) is
affine and fixed — `screen_x = (x - y) * HALF_WIDTH`, `screen_y = (x + y - 1)
* HALF_HEIGHT - z * Z_STEP` — two equations in three unknowns. A screen pixel
alone names a *line* through world space, not a point: exactly one degree of
freedom is missing, no matter how simple or how constant the projection is,
because that is what an orthographic projection is — a camera direction
collapsed away on purpose. A single scalar closes the system; a full
`(x, y, z)` is two numbers more than the missing information actually
requires, once that scalar exists.

Which fragment's scalar survives is already decided before this payload is
ever written: the depth test each world pass already runs (`depth_state()`,
`LessEqual`) picks the one point along that line that is visible, the same way
it always has. The buffer's job was never to resolve *that* question — it
resolves the one algebra cannot: which object is the surviving fragment part
of. Hence two channels, not one and not five: a **depth**, and an **id**.

### Not settled — the depth texture's own reconstructability

~~Whether the existing depth texture (`Depth24Plus`, `renderer.rs:45`)
already carries a reconstructable value, or only an ordering key.~~
**Answered (step 1): only an ordering key.** Every world pass writes
`order.to_depth(base)` (`statics.rs:269`, `ground.rs:194`, `mobiles.rs:434`)
and nothing reads the depth texture back — `blit.wgsl` has no reference to
it at all, only to `place`. The value itself is not `z`: `Order` folds two
numbers into one key, `(tile - base) * DEPTH_PER_TILE + priority_z`, and
`priority_z` is not a world height — it is `z` bent by object-type-specific
adjustments (ground averages its corners and subtracts 2, a static shifts
±1 by two tiledata flags, a mobile adds 1) that only make sense once the
object's kind is already known. Two different world heights on the same
tile can fold to the same key — `depth::tests::
priority_z_can_collide_for_two_different_world_heights` pins a concrete
pair, a flat static at `z=5` and a wall at `z=4`, that produce the same
`priority_z` and hence the identical depth value — which a linear depth
worth reconstructing from could never do for two genuinely different
points. So: a linear world depth has to be written somewhere it is not
written now — decision 1 is not free. In exchange, decision 2 already pays
for part of what decision 1's algebra was reaching for: the id's
storage-buffer row can hold the instance's tile `(x, y)` directly, read
once per fragment. **Not `z` — step 3 found `z` is not instance-constant
for a standing face either, the same way decision 16's fraction below is
not; both are this fragment's own and both stay attachment-side.**

### Not settled — decision 16's fraction (`lighting.md`'s decision 16)

**Decision 16's fraction is likely free, but say so only once checked.**
`blit.wgsl` reads it to build `at`, the sub-tile world position the whole
lighting distance calculation runs on (`sub.x`/`sub.y` folded into `at.x`/
`at.y`, `blit.wgsl:1277-1281`) — it is not a separate ground-texturing fact,
it is the fractional part of the same position decision 1 already argues is
reconstructable. Solving decision 1's two equations for `x, y` given a depth
does not round to a tile; the fraction should fall out of the same algebra
for nothing, for any flat/billboard face. Worth confirming in step 1 rather
than assumed, and it does not extend to ground's bilinear case (the item
above) for the reason already given there.

**Step 1 answered, and it complicates this item rather than closing it.**
The depth that exists today is not the reconstructable position this
paragraph assumes (see the depth-texture item above) — a linear depth still
has to be written, and once it is, it names an object's own anchor, which
decision 2's `instances[id]` row already carries directly. Whether solving
for the fraction from a *new* linear depth is still worth doing, versus
reading it off the id's row the same way `(x, y, z)` would be, is a separate
question this step did not answer and step 2 or 3 should settle before
building either path.

**Step 2 settled the face-instance half as needing no fraction and no `z` at
all — step 3 found both wrong before either was built.** Traced against
`statics.wgsl` rather than assumed: a fragment's `z` is *not* the instance's
own height for a standing face — it is recomputed per fragment from screen
position (`z = base + ((sub.x + sub.y - 1) * HALF_TILE_HEIGHT - down) /
Z_STEP`), because that is what gives a wall a lighting gradient down its
face instead of one flat brightness. And the sub-tile fraction this
paragraph called ground's alone is computed for a static's face too, by the
same shader, for a reason stated in its own comment: "the next tile along
the run starts its fraction at 0 where this one ended at 1, so a row of wall
tiles is one continuous surface rather than a row of separately lit
sprites" — without it, `two_wall_tiles_in_a_row_name_one_continuous_surface`
is exactly what breaks. Both are genuinely per-fragment, not per-instance,
for every stance decision 3 gives an id to, flat or standing alike — a floor
static's picture spreads a fraction across its whole tile the same way
ground's does, not only a wall's.

**Only the *tile itself* — the integer `(x, y)` — turns out to be the
instance-constant fact decision 1's algebra was reaching for.** `z` and the
fraction stay exactly where they already were, computed the same way and
packed into the same bits of the attachment; only what used to occupy the
attachment's `x`/`y` channels is now an id, addressing the row below for
that one fact alone. Caught before landing rather than after: two
independent test failures (a parity-shader/CPU-reference mismatch, and a
raw-attachment assertion) both pointed at the same wrong assumption from two
different angles.

**The row itself, decided here rather than left implicit — and narrower
than first drafted:** an anchor's `x: u16, y: u16` alone (not `z`, see
above; `place.rs:219-237`'s third field stays attachment-side). `Stance`
does **not** move into the row either: a corner's *resolved* direction
(which half a pixel was drawn on) is still per-fragment, computed exactly
as it is today, and still has nowhere to live but the attachment. What
step 4 (corners) added past this paragraph's original claim is a second
`u32` field, `twin` — not a normal and not a `Stance`, but which *row* the
other half's id points at, so the two halves stop sharing one id without
either needing to carry the other's `Stance` or geometry. `kind` does
**not** ride in the row either — it is what selects *which* of the three
per-kind buffers above the id indexes into in the first place, so a row
stating it again would be exactly the repetition this plan's intro opens by
objecting to ("it repeats per pixel what a whole sprite shares"). What the
row removes is narrower than step 2 drafted, but it is still exactly the
repetition named there: two numbers, written unchanged into every one of a
sprite's pixels, now written once.

*(Historical note: this trail describes the id row as ending up "an anchor's
`x, y` alone, two bits of `Kind` selecting the buffer, thirty bits of index."
What actually shipped, confirmed against `place_format.wesl`'s `pack_place`
and `blit.wesl`, is very slightly different: `kind` was never carved out of
the id's own 32 bits — it continued to live in the attachment's fourth
channel exactly where it always had, and the shader branches on that
existing field to pick which buffer the *full*, unconstrained 32-bit id
addresses. The outcome — one shared, generously-wide id, `kind` deciding the
buffer — is the same; only the literal bit layout differs from this
paragraph's draft. See `gbuffer.md`'s own "The id and its storage buffers"
for the shipped shape.)*

## The id and its storage buffers — reasoning

**Decision 2 (the id indexes a storage buffer).** `lighting.md` decision
30.5 ruled storage buffers out for the occlusion grid under the crate's old
ceiling, WebGL2. That ceiling was no longer current — decision 30.5 was
reopened and answered: the crate's target became **WebGPU**, which has real
compute shaders and storage buffers, and `crates/client/render/src/lib.rs`'s
own module doc is the record of the change. `Occlusion`'s texture-folded
lookup (`LIST_ROW`, decision 38.4) was not undone — it stayed, running, as a
still-valid technique — this plan simply did not have to route around the
constraint that produced it, because that constraint no longer applied to
new work.

So: build the plain mechanism, `instances[id]` — no row-width bookkeeping, no
address arithmetic to keep in sync between the bake and the shader. If a
future target ever needs the older ceiling back, decision 30.5 has the
compression already worked out (a texture and `textureLoad`, "a dozen lines,
not a millisecond") as a known, cheap fallback — not a reason to pay for it
then.

Instance data was already uploaded to the GPU — `statics.wgsl`'s
`@location`-tagged vertex attributes are a per-instance buffer, just one the
GPU can only walk in draw order, not address at random. A storage buffer is
the same memory bound a second way: `blit.wgsl`'s fragment stage reads the id
out of the attachment and does `instances[id]`, an arbitrary lookup a
vertex-attribute binding cannot do. No second draw call and no second upload —
one buffer created with both `VERTEX` and `STORAGE` usage, bound once more, to
a different stage.

Everything the four spare bits and the fixed `outward()` switch used to encode
— which tile, which way it faces, what kind of thing this is — becomes a field
of that one row, read once per fragment instead of decoded from a packed word.

Three buffers, not one — ground's row and a face's row hold different data
(ground has no normal to carry, a face has no corner heights), so `instances`
is really `ground_instances`, `face_instances` and `mobile_instances`, and
`Kind` (unchanged from today's two bits, `place.rs:58-74`) is what the id
channel carries to pick which — see the id-width item below for the bit
budget and the measured counts behind it.

### Not settled — the id's width and the buffer's capacity

~~The id's width and the buffer's capacity, i.e. how many faces one frame's
storage buffer is sized to hold. Not measured against how many faces a real
screen ever holds...~~ **Answered (step 2):** measured, at the same frame
`docs/lighting.md`'s 25,702/17,201-statics numbers were taken from —
Britain, widest zoom, `Cutaway::OPEN` (nothing hidden, the worst case) —
by extending `tests/cost.rs` to print the two `collect()` calls it already
builds but never counted (`cost.rs:319-347`):

```
27889 ground quads, 6560 static quads (431 of them corners,
so 6991 faces once decision 3 splits each in two)
```

Ground is not face-decomposed by decision 3 — a land quad is one face today
and stays one — so its count needs no correction. A static's does: 431 of
6,560 carry a corner `Stance` (`place::STANCE_CORNER`, `place.rs:160`) and
decision 3 splits each into two faces, so 6,560 objects become 6,991 faces.
Treads are not in this number — nothing decomposes a tread into top+riser
yet (`occlusion.rs:1358-1378` still pushes one body per tread, not two
faces at the time of this measurement; see decision 3's own body above for
why that gap was the render side's to close, in step 4) — but a flight of
stairs is a small, bounded class of statics next to 6,560 of them, and even
a generous doubling of every tread in view would not move this number by an
order of magnitude. Mobiles are not in it either, for a different reason:
they are population, not map data, so a snapshot of Britain's furniture
cannot measure them — but every mobile-or-worn-layer quad is already exactly
one face by construction (`Stance` never varies by corner or tread for
`Kind::Mobile`), so no multiplier applies there at all, only a raw count
bounded by however many characters a shard ever draws on one screen, which
is server population and an order of magnitude below the map's own
furniture in any arrangement this client has ever drawn.

So: **one shared 32-bit id channel**, not per-kind widths to track. Two
bits carry `Kind` — unchanged from today's values (`place.rs:58-74`,
`Nothing=0, Land=1, Static=2, Mobile=3`) — and select which of three
storage buffers (ground quads, static/item faces, mobile-and-layer quads)
the remaining 30 bits index into. Thirty bits is not a number chosen to
feel safe; it is chosen because a per-kind id, each counted separately,
needs nowhere near it — the widest real frame measured needs 27,889 for
one kind and 6,991 for another — and one width for all three kinds avoids
the "row-width bookkeeping... to keep in sync" decision 2 already refused
to pay for, extended from *one* buffer to three. If a future city or a
crowded event needs more of one kind than Britain ever showed this client,
the ceiling is 1,073,741,823 away, not a few thousand.

*(As shipped: the two `Kind` bits were never actually carved out of the id
channel's own 32 bits — see the note at the end of the decision-16 trail
above. The measured face counts and the conclusion "no width problem exists"
both still hold; only the literal bit layout differs from this paragraph.)*

The **buffer's capacity** — the initial allocation, not a cap; decision 2's
storage buffers resize the same way today's vertex buffers already do
(`renderer.rs:491-495`, `1029-1031`) and never drop a frame's worth of
instances — is a different question, and this measurement answers it too.
Today's `INITIAL_QUADS = 4096` (`renderer.rs:37`) is not sized against
reality: a widest-zoom frame at Britain already asks for 6.8x that many
ground quads and 1.6x that many static quads, so **the very first frame
drawn at this location reallocates**, on every run, before a single pixel
is on screen. Worth carrying into step 3 rather than repeating: seed each
kind's storage buffer from the measured count with one power-of-two of
headroom, the same rounding `renderer.rs`'s own grow path already uses —
ground at 32,768, static/item faces at 8,192, mobiles left at today's 4,096
for lack of a map-density argument to move it, since population is a
server number this measurement cannot speak to.

**This last recommendation — per-kind seeded capacities — was never
implemented.** `renderer.rs`'s `INITIAL_QUADS = 4096` remains one flat
constant shared by every buffer; see `gbuffer.md`'s Status section for this
as a standing, current gap rather than a step still to be scheduled.

### Steps — building the storage buffers

- [x] **1. Confirm or replace what the depth channel carries.** Confirmed:
      only an ordering key, never read back. See `depth::tests::
      priority_z_can_collide_for_two_different_world_heights`.
- [x] **2. Design the face-instance row's layout and the id's width/buffer
      capacity against a real frame's face count, not object count.**
      Measured at Britain, widest zoom: 27,889 ground quads, 6,560 statics
      (6,991 faces once corners split). One 32-bit id channel, `Kind` in
      two bits selecting one of three per-kind storage buffers, 30 bits of
      id — see the answered item above for the row's fields and the
      buffer-capacity numbers for step 3.
- [x] **3. Wire the storage buffer.** Dual-usage (`VERTEX` + `STORAGE`)
      buffer creation, the second bind group on `blit.wgsl`'s fragment
      stage, `instances[id]`. Landed narrower than step 2 drafted, for the
      reason under decision 2 above: only a static's or a mobile's **tile**
      (`x`, `y`) moves to the row — `z` and the sub-tile fraction are
      genuinely per-fragment for a standing face, not per-instance, and
      stay exactly where and how they were computed, in the attachment.
      `SpriteRenderer`'s own instance buffer is the row (`STORAGE` added to
      its existing `VERTEX` usage, no second upload), addressed by
      `@builtin(instance_index)`; `Stance`'s resolution stays
      attachment-side too, since a corner's is per-fragment. Ground was
      untouched at this step, as planned. `outward()` itself did not
      change at all.

## Corner faces — reasoning

**Decision 3, first half (an id names a face, not a game object — and the
normal is honest per-face geometry, not a fitted formula).** Decision 40's
problem dissolves twice over, not once. The first way: a normal is
per-*instance* data once the id exists, and an instance budget is thousands
of entries, not a screen's worth of pixels, so there was never a reason to
fit it into a code. The second, sharper way: the *tread itself* stops being
approximated by one surface standing in for a shape it is not.
`Prism::tread_normal`'s blend — mixing `Surface::Flat`'s `[0, 0, 1]` toward
`Surface::Face(up)`'s horizontal normal by the tread's own rise over run —
exists to make one flat top read like the ramp a flight of them actually is.
That is a proxy for geometry the occlusion grid already has and does not need
approximating: a tread is a box with up to two faces a camera ever sees — its
top, and the riser between it and the tread before it — and a lid static
under the flight contributes its own top where no tread covers it. This
session's stair is 3 treads × 2 faces + the lid's own top: **seven honest
normals**, none of them blended.

So the id names a **face**, not a game object. A wall's sprite has one
relevant face and collapses to exactly what it is today — one id, no
change. A tread has up to two; a corner has exactly two. Each is
rasterised separately, by the invisible geometry pass decision 4 below
adds, and each carries its own trivial, axis-aligned normal — `[0, 0, 1]`
for a top, the climb's own outward direction for a riser — with no
blending arithmetic anywhere. A corner static gets this for free: its two
faces become two separate face-instances, rasterised and depth-tested
independently, which is what `statics.wgsl`'s screen-half resolution stands
in for today — decision 25's exact arithmetic, `right = FaceNorth +
(offset >> 1)`, `left = FaceSouth + (offset & 1)`, `stance >=
STANCE_CORNER` picking which pixels ask the question at all.

**Step 4 found "for free" was two different claims, true for different
reasons.** A wall needed nothing: `Stance::FaceNorth/East/South/West` was
already the exact normal, so "one id, no change" above is the whole story
for it. A corner was not free in the same way — its normal was already
honest too (the `across`-test resolves the correct `Stance` before the
attachment write, so `blit.wgsl`'s lighting was never wrong for a corner),
but both halves of its picture still shared **one id**, so nothing could
address "the corner's north-ish half" apart from its south-ish one. The fix
landed as identity, not geometry: `sprite::split_corners` gives a corner's
drawn row a second, undrawn row past the frame's real instances, sharing
the same tile, and `statics.wgsl`'s existing `across > 0.0` test picks which
of the two ids a pixel's half writes to the attachment — `SpriteQuad::twin`,
not a second rasterised triangle. No invisible geometry pass was needed for
this case; see the mesh-faces section below for why a tread is different.

**This also reopened a settled claim, not just a bit-packing choice — say
so plainly rather than let a reader trip over the contradiction.** Decision
30's fourth bullet and decision 38.3 both state, as a closed question, that
occlusion geometry stays out of the render path: "the G-buffer is still the
bridge from a pixel to a world surface... a solid is consulted by the light
and by the normal, and never by the rasteriser" (38.3) — the separation was
called "exactly why the freedom is affordable" for occlusion's own shape.
This plan did not touch the rasteriser (the visible sprite draw stayed
untouched), but it did put honest per-face geometry behind the G-buffer's
*normal*, which 38.3 filed under "the light and... the normal" already, so
the two decisions were not actually in conflict — 38.3 drew the line at the
rasteriser, and this plan stayed on the same side of it.

**Settled by step 5 — honest per-face lighting fixes decision 40 on its
own.** Decision 40 existed because a single flat top read wrong next to a
nearby light — a hard-edged cliff in `cone` across the first tenth of a
tread's climb. That cliff turned out to be a property of the sampling that
went looking for it, not of the real geometry — see the mesh-faces section
below for the measurement. `Prism::tread_normal` was deleted rather than
ported.

### Not settled — did the face-instance table already exist on `Solid`?

~~Whether the face-instance table is its own texture or a field `occlusion`
already has reason to grow.~~ **Answered (step 4, corners): neither — there
is no table to share.** Checked directly rather than assumed: a wall's face
normal was never missing in the first place — `place::Stance::FaceNorth/
East/South/West` already *is* the honest, exact per-face normal, and
`blit.wgsl`'s existing `outward()` already reads it correctly. Traced why:
`Stance::of` and `occlusion::edges_of` both derive from one shared
measurement, `facing::facing_of`, taken once when the atlas packed the
sprite (`Sprite::facing`) — not two independent guesses that happened to
agree. So decisions 36/38.3's "for free" was true on arrival for a single
face, and there was never a `normal_of_face` to build on `Solid`, on this
plan's own table, or anywhere else, because the fact already existed
upstream of both. What *was* missing, and is a different question from the
one this bullet asked, is a corner's two faces sharing one id — see above.

### Steps — corners and walls

- [x] **4a. Walls and corners — the two cases that turned out not to need an
      invisible geometry pass at all.** Solid-sharing "Not settled" question
      answered first, as planned: there is no table to share, because a
      wall's normal was never missing (`Stance::FaceNorth/East/South/West`
      already is it, both derived from one shared `facing::facing_of`
      measurement, `occlusion::Solid` never consulted). A corner's normal
      was likewise already honest per-fragment; what it lacked was
      identity — both halves of one picture wrote one shared id. Fixed with
      no new pass, no new pipeline, no new shader file:
      `sprite::split_corners` gives a corner's drawn row a second, undrawn
      row sharing its tile (`SpriteQuad::twin`), and `statics.wgsl`'s
      existing `across > 0.0` test — the same one that already resolved the
      correct `Stance` — now also picks which of the two ids a pixel's half
      writes to the attachment.

      Caught before landing, not after: growing `SpriteQuad` by one field
      also grows `blit.wgsl`'s mirror of it, and WGSL's storage-buffer
      alignment rounds that struct's size up to 64 bytes regardless of the
      Rust side's raw 52 — two parity tests
      (`the_shader_and_light_sample_agree_about_a_wall_that_faces_away` and
      its `_surface_that_looks_up` sibling) failed on a channel mismatch at
      a tile past the first until `SpriteQuad::STRIDE` padded to match.

### Found while scoping step 4a — the shared corner arithmetic between statics and items

A corner static's shared arithmetic (`crate::statics::place`, `Placed`) is
used by both the map's own furniture and `crate::items` — found while
scoping step 4a, not assumed. An item can carry a corner `Stance` the same
way a map static can (both go through `statics::place`), and
`app/src/lib.rs` appends `items::collect`'s quads to the map's before
either is drawn — after `statics::collect`'s own sort, not before it. So
`split_corners` has to run on the *merged* list, at the call site in
`app/src/lib.rs`, not inside `statics::collect` itself — an id-per-face
scheme built only against map statics would have silently left a
corner-shaped item's two halves sharing one id, the exact bug this step
exists to fix, just for a narrower class of object. `sprite::split_corners`
is written generically over `Vec<SpriteQuad>` for this reason, not as a
method on `statics::collect`'s own return type. (This holds true in the
shipped code, `crates/client/app/src/lib.rs:4464` — see `gbuffer.md`'s
"Corner faces" section.)

## Mesh faces — reasoning

**Decision 3, second half — a tread's shape.** See the "Corner faces —
reasoning" section above for "seven honest normals"; the argument for a
tread's own top-plus-riser decomposition and for treating a face as
per-instance data is the same one that produced the corner fix, applied to
a shape a single id genuinely cannot state.

**Decision 4 (the invisible geometry pass stays depth-consistent with the
visible one).** A face-decomposed proxy is a second, parallel description of
the same objects the sprite passes already draw — not a new source of truth
about what the world contains. If a rug's sprite covers a floor's on screen,
the proxy pass has to agree the rug's face wins the same pixels, or lighting
answers about a surface nothing shows. Concretely: the proxy pass draws from
the same instance list and the same depth ordering the visible passes
already use, decomposed to faces, so a fragment's winning id is always the
one the picture agrees is on top.

**Step 4a found this decision describes fewer cases than it reads as
describing.** A wall needed no proxy at all (its normal was already exact).
A corner needed a second id, not a second, parallel description of its
geometry — the *existing* fragment already carries the depth and the pixels
this decision worries about keeping consistent; `across > 0.0` is the same
test either way, so there was no second draw to keep in step with the
first. What this decision is actually the argument for is a tread's top and
riser, which really are two surfaces at two different screen positions with
no existing fragment test to reuse — see step 4b/4c below.

**Decision 5 (geometry-agnostic on purpose).** Nothing above assumes the
surface is a flat billboard. A depth and an id are exactly what a rasterised
triangle of real geometry produces too — the reconstruction does not care
whether the id's row describes a sprite's tile or a mesh's material, because
"what was here" was never answered by the shape of the thing, only by what
its own data says. [`client.md`](../../client.md)'s later milestones bring real
geometry into this client; this plan was written so that day does not
reopen the G-buffer's shape a second time. The alternative this plan does
*not* take — deriving a billboard's exact world position from its screen
row and a closed-form offset, which is true today and stops being true the
day a sprite is a mesh — was considered and dropped for exactly that
reason.

### Steps — occlusion decomposition and the mesh render pass

- [x] **4b. Treads, the occlusion half.** `occlusion.rs` decomposes a tread
      into its top and riser instead of one whole-tile body — the
      representation the render pass (step 4c) needs to walk. `Builder::
      add`'s climbable branch (`occlusion.rs:1402-1451` at the time) pushes
      two `Solid`s per tread instead of one: a thin lid at the tread's own
      height (`edges: 0`, the same rule an ordinary floor's lid already
      uses) and a panel spanning the rise from the tread before it, named
      `up`'s opposite edge (`edges: opposite(edge_of(up))`, the same rule a
      named-edge wall panel already uses). Nothing about *how* a lid or a
      panel stops a ray is new — decision 3's "seven honest normals" is
      exactly this shape, minus the render side. `Solid::tread_box_of` is
      retired; the climb-axis footprint math it shared with nothing is now
      `Solid::strip_footprint`, used by both `tread_top_box_of` (a strip)
      and `tread_riser_box_of` (a single boundary, degenerate on the climb
      axis).

      **Caught before landing, not after: `Solid::footprint()`'s `far`
      adjustment assumed every degenerate "far" plane sits on the tile's
      true integer boundary.** True for every panel `Solid::box_of` had
      ever built — never true for a riser past the first tread, whose
      boundary (`index / count`) is a proper fraction. Unpatched,
      `footprint()` would have walked a mid-flight riser one tile into its
      neighbour, silently wrong for `bake`'s spill (decision 38.2) the
      first time a stair sat near a frame's own edge. Fixed by gating the
      `-1` on `min.fract() == 0.0`, which is unconditionally true for every
      existing caller and false only for the new fractional case —
      `occlusion.rs::tests::a_mid_flight_risers_footprint_stays_on_its_own_tile`
      pins it.

      **Deliberately stopped here.** The render pass — pipeline, shader,
      projecting these boxes' corners to screen quads, writing depth+id —
      was step 4c, not this one; decision 38.5's own discipline against
      changing where geometry lives and what it is in the same step.

- [x] **4c. Treads, the render half.** Rasterise depth+id for each face
      step 4b's occlusion grid carries — a tread's top and riser, a lid
      static's own top — from the same instance list and depth ordering
      the visible passes already use (decision 4), the way 4a's corner
      id-split kept step in sync with the picture it split. What step 4b
      hands it: six honest boxes per 3-tread stair (a lid per top, a panel
      per riser) instead of three bodies, ready for something to walk and
      project — building that something, and deciding how a face's *id*
      (not its occlusion box) gets from a `Solid` to a storage row a
      shader can index, was this step's own work, not inherited from 4b.

      **Landed as a second, invisible pipeline**
      (`renderer::MeshFaceRenderer`, `mesh_face.wgsl`), not a variant of
      `SpriteRenderer`'s: a `Mesh` face's true screen shape is an arbitrary
      projected quadrilateral, not the axis-aligned rectangle
      `statics.wgsl` instances, so the new pass draws raw,
      CPU-triangulated vertices (`crates/client/render/src/mesh.rs`'s
      `Face::fan`, `0,1,2,0,2,3`) instead. It writes only `place` — no
      colour target at all, the same shape `SpriteRenderer::render_mask`
      already uses for a pass that ignores `target.view` — so the
      billboard sprite's own picture is untouched; only that sprite's
      `place` pixels get a more honest per-face normal than
      `Prism::tread_normal`'s blend gave them. Depth is the enclosing
      static's own `SpriteQuad::depth`, reused rather than recomputed —
      decision 4's "a second copy of the formula is a second chance to
      disagree with it," restated for depth instead of a lighting
      fraction.

      **The id scheme did not grow `Kind`.** `Kind` is a hard 2 bits,
      already spoken for (`Nothing/Land/Static/Mobile`); a mesh face stays
      `Kind::Static` and a new `Stance` value, `MeshFace = 10` (10-15 were
      free), is a routing *sentinel* in the attachment's stance bits —
      `blit.wgsl` sees it and reads a new, small `mesh_instances[id]`
      (tile + the face's *real* stance) instead of `face_instances[id]`.
      The real stance is one of `Flat`/`FaceNorth/East/South/West`,
      because `Prism::mesh` only ever produces those five exact normals
      today (`place::Stance::of_normal` maps a `[f32; 3]` back to one), so
      `blit.wgsl`'s existing `outward(stance)` gives the normal unchanged
      — the general packed-vector question stayed open, on purpose,
      because nothing built here needs more than an axis-aligned normal.

      **This also answered, for a mesh face, the question the per-face
      normal-format item below left open for a *sprite's* face — the
      sub-tile fraction and per-fragment `z`.** Neither a constant
      approximation nor a restated copy of `statics.wgsl`'s per-stance
      analytic inversion was needed: each vertex carries its own true
      world position (in addition to its projected screen position), and
      because the projection is affine and every face is planar, the
      rasterizer's own linear interpolation gives every fragment an
      *exact* world position for free — `sub = fract(world.xy)`,
      `z = world.z`, both exact, neither approximated nor re-derived.
      Cheaper and more correct than either option considered going in.

      **Not done in this step, on purpose (at the time):** `items.rs`
      (server-dropped ground items) did not get mesh-face collection wired
      in — only `statics::collect`'s map-furniture walk did, though
      `Placed.prism` was already available to `items.rs` too, unused. No
      frame-test exercised the new pass directly either. **Both have since
      been closed** — `items.rs::collect` now builds mesh vertices/rows for
      any placement carrying `Placed::prism` (`items.rs:104-153`), and
      `tests/frame.rs` gained direct pixel-decode coverage for the mesh
      pass (`a_mesh_face_pixel_carries_the_mesh_face_sentinel`) plus a full
      shader/CPU parity suite exercising `mesh_face.wgsl`'s own vertex/
      fragment path — see `docs/lighting_raymarch.md` for that work. See
      `gbuffer.md`'s current "Mesh faces" section for the shipped state.

- [x] **5. Checked against the same reproduction that found decision 40** —
      `examples/isolated_scene.rs`'s profile mode, the `1497,1626,10` stair,
      `(1497.5, 1626.83, 16)` to `(1497.5, 1626.17, 20)` — and **honest
      per-face lighting fixes it on its own; `Prism::tread_normal` is
      retired rather than ported.**

      **The hard cliff decision 40 found was never a property of the real
      geometry — only of the sampling that went looking for it.** That
      reproduction's segment linearly interpolates `z` from `16` to `20`
      across the whole flight as if it were one continuous ramp, and
      samples `Surface::Flat` (a single fixed tag) along it — a proxy for a
      shape the three real treads never were. Re-run at each tread's own
      real, constant height instead — `Surface::Flat` at `(1497.5, y, 16)`
      for `y` inside the low tread's own real footprint
      (`1626.667..=1627.0`), and likewise for the other two — `cone` is not
      a cliff at all: a flat `0.273` across the whole of the lit low
      tread's top (every sample to three decimals, nine points spanning
      its real width), and fully occluded (`through`: "stopped at (100,
      100)") on the two treads above it, which cannot see the lamp past
      the flight's own geometry — a real, correct occlusion cutoff, not a
      facing artifact, and exactly what a real step next to a light it
      partly blocks its own view of should look like.

      **The blend was compensating for the fake ramp, and now costs
      accuracy it does not buy back.** Sampled at the exact same point
      (`1497.5, 1626.83, 16`, the low tread's own real coordinate),
      `Surface::Sloped(tread_normal(0))` reads `cone: 0.715` against
      honest `Surface::Flat`'s `0.273` — nearly three times as bright,
      because `tread_normal(0)` was never close to `[0, 0, 1]`: even the
      first tread's rise (from the lid's own base) tilts it hard towards
      `up` to make the *fake ramp's* far end land smoothly at `0.000`.
      That tilt is a real distortion once every tread gets its own true
      position and its own true, unblended normal from `Prism::mesh` —
      nothing is left for the blend to correct that measuring the real
      geometry does not already correct for free.

      **Retired, not ported:** `Prism::tread_normal` and its tests
      (`facing.rs`), `light::Surface::Sloped` and `Spot::sloped`
      (`light.rs`), and the `isolated_scene` profile tool's `tread` mode
      and its `_TREAD_UP`/`_TREAD_HEIGHTS` env vars, which existed only to
      drive it. `outward()`'s switch — the shader-side case that would
      have had to grow to decode a `Sloped` normal had this gone the other
      way — was never built at all, so there was nothing there to remove.

### Not settled — the per-face normal format for anything not axis-aligned

**The per-face normal format for step 4c/5 cannot be the fixed
`Stance`-shaped set (`Flat`/`Face(N/E/S/W)`) decision 3 assumed for treads.**
Reopened mid-session-4b: `docs/lighting.md` decision 35's rejection of
sloped roofs ("a roof in this client is a slab five z deep... it would not
buy the thing it looks like it buys") is no longer settled — inclined faces
for roofs, land, and future custom geometry are wanted, for the flexibility.
A face's normal has to be a general vector from the start, not the six-value
enum decision 3's axis-aligned tread faces would have gotten away with. A
packed encoding (an 8-bit octahedral normal or similar was suggested) is the
likely shape — not committed to a bit layout here, the same discipline
decision 2's id-width item followed: measure against a real consumer before
picking a width, and there is no render-side face row to measure against
yet. Purely a step 4c/5 question — nothing about step 4b's occlusion-grid
work (`Solid.edges` selects a shadow-test formula, never a rendered normal)
changed for it at the time.

**Fence removed (2026-08-07).** The occlusion-grid side named as untouched
above is reopened too, in [`lighting_geometry.md`](lighting_geometry.md) —
a mesh occluder's *silhouette* (not just its shading normal) is now wanted
for the same curved-roof case this item names. This item's own scope (the
render-side normal format) stays exactly what it says; the sentence that
used to fence it off from occlusion no longer holds. **This is a currently
open question** — see `gbuffer.md`'s Status section for its short factual
statement; nothing below has resolved it, and nothing has been committed to
a bit layout for it. What is now different is that both the render-side
question (this item) and the occlusion-grid question
(`lighting_geometry.md`) point at the same underlying need — a general
mesh-occluder primitive — so a session picking either up should read the
other first.

## Occlusion-side tread geometry — reasoning

Covered above under "Steps — occlusion decomposition and the mesh render
pass" (step 4b) and under decision 3/decision 4's reasoning — the render-side
motivation for the occlusion grid's tread decomposition is the same "seven
honest normals" argument; the DDA/raymarch mechanics that walk the resulting
`Solid`s are `docs/lighting_raymarch.md`'s own territory, not repeated here.

## Selection — reasoning

### Not settled — does `render_mask` shrink once the id can name a static?

~~Whether `SpriteRenderer::render_mask` shrinks.~~ **Answered (step 6): no —
checked directly rather than assumed, and the id does not close the identity
gap after all.** The id that lands in a static's `place.x`/`place.y` is
`@builtin(instance_index)` into the *main world draw's* own instance buffer
(`statics.wgsl`, `id = instance_index`), assigned by `statics::collect`'s
draw order over every static in the frame. The selected static's silhouette
is a second, tiny, unrelated draw — `SpriteRenderer::render_mask` over a
one-quad list `statics::selected` builds — with its own instance numbering
starting again at zero. There is no shared id space between the two, so a
pixel's id can never be compared against "the picked static's own id" to
answer "is this pixel it" — which is exactly what select.rs's own doc
already said ("the first cannot be answered from the second"), now
confirmed for the reason stated rather than assumed. `render_mask` keeps
both of its callers: `outline.md` D1's, unrelated and untouched, and
`select.rs`'s own, unchanged by anything this step found.

### Not settled — ground's own reconstruction (moot)

~~Ground's own reconstruction is not the billboard's...~~ **Answered (step
7): moot — `ground.wgsl` was never reconstructing anything to begin with.**
Checked directly rather than assumed: the vertex shader already evaluates
the bilinear formula once, exactly, at each of the four real corners
(`mix(mix(...), mix(...), ...)`, exact because it is only ever evaluated
where a mix reduces to one of its own inputs), and the rasteriser's own
linear interpolation of that per-vertex height and per-vertex sub-tile
fraction across the two triangles is the "free" part — the same trick step
4c's mesh faces use, arrived at independently and earlier, because ground
was never a billboard's flat plane to begin with. Both were already
fragment-side and untouched by this step, for the same reason step 3 found
`z` and the fraction fragment-side for a standing face: **only the tile
itself was ever the instance-constant fact worth moving.** So step 7 turned
out to be a narrower question than this bullet posed — not "how does ground
reconstruct a height", but "does ground's tile get the same
`ground_instances[id]` move step 3 gave a static's" — and the answer is
yes, unconditionally, no closed-form height solve required anywhere.

### Steps — selection and ground

- [x] **6. `select.rs`'s ground wash reads the id instead of tile/stance;**
      measure whether `render_mask` still has a reason to run for selection
      specifically once it does, separately from outline's own use of it.

      **A real bug, sitting since step 3 landed and never caught: `select.wgsl`
      still compared a static's `place.x`/`place.y` against a selection's tile
      directly, as if those channels still held the tile itself.** They have
      not since step 3 moved a static's tile off the attachment and onto its
      own instance row, addressed by id — `blit.wgsl` was updated to resolve
      it, `select.wgsl` was not, because nothing forced the question until
      this step went looking. The effect: the ground wash under a selected
      wall was silently comparing an id against a tile number, which either
      never matched (a wall's floor never washed) or matched by pure
      coincidence — and the existing GPU test (`tests/select.rs`) never caught
      it, because its own fixture still hand-wrote a literal tile into a
      `Kind::Static` texel, the same pre-step-3 encoding the real shader had
      already stopped producing. A fixture that pokes the attachment by hand
      is a second, unchecked implementation of what the world pass puts
      there — decision 9's own warning, landing again, for the same reason
      the backlog already named it once for `tests/frame.rs` and `plan.rs`.

      **Fixed the way `blit.wgsl` already had to be:** `select.wgsl` gains
      the same `FaceInstance` storage binding `blit.wgsl` reads as
      `face_instances` (the statics pass's own instance buffer, bound a
      second time), and resolves a `Kind::Static` pixel's tile through
      `face_instances[id].place` before comparing it against the selected
      tile — `Kind::Land` is untouched, exactly as it is in `blit.wgsl`,
      because ground never moved off the attachment. `mobile_instances` and
      `mesh_instances` are not needed: the ground wash only ever tests
      `kind == KIND_LAND || (kind == KIND_STATIC && stance == STANCE_FLAT)`,
      and neither a mobile nor a mesh face's routing sentinel
      (`Stance::MeshFace`) can satisfy that. A mesh face whose *real* stance
      (in `mesh_instances`, not the attachment) is `Flat` — a tread's or a
      lid's own top — is a real gap this leaves open, but an existing one
      and not a new one: the attachment's stance bits were always the
      sentinel for a mesh face, never its real stance, since step 4c
      landed, and nothing about the id/tile bug this step targets made
      that better or worse. Left alone on purpose rather than folded in
      here — decision 38.5's discipline, one thing at a time. **Still open
      today** — see `gbuffer.md`'s "Selection" section for the current
      statement of this gap.

      **The plumbing already existed at the one call site that matters:**
      `app/src/lib.rs` fetches `window.statics.instances_buffer()` a few
      lines above where it builds `blit::Frame` already, for exactly this
      buffer — handing the same reference to `select::Frame` cost one
      field, no new buffer, no new upload. `tests/select.rs`'s own fixture
      needed the larger change: its `place_texel` helper now takes the
      attachment's raw two words rather than a tile, `static_texel` packs
      an id into them the way `statics.wgsl` does, and a tiny
      `face_instances` buffer of one row (both static bands in the fixture
      stand on the same tile) is uploaded and bound alongside the place
      texture — the same shape `tests/frame.rs`'s `parity_frame` already
      adopted for `blit.wgsl`. All pre-existing assertions still hold with
      no numbers changed: the fixture was testing the *rule* correctly,
      only the encoding it exercised the rule through had gone stale.

      Four gates green on `openshard-client-render` and
      `openshard-client-app`, and the full `cargo test --workspace`
      besides: no count moved except the fixture's own internal encoding.

- [x] **7. Ground's bilinear case, once statics prove the shape works at
      all.** Landed the same shape step 3 gave a static's tile:
      `ground.wgsl` writes `@builtin(instance_index)` into the attachment's
      first two channels instead of a literal `(x, y)`, and
      `blit.wgsl`/`select.wgsl` resolve it through `ground_instances[id]` —
      the ground pass's own `GroundQuad` buffer, `STORAGE` added to its
      existing `VERTEX` usage, the same dual-purpose buffer decision 2
      described and step 3 built for statics. `z` and the sub-tile fraction
      are untouched — see the "ground's own reconstruction" item above for
      why neither was ever a fact this step had anything to move.

      **The row's own WGSL struct could not just mirror `FaceInstance`'s
      shape, and the reason is `GroundQuad`'s own byte layout, not a design
      choice.** `FaceInstance` gets away with declaring `rect: vec4<f32>`
      as its first field because `SpriteQuad::write` puts a `vec4`-sized
      value first too — but `GroundQuad::write` puts a bare `(x, y)`, eight
      bytes, first, and WGSL aligns a `vec4` field to sixteen bytes
      regardless of what a struct's byte-level contract actually needs.
      Declaring the row the same way `FaceInstance` does would have
      silently opened a gap after `origin` that `GroundQuad::write`'s real
      bytes do not have, and every field read after it — including
      `place`, the one this step actually needs — would come out four
      bytes short of where the Rust side put it. Caught before landing, by
      working out the alignment arithmetic rather than trusting the shape
      to carry over: `GroundInstance` is declared as sixteen scalars
      (`f32`/`u32`, each aligned to four bytes and never more) instead of
      the four/five `vec4`s `GroundQuad`'s own doc groups them into.
      `MeshFaceInstance` (step 4c) is the same discipline, arrived at for
      the same reason — a struct with no field wider than a scalar cannot
      open a gap its writer's bytes do not have — this step is the first
      to need it *and* say why, since `MeshFaceInstance`'s two `u32` fields
      never had a `vec4` sibling to collide with.

      **No change to `GroundQuad::write`, `GroundQuad::STRIDE`, or any of
      its existing byte offsets.** The scalar-struct fix above meant the
      vertex-fetch side and the storage-read side could keep disagreeing
      about how many attributes the buffer has without disagreeing about
      where anything *is*: `ground.wgsl`'s pipeline now declares five
      vertex attributes instead of six — the tile's two words stay in the
      buffer, at the same offset, simply unfetched by the vertex stage —
      while `blit.wgsl`/`select.wgsl` read those same two words back
      through the dual-bound storage view. One buffer, one upload, two
      different pipelines each reading only the part of it their own stage
      needs.

      **A real bug caught by the compiler before it was a bug in the
      picture: `every_pixel_names_the_tile_it_came_from`.** The one
      existing test that read the raw attachment and asserted a ground
      pixel's first two channels equalled its literal tile, `[300, 400]` —
      true before this step, and simply wrong after it, since those
      channels are an id now. Not a fixture that poked the attachment by
      hand (decision 9's own warning) — a fixture that read a *real*
      draw's real output and had old knowledge baked into the assertion.
      Fixed by decoding the id and checking it against the `GroundQuad`
      that produced it (`ground[id].place == Place::land(300, 400)`)
      instead of the attachment directly, the same move `render_places`'
      corner test already made for a static's id.

      **Four more fixtures joined the "hand-writes the attachment" list
      this backlog already tracks, on the ground side specifically:**
      `plan.rs`'s `drawn` (shared by `draw` and `pictures.rs`'s
      `elevation`), `tests/frame.rs`'s `parity_frame`, and
      `tests/select.rs`'s `scene` each needed a `ground_ids`/`ground_rows`
      map built alongside the pre-existing `face_ids`/`face_rows` one, the
      ground mirror of exactly what step 3 made each of them build for a
      static. Every real-GPU test in
      `tests/frame.rs`/`tests/cost.rs`/`examples/isolated_scene.rs` that
      draws actual `GroundQuad`s through a real `GroundRenderer` needed
      `ground_pass.instances_buffer()` threaded into its `blit::Frame` —
      previously always a dummy, since nothing indexed it — and the one
      compile error at a time the missing-field checker produced was the
      whole of finding them: no site was missed silently.

## Testing fixtures — a cross-cutting backlog item

Several backlog entries were about test infrastructure that touches every
topic above (place-attachment packing, corner splitting, ground rows) rather
than any single one, so they are collected here rather than split across the
sections above.

**Three test fixtures write the `place` attachment by hand, bypassing
`statics.wgsl` entirely, and step 3 had to teach each one the id scheme
separately.** `tests/frame.rs`'s `parity_frame` (the shader/`light::sample`
parity fixture) and `plan.rs`'s `drawn` (shared by `draw` and `elevation`,
so `pictures.rs`'s elevation tests too) both synthesize a `Kind::Static`
attachment texel-by-texel for coverage a real draw call can't easily give —
every sub-tile fraction, or a wall run several tiles long — and both now
build a small id-addressed row buffer alongside it by hand (deduped by
`(x, y)`, one row per distinct tile, `SpriteQuad::write`'s own layout
reused rather than re-derived). This is exactly the drift decision 9 warns
about for the *lighting* formula, one layer down: a fixture that pokes the
attachment's bits directly is a second, unchecked implementation of
whatever `statics.wgsl` does to fill them, and it went stale silently here
— `cargo test` caught it only because two unrelated assertions (a
shader/CPU-reference pixel mismatch, and a raw-channel equality) both
happened to depend on the part that broke. **Worth a sharper fixture
eventually**: a small helper that builds a `place` texture and its
`face_instances` row together from a list of `(Place, Stance)` pairs,
shared by both files, so the next repacking of this attachment has one seam
to update instead of three. Not done at the time — decision 38.5's
discipline against changing more than one thing in a step, and these two
fixtures were each still readable on their own. Still true after step 4a:
neither fixture's `place_of` closure ever asks for a corner `Stance`, so
both got a one-line `twin: 0` and nothing more — the sharper shared helper
would need to know about `twin` too, once it exists.

**A fourth fixture joined this list at step 6: `tests/select.rs`'s
`scene`.** It hand-writes the same attachment for the same reason (a
screen of texels no real draw call assembles conveniently) and now builds
its own one-row `face_instances` buffer beside it, the smallest version of
the same shape `parity_frame` uses. Confirms the pattern rather than
changing it — worth folding into the same sharper shared helper if one is
ever built, not a fourth reason to build it now.

**Step 7 doubled every one of these four fixtures' own bookkeeping rather
than adding a fifth.** `Kind::Land` needed the identical id-row treatment
`Kind::Static` already had, so `parity_frame`, `plan.rs`'s `drawn` and
`tests/select.rs`'s `scene` each grew a second `ground_ids`/`ground_rows`
map beside the `face_ids`/`face_rows` one — same dedup-by-tile, same
first-sight numbering, `GroundQuad::write` in place of `SpriteQuad::write`.
A shared helper built now would take a list of `(Place, Stance)` pairs *per
kind* and hand back both rows and both buffers in one call instead of four
call sites each writing the same eight lines twice — worth doing before a
fifth kind ever needs the same shape, not done at the time for the reason
it was not done at step 6: each fixture was still readable copying the
pattern rather than abstracting it, and decision 38.5's discipline is
against changing more than the one thing a step is actually about. **Not
known to have been revisited since** — treat as still-open test-debt if
picked up.

## Other backlog items

**Decision 3 said a tread's face count two different ways.** "A tread is a
box with up to two faces a camera ever sees — its top, and the riser" and
the worked example built on it (3 treads × 2 faces + the lid = seven) said
two; a later sentence in the same decision said "a tread has up to three."
Found while sizing step 2's id against decision 3's own numbers — the two
could not both be used for the same measurement. Fixed to "up to two,"
matching the paragraph the worked example is built on; if a tread ever
turns out to expose a third face (an end tread's open side, unhidden by the
next one), that is a discovery to make when it actually decomposes one, not
a number to carry forward from a sentence nothing else in the decision
agreed with.

**`blit.wgsl`'s own top-of-file comment (line 34) used to say the place
attachment is `(x, y, z + 128, kind)` for every pixel.** Fixed at step 7, on
the same trip through the file that gave `Kind::Land` an id too — it now
says `(id, z + 128, kind and the fraction)` and names both steps that put
an id there instead of a tile. Kept in this archive as the record of a
comment that drifted twice before anyone fixed it — the current comment is
correct and current.
