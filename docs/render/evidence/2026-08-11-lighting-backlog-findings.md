# Evidence — findings from the lighting rebuild, with their measurements

*Recorded 2026-08-11, lifted verbatim out of `design_model.md`'s backlog. Each
entry is a thing somebody noticed while building the model, with the number that
was taken for it. It is a record of findings, not a queue: the live, ranked queue
is [`../README.md`](../README.md) and what is unbuilt is in
[`plans/render/lighting/PLAN.md`](../../../plans/render/lighting/PLAN.md).*

## Backlog

Things noticed while writing this, not blocking any phase:

- ~~🚩 **Nothing gates that an instrument writes what a world pass writes, and the
  field it got wrong is the one no picture shows.**~~ **Built: `tests/attachment.rs`,
  three tests, each fault-injected to red.** The claim it states is the one nobody
  did — *a row that draws a static this frame's grid holds names the occluder the
  grid holds it as* — and what made it statable is that a `plan::Picture` now
  carries the rows it was drawn from (`plan::Named`). A picture could not be asked
  what attachment it came from, which is exactly how `plan::elevation` stamped
  `OwnerId::NONE` into every row for three phases with two green tests over it.

  **The world-pass half is a round trip and not an equality.** `items::collect`
  calls `owner_at` itself, so comparing its row against `owner_at` would be the
  code agreeing with itself; the gate goes the other way instead — take the number
  the row carries, look it up in the grid's own list for that tile, and require the
  solid it lands on to be the very static that was drawn. That is only *reachable*
  on a tile holding two occluders, since on a one-solid tile every number in range
  resolves to the same static. So the fixture is two scenes: the wall run, and
  `storey_over_a_torch`, whose ring tiles carry a wall at `z 0` and another at
  `z WALL_HEIGHT`. The test counts both — rows examined, and rows on an ambiguous
  tile — because a census that examined nothing passes.

  **The two questions, answered.** *`Kind` is enough* to say which writers must
  ask, among the passes that write a G-buffer: `Kind::Static` is exactly that set.
  The two passes that write `Kind::Static` beside a bare `NONE` — `statics::selected`
  and `items::outlined` — are out of the claim rather than exceptions to it, because
  the silhouette pipeline's vertex layout declares no `place` attribute at all
  (`renderer.rs`: *"a silhouette has no hue and no place"*), and a row that reaches
  no attachment cannot be wrong about one. And the ground's `GroundQuad` is the
  **honest exception**: `occlusion::place` is only ever handed statics and ground
  items, so no land tile is ever a solid, and an owner field there could only ever
  hold `NONE` — which is a field a later writer gets wrong for free.

  🔴 **And a third instrument was carrying the defect's shape, unread.**
  `plan::draw`'s `owner_of` was a constant `OwnerId::NONE`, correct only because
  `drawn` asks for an owner where it builds a *face* row and a plan view builds
  none. That is the same sentence as the bug that shipped: a constant that is right
  until something reads it, in a field no pixel shows. It is `unreachable!` now, so
  a plan view that grows a static stops instead of quietly drawing a fragment that
  is a point of nothing. Found by the injection that failed to go red — the first
  version of the plan-view test asserted `owner == NONE` and was a tautology
  against a hardcoded constant.

  **Still not gated, and named so it is not assumed:** `MeshFaceRow::solid`, the
  mesh half of the same join. No built scene has a climbable — `scene.rs` mentions
  neither prism nor climbable — so there is no synthetic route through
  `items::collect` that produces a mesh row at all. It wants a scene before it can
  want a test. `mobiles.rs`, `text.rs` and `gump.rs` are unasserted on purpose:
  their `NONE` is honest by kind, and the claim above does not reach them.
- ~~🚩 **The impostor's *normal* is the whole of what 6c did to a sprite's
  shading, and it was measured by injection rather than argued.**~~ **Done
  2026-08-11**, by the candidate this entry itself named. The record, then the
  answer and what it cost.

  A person
  reported a static reading darker and striped where it used to be even, and
  three frames of one place (Britain `(1497, 1627)`, `View::Light`, a lamp post
  added by hand) say which half of 6c it is: the commit before 6c draws the post
  fully lit and the flight beside it in broad even bands; HEAD draws the post
  black and the flight cut into dark stripes; and HEAD **with `best.normal`
  forced to the zero vector and the position left alone** is the pre-6c picture
  again. So the position half is innocent and the cosine against a box's
  camera-facing face is the whole difference. What that face *is* is worth
  stating plainly: `Meeting::normal` is always one of `+x`, `+y`, `+z`, so every
  sprite fragment claims to look towards the camera, and a flame standing behind
  that plane — including a lamp's own, inside its own box — reads `N · L ≤ 0`.
  `View::Normal` over the same place shows it directly: a lamp post's pole comes
  out split down the middle into a green half and a red one, which is a whole
  tile's box answering for a picture of a thin pole. **The black emitter entry
  above and this are one finding**, and the candidate this suggests and nobody
  has measured is that a *body* — `edges == EDGE_MASK`, the box for a graphic
  whose art would not name a side — should write **no facing** rather than a
  face, keeping the measured normal for the panels and lids where the art really
  does say which way a surface looks. It is a different answer from the three at
  the black-emitter entry and should be judged beside them, not instead.

  **That is what landed.** `impostor::Volume` carries the box's own `Edges` now
  — free, in the alignment word after `lo` that this side had to write anyway,
  beside the `solid` that already rode in the one after `hi` — and `statics.wesl`
  writes `pack_normal(vec3(0.0))` where the mask is `EDGES_ANY`. The **stance**
  is still taken from the met face, deliberately: it names which face of which
  box the ray left by, which `blit.wesl`'s `on_the_lit_surface` and `own_solid`
  read as geometry, and that is a fact whether or not the art drew a plane
  there. The facing is the only thing this refuses to claim.

  *Why it is not an exemption.* It is a measurement that was never taken, said
  so — `normal_format.wesl`'s middle state, the one `blit.wesl` lights from
  every side. The same sentence is already written one module over:
  `light::mounted_at` refuses to move a flame off an `Edges::ANY` cell because
  "there is no direction in it to move along and a guess would be a wrong one".
  The impostor was making exactly that guess, one pass along.

  *Measured, on the frame the defect was reported from* — `isolated_scene` at
  Britain `(1497, 1626, 10)`, radius 6, a lamp post by hand, `640×480`,
  `View::Lit`, against the same frame with `EDGES_ANY` set to a mask no box can
  carry (which is the injection, and it is also the positive control below):
  **31,375 of 307,200 pixels change, 10.21% of the frame**, worst channel step
  `168` of `255`. On the one-item scene the entry above reproduces at, over the
  8,064 pixels the lamp's own picture covers: the mean brightest channel goes
  `4.3 → 17.5`, and the share at or under `8` of `255` — black, to a person
  looking — goes **82.7% → 39.1%**. The pictures either side are the acceptance
  instrument and they say it plainly: a black silhouette with a green wick
  becomes a lit lamp, and the flight of steps beside it stops being striped.

  *What holds it.* Three gates, each red under the injection above.
  `grids.rs`'s `the_statics_pass_knows_which_mask_means_the_art_named_no_side`
  pins `EDGES_ANY` from the shader's own source (`docs/render/design_pixel_spaces.md` rule 6).
  `frame.rs`'s `a_sprite_pixel_meets_the_same_box_on_both_sides` now sweeps a
  fixture whose masks are the ones the grid would hand those shapes — two treads
  and a lid — and asks both halves of the rule of every texel of the quad, with
  the *face* still compared everywhere through the stance so the CPU-against-GPU
  claim about `meets`'s arithmetic is untouched. And
  `a_sprite_fragment_is_a_point_of_the_primitive_it_names` reads its axis out of
  the stance rather than the normal, and counts both populations, so a scene
  that drifted to all bodies or to none could not pass it by never reaching it.

  *What it does not fix, named so nothing claims it.* A body is lit from every
  side, so a crate has no shading across its own faces — that is the pre-6c
  picture for exactly the set 6c had no measurement for, and what would improve
  on it is a *measured* facing, not a better guess. **A climbable's tread is
  swept up with them and should not be**: `boxes_of` hands a tread `Edges::ANY`
  to pick an occlusion test, and a tread's lid and its riser are planes the art
  did draw — one field with two domains, filed and measured in its own entry
  above ("A climbable's tread is marked a body"). And the emitter's own
  remaining darkness is `FLAME_LIFT`: half a tile, whatever the sprite's height,
  so a lamp post's pool is centred at its **foot** and its head — nineteen `z`
  up — takes the far end of an inverse square. See the entry below.
- 🟡 **A flame burns half a tile up whatever it is standing in, so a tall
  emitter's own head is the dimmest part of it.** `FLAME_LIFT` is `Z_PER_TILE /
  2`, and its doc argues the number honestly — a brazier's flame is about there,
  and "the sprite's real height is not available here, and asking the atlas for
  it would tie the lights to whether this frame's art happened to be packed".
  What the fix above made visible is the cost on a *tall* one: a lamp post's
  picture is seventy-six pixels of sprite, nineteen `z` units, and its lantern is
  at the top of it while the flame burns at `5.5`. On the one-item scene the pool
  is plainly centred on the post's **foot** — the base is the brightest thing in
  the frame and the lantern head takes the far end of an inverse square. It is
  not a defect in the model and it is not the black emitter: it is a light placed
  from the map's `z` alone, on a sprite whose height nothing in `light::gather`
  can see. The two candidates are the ones the entry above rejected *for that
  defect* and which are still live for this one: read the flame's own height off
  the art (which wants the atlas in the light collector, and is the same
  measurement `MOUNTED_CLEARANCE`'s doc asks for), or off `calc_height`, which is
  in reach today and is the item's own height rather than its flame's.
- **"Vertical steps along the tiles" is reported and not reproduced.** Named by a
  person at Britain `(1459, 1693)` beside the ragged silhouette above, in the
  live client. `examples/isolated_scene` at that place, with a lamp post added by
  hand and every knob at its default, does not draw them: the wall's lit face is
  smooth across its tile boundaries and the ground has no tile-shaped step in it
  at all. What differs between the two pictures is what the tool cannot yet
  build: the client's carried lantern is a **beam**, its ambient may have the
  **sky field** on — which is per tile and interpolated nowhere, the first
  candidate for anything tile-shaped — and the knobs may be off their defaults.
  Pinning it wants the client's own `View::Normal` of the same frame, which
  separates a geometry answer from a walk answer, plus the tab's numbers.
- 🟡 **Light comes through a floor at the *corner points* between its tiles, and
  it is one line of `primitive_stopped`.** **The hole is closed and the report is
  not explained** — the two halves came apart when the fix was measured, and
  what follows the original entry says which is which. Seen from under a ceiling
  at
  Britain's `(1492, 1642)`, `z 28` under statics at `z 40` and `z 23`: a regular
  lattice of bright dots, one per tile **corner**, and nothing along the joins
  between them. The lattice is the tell — a leak along a join is an interval
  problem and a leak at a join's *point* is a degenerate one.
  <br>
  The rule is read rather than guessed. `primitive_stopped`'s lid arm asks
  `ray_vs_solid` for the run the ray spends inside the lid's **horizontal
  footprint** (an infinite-`z` box) and hands `crosses` the `z` at the two ends
  of that run. A ray threading the exact corner of the footprint enters and
  leaves it at one `t`, so those two `z`s are **the same number** — and
  `crosses`, which asks whether the ray went from one side of the lid's plane to
  the other, correctly answers "it did not", because over an interval of zero
  length nothing travels anywhere. Every one of the four lids sharing that
  corner answers the same way, so the point is a hole through a continuous
  floor. Along a join *line* the ray still crosses the interior of one footprint
  on the other axis, `entered < leaves`, and it is caught — which is exactly the
  shape a person sees.
  <br>
  **The fix is to state the lid rule directly rather than as an interval.** A lid
  is a plane: pierce it once — `t = (lid_z − from.z) / delta.z` — and ask whether
  `(x(t), y(t))` is inside the footprint, inclusively. At a corner that test says
  *yes*, which is the honest answer for a point interior to the floor as a whole;
  a ray running along the lid's own plane has no `t` in `(0, 1)` and stays
  unblocked, which is the strictness `crosses`'s doc argues for (a candle
  standing on the floor it lights). It is also **smaller** than what it replaces:
  one intersection instead of a slab test plus a crossing test, and the
  `-1.0e6`/`1.0e6` sentinel box goes with it. `light::crosses` is the CPU twin
  and moves with it; the gates are `tests/lighting.rs`'s floor scenes and
  `scene::storey_over_a_torch`, which is the fixture the *opposite* defect (a
  floor stopping nothing at all) was found on.
  <br>
  **Done 2026-08-10, and smaller still than the entry proposed.** The pierce
  does not have to be computed: `primitive_stopped` has already run
  `ray_vs_solid` against the lid's own box — footprint *and* `z` span — and
  returned early when it missed, so *did the ray meet this lid* is answered
  before the arm begins. What was left was *did it get from one side to the
  other*, and that is `crosses` over the **segment's own two ends**. One line in
  each twin, the sentinel box gone from both. The gate is
  `light::tests::a_ray_through_the_point_four_floor_tiles_share_is_stopped_by_
  them` — four floor tiles of four graphics (a merged floor is one primitive
  with no interior corner to leak at), a fragment over one and a flame under the
  one diagonally opposite, so the segment's midpoint *is* the shared corner.
  Confirmed to have teeth by fault injection: the old arm makes it read
  `streaming 1, exact 1` where it now reads `0`.
  <br>
  🚩 **What is not confirmed is that this is what the person saw**, and the
  measurements say so plainly. A sweep of 40,000 fragments across four tiles of
  a floor standing in the shadow of a storey ten `z` above it, at
  `FLAME_RADIUS`, leaks **nothing** — under the old arm as much as the new one,
  which is why that sweep is not in the tree: a gate that cannot fail is worse
  than none. And `examples/isolated_scene` at `1492,1642,28`, with a flame added
  above the `z 40` lid, renders **byte-identical** in `View::Light` under the two
  arms. Both readings say the same thing: the corner case is a set of measure
  zero for an ordinary ray, so it cannot on its own paint one dot per corner over
  a floor. Either the lattice has a second cause — the impostor naming one of
  four coplanar lids at a fragment that sits exactly on their shared corner is
  the nearest candidate, and it is a 6f-shaped question — or the arrangement that
  produces it is not the one reconstructed here. **What the next attempt needs
  is the frame**: the camera and the light the person actually had, since the
  coordinates alone did not rebuild it.
  <br>
  **And that is what it was — the impostor, not the walk.** The person reported
  it a second time with the tile they were standing on (`1492, 1643`, stand
  `z 20`), `isolated_scene` there reproduced the lattice at once, and reading the
  G-buffer settled it in one row: at a bright pixel `View::Shadow` and
  `View::Height` read **exactly what its neighbours read** and `View::Normal`
  does not. The dots are not lit more; they are *facing* differently. Fourteen
  of them, on a lattice of exactly `TILE_WIDTH` — one per tile corner — each
  carrying a side face's normal on a surface whose every other pixel carries
  `+z`, at `z ≈ 40`: a lid. A wall's cosine in the middle of a roof. See
  `impostor::meets`.
- 🚩 **A corner still lights up where a lid meets a side face, and it is the
  *shadow* term there rather than the face.** The same person, same roof, two
  tiles over: `1510, 1636` and `1490, 1636`, stand `z 20`, ceiling `z 40`, the
  roof `0x051C`. Reproduced in `isolated_scene` at both — two or three clusters
  of three to six pixels, not the one-per-corner lattice the entry above was.
  <br>
  A nine-by-nine window of the G-buffer round the brightest one reads like this,
  and it is the whole diagnosis: the lower half is `+z` and lit and *dark*
  (a lid facing up, with the cosine giving it nothing), the upper half is `+x`
  or `+y` and **shadowed**, and along the seam between the two there is a notch
  of five pixels that are `+x`/`+y` and **lit** — a side face's cosine, at full
  flame, against neighbours of the same normal and the same `z` that the walk
  calls shadowed. So it is not which face was met: the face is the same as its
  neighbours', and the walk answers differently for it.
  <br>
  What that leaves is the exemption. `on_the_lit_surface` releases a candidate
  whose extent along the fragment's own normal axis **ends** on the fragment's
  plane, and a seam is exactly where a lid's edge and a wall's face share a
  coordinate — so a fragment there is released from the very primitive that
  shadows its neighbours. Measured at the other spot: the bright pixel names a
  different solid from the pixels below it (`0` against `15` and `26`), which is
  the same sentence from the other end.
  <br>
  **What it needs is the instrument this class keeps asking for and the tree
  does not have: which primitive a pixel names, as a picture.** Four defects on
  this track have now been "the fragment names the wrong box" (6f, 6h, the lid
  face, this), and each was diagnosed by hand-decoding the position plane's
  fourth channel through a throwaway shader edit. A `View::Solid` beside
  `View::Normal` — the id hashed to a colour — plus a per-pixel *who stopped the
  ray* probe would have made each of them minutes' work.
  <br>
  ✅ **`View::Solid` landed, and it answered on its first frame.** The whole
  `+x`/`+y` region above the seam is a point of **no primitive at all**, while
  the `+z` surface below it names one. The chain from there is short and every
  link is in the tree already:
  <br>
  `occlusion::opacity` reads a graphic's own flags — `NO_SHOOT` is opaque,
  `WINDOW` is a pane, **everything else is `CLEAR`** — and `Builder::add`
  returns without pushing anything at all for a `CLEAR` one. So this roof piece
  stands nothing in the grid: it is not geometry, it stops no light, and
  `Occlusion::id_of` has no name to give it. `statics::push_volumes` still hands
  the impostor a `boxes_of` box for it — that is 6c's deliberate fallback, since
  the grid refuses about half of Britain's drawn pictures and the alternative is
  a billboard — so the fragment gets a measured position and a measured *face*
  while being a point of nothing. And `boxes_of` reads a picture with no `FLOOR`
  flag through `edges_of`, as a **wall**: side faces, at the tile boundary, at
  the very height the roof lies at.
  <br>
  So the glow is three facts meeting. The pixels are the picture's overhang past
  that box — a *miss*, taking the nearest face, which along a silhouette is a
  side one. A side face has a real cosine towards a flame the roof's own lid has
  none of. And being a point of nothing they are exempt from nothing, so which
  of them the neighbouring lid shadows is decided by where the clamp put them on
  its edge — five pixels clear it and blaze.
  <br>
  **And the client's own files name the pieces**, which settles what is a defect
  here and what is data. `tile_probe` on the tiles round the glow:
  <br>
  - `0x051C` "stone pavers", `z 40`, `FLOOR|NO_SHOOT|PLATFORM` — the surface the
    person calls the roof. A lid, opaque, **in the grid**: that is the `+z`
    region, naming its solid.
  - `0x00C8`/`0x00C9` "stone wall", `z 20`, height 20, `WALL|NO_SHOOT|BLOCK` —
    the wall under it, in the grid.
  - `0x00DD`/`0x00DE` "stone wall", `z 40` and `z 43`, height 3, `WALL|BLOCK`
    and **no `NO_SHOOT`** — the wall's top course, standing at exactly the
    pavers' own height. `occlusion::opacity` reads that as `CLEAR` and
    `Builder::add` returns without pushing anything, so it is the piece that is
    a point of nothing.
  <br>
  So it is a **cornice, not a roof**, and it is `WALL`-flagged: reading it as a
  wall is right, and the side faces are its own. Asking `is_roof()` in
  `boxes_of` was tried against this frame and moved no pixel — the note stands
  in that function, since the header does claim a roof is a lid and the next
  person will think of it too.
  <br>
  What is left is the **fringe**, and this is the case that decides its open
  item above. The lit pixels are the picture overhanging its own box: they take
  the nearest face — which along a silhouette is whichever, and here is a side
  one with a real cosine — and they are clamped onto the box's *edge*, which is
  exactly where the neighbouring lid's shadow boundary runs, so a few of them
  land clear of it and blaze. Being a point of nothing they cannot be excused by
  identity either.
  <br>
  ✅ **And the person who reported it named the shape of the answer: give a
  floor real bounds** — done 2026-08-10, `docs/render/design_frame_assembly.md`'s **P4.1**, which
  carries what landed and the one thing this paragraph got wrong (the
  thickness: a `z` unit puts the top of every interior wall under a storey into
  shadow, measured, so `occlusion::LID_THICKNESS` is `1/64` and argued from the
  wire's resolution and the screen's instead). A lid was the one primitive in
  this grid that was a
  *plane* — `min.z == max.z` — and every defect on this list that involves a
  floor is a consequence of that degeneracy rather than of any one rule: the
  corner leak (an interval of no length), the strictness `crosses` needs (a
  candle on the floor it lights), the fragment sitting exactly *in* the plane
  and so on neither side of it, and `meets` having to be told that a lid's side
  faces are lines. A floor a `z` unit thick is a body like every other, and each
  of those dissolves rather than being ruled about: a ray from its top going up
  never enters it, a ray from its top going down does, and its faces have area.
  <br>
  What it touches, and what has to be measured before it lands: `Solid::box_of`
  gives a lid `bottom..top` today and would give it `bottom - 1 .. top`, so
  every floor in the world moves; the walk's whole `Edges::NONE` arm — and
  `crosses` with it — becomes a body's `opacity` outright; `occlusion::merge`
  starts folding floors as bodies; and `impostor::meets`'s "a lid has no side
  face" guard becomes dead. The gates that decide it are `tests/lighting.rs`'s
  floor scenes, `scene::storey_over_a_torch`, and the traced suite — a storey's
  floor is the fixture that catches both directions, and the *thickness* is the
  one number to justify rather than pick: a `z` unit is what `Z_STEP` calls one
  step of height, and a floor thinner than the quantum its own height is stated
  in is a floor the wire cannot describe.
  <br>
  Three ways out of the fringe, and the zero vector is not one of them: `blit.wesl` shades a
  fragment with no facing as *lit from every side*, so a fringe given none comes
  out brighter still. What is left is to give a **miss** the face the sprite's
  own volume presents rather than the nearest one (uniform along a silhouette,
  and for a panel it is the panel's own side), or to stop drawing geometry the
  grid holds nothing for — which 6c already refused, and rightly: the fallback
  is what keeps half of Britain's pictures off billboards.
- 🚩 **A wall run built of several graphics shows the same "lid at the seam"
  glow as the cornice above, and it is not that case — these statics *are* in
  the grid.** Reported live in `openshard-playground`, not yet a checked-in
  fixture: Britain, `(1507, 1656)`–`(1507, 1662)`, an upper-storey brick wall
  standing on the tile's `East` edge, three (and more) consecutive tiles each a
  different graphic — `0x0038`, `0x0035`, a window at `0x003C` — all
  `WALL|NO_SHOOT|BLOCK`, so `Builder::add` pushes a real panel for every one of
  them. None of this run is the "point of nothing" the entry above is about.
  `View::Normal` at the seam between two of these tiles reads a `+z` (lid) band,
  roughly 20–40 screen pixels wide at `zoom` 4–6, cut into the *middle* of an
  otherwise uniform `+x` face — not a silhouette edge, since the `+x` colour
  returns on both sides of it. Reproduces in `isolated_scene`
  (`OPENSHARD_SCENE_AT=1507,1660,27`) too, measured pixel by pixel, so it is not
  particular to the live client's own scene assembly.
  <br>
  **Ruled out this session, each checked rather than assumed.** The two panels'
  own end faces (`Solid::box_of`'s outer plane) are bit-for-bit equal — in `f64` and
  after the `f32` wire round-trip, verified numerically for the real tile
  coordinates: no rounding anywhere in the box's own extent, on either axis.
  The defect reproduces with **no active flame** anywhere in the scene
  (`View::Shadow`/`View::Light`/`View::Sun` all flat there), so it is not
  `on_the_lit_surface` or the `lit.solid == Some(id)` exemption — there is no
  shadow ray for either to get wrong. `sample_count` is `1` everywhere in
  `renderer.rs`/`gbuffer.rs`, no MSAA anywhere to blend a normal across an edge.
  `depth::base_for` is `x + y`, symmetric in the two axes. The saved world's
  `decorations`/`items` tables hold nothing near this tile.
  <br>
  **Reported direction-specific, and still unexplained.** Seams along a run's
  `y` — a tile's `East`/`West` panel, thin in `x` — show it; seams along a
  run's `x` — `North`/`South`, thin in `y` — do not, on the same building.
  Nothing read this session in `Solid::box_of`, `lit_plane` or `depth::base_for`
  treats the two axes differently, so the asymmetry itself is still open.
  **Not yet checked:** the corner case's own guarantee does not obviously reach
  this one — `Facing::Corner`'s designed `PANEL_THICKNESS`-square overlap is
  real, but it is for *one* static naming two edges; `boxes_of`'s plain per-tile
  push has no stated equivalent for *two different* statics meeting across a
  tile boundary, and whether that gap is real was not settled either way. Nor
  is the selection `statics.wesl` runs between *different* static instances
  competing for one screen pixel — this session never opened that shader.
  <br>
  **It does not reproduce in `isolated_scene`, and the reason is that the tool
  and the client do not have the same primitives at the same place.** Measured
  rather than argued, at `1507,1660` and again at `1505,1653`: over a bare pair
  of abutting panels, over the four-panel run, and over the whole building with
  everything the tiles really hold, the `+x` face is continuous across every `y`
  seam and every `+y` run is `4` screen pixels — `PANEL_THICKNESS`'s own `0.2`
  of a tile, an honest end face at a free end. The one-pixel runs a census does
  find are all wedge *tips*: a lid emerging from behind a wall, one pixel on the
  first row and three on the next. Nothing a person would call a stripe.
  <br>
  What is not shared with the client is the **partition**. `merge::merged` runs
  in `Builder::finish` over the *frame's rectangle*, so which pieces of a run
  fold into one primitive is a fact about what else got into the picture. Same
  camera, same place, radius `4` against radius `16`: **132 of 83,830 adjacent
  pixel pairs change their answer to "one primitive or two"** — seams appear and
  vanish with the rectangle. And `statics::push_volumes` takes the *grid's*
  merged box wherever the grid names the piece, so this reaches the normal plane
  by construction. A defect that lives on a seam therefore cannot be reproduced
  by pointing the tool at the coordinates: the seam is not there to hit.
  <br>
  **Ruled out this session.** The client's F10 is, as far as `View::Normal` is
  concerned, exactly "meet the sprites against the grid or against nothing" —
  with the lights off `App::draw` never calls `light::collect`, so
  `statics::collect` gets an empty grid and every fragment takes
  `statics.wesl`'s billboard fallback. That switch is now
  `OPENSHARD_SCENE_IMPOSTOR=0` in `isolated_scene`, and at both places above the
  two `View::Normal` dumps are **equal to the pixel** (0 of 691,200) while
  `View::Solid` goes from 87 primitives to none: the merged box and the per-tile
  box agree everywhere a fragment lands *there*. The **bake** is ruled out too —
  the client passes `Some(&mut self.occlusion_bake)` and the tool passes `None`,
  and the only oracles for it held one rectangle still, which is the one state a
  cache is never asked about. `a_baked_grid_is_the_one_the_walk_builds_after_the_camera_moves`
  now walks the rectangle across the town a tile a frame and the baked grid is
  the walked one at every step.
  <br>
  **The rectangle is exhausted, measured.** Growing what the tool is given of
  the map settles the partition: radius `16` against `24` moves 36 of 205,148
  adjacent pairs, and radius `24` against `40` moves **none of 207,415**. The
  stripe census is identical at all three. So handing the tool the whole map
  changes nothing at this place, and "the client has more in frame" is not the
  difference.
  <br>
  **The anchor is a real one-pixel difference, and it is now expressible.**
  `OPENSHARD_SCENE_ANCHOR_REAL=1` builds the scene where the map has it instead
  of translating it next to the synthetic origin. Same data, same camera — the
  anchor delta is a whole number of tiles, so the projection moves an exact whole
  number of screen pixels and the framing is bit-identical — and **760 pixels
  come out different, 746 of them one-pixel runs**: 514 that were a wall's own
  face at `(100,100)` are a lid's `+z` at `(1507,1660)`. That is the arithmetic
  and nothing else, and it is exactly the width the reported defect has. What it
  does *not* do at this place is put a new stripe in the middle of a uniform
  face: the wedged-run census barely moves (25 → 24), so what the anchor buys
  here is edges landing one pixel over, not the reported band.
  <br>
  **And nothing loses its facing.** Four places — `1505,1653`, `1507,1660`,
  `1490,1636`, `1497,1626` — crossed with both the anchor and the grid: **zero**
  pixels of 691,200 come out with no facing in any of the sixteen frames. So the
  "faces disappear when the light goes on" a person sees is not a fragment left
  without an answer, at least nowhere this tool has been pointed. The grid moves
  the count by nothing at three of the four and by thirteen wedged runs at
  `1497,1626`, which is the one place worth going back to with a picture.
  <br>
  **And then the person showed a picture, and the thing they were reporting was
  never a stripe.** At Britain's `(1501, 1659)` — a counter (`0x0B40`,
  `BLOCK|PLATFORM`, height 6) and boards on the tile, shingles overhead — what a
  `View::Normal` crop holds is isolated **specks**: single pixels carrying a side
  face's normal with lid on all four sides of each, spaced **exactly
  `TILE_WIDTH`** apart. It reproduces in `isolated_scene` at once — seven of
  them, two naming no primitive at all and every one naming a primitive its
  surroundings do not — and it reproduces **identically at both anchors**, so it
  was in every frame this session had already drawn.
  <br>
  **The measurement was wrong, not the renders.** Every census run above counted
  *runs*: a foreign face wedged between two of the same, run-length one. A speck
  has no run — its neighbours along the row are lid on both sides only because
  the neighbours in the column are too — so a run-length detector reports zero on
  a frame full of them. `docs/style.md`'s own moral, from the other end: a
  detector must be able to say what it counted, and this one was blind to the
  shape it was hunting.
  <br>
  **Where it is not.** `examples/speck_probe.rs` sweeps a body's whole top face
  through `impostor::meets` at the sub-pixel step the projection actually puts
  fragments on — corners inclusive, since the tie rule is written for them — and
  **0 of 7921** samples come back a side face, at Britain's magnitude and near
  the origin alike. So the face choice for one box is not it, and neither is the
  `hi.z > lo.z` guard, which covers a lid and was never about a body. What is
  left is which *box* the fragment is given: each speck names a different
  primitive from everything around it, which is `impostor::nearest` picking
  another of the static's own volumes — or `push_volumes` handing it a list the
  sprite is not a picture of. That is the next thing to instrument, and it wants
  the real `boxes_of` for `0x0B40`/`0x0B01` rather than a synthetic pair.
  <br>
  **Cut the roof and the count goes from 7 to 66**, which is what makes this a
  reproduction rather than a sighting. `OPENSHARD_SCENE_NO_ROOFS=1` is now the
  tool's own cutaway — the third difference with the client, and the state a
  player standing indoors is actually in. Under the roof the specks stop being
  scattered and line up: **dashes of four, running along the diagonal a tile
  boundary projects to**, and each pixel of a dash sits on one floor slab while
  naming the **neighbouring** one (`here (69,55,46)`, `around (131,88,61)`, and
  the pair steps to the next tile with the next dash). Thirty-two of the
  sixty-six are a point of no primitive at all.
  <br>
  So the surface is a floor of abutting lids, the line is the seam between two of
  them, and it is one pixel wide because the seam is a shared plane and the
  fragment on it is answered by whichever slab wins the tie. That is the whole
  report, and it is the same sentence as the reporter's first picture: a short
  red stroke on the join between a green face and the blue above it.
  <br>
  ✅ **And the cause, measured end to end by `examples/seam_probe.rs`** — which
  prints, for the real graphics on the real tiles, the box each static stands and
  whether it has height. At `(1501, 1659)`:
  <br>
  - `0x04AC` "wooden boards", `FLOOR|NO_SHOOT|PLATFORM`, box `z 27.0..27.0` — a
    **lid**. `meets`'s `hi.z > lo.z` guard means a fragment of it *cannot* come
    back `+x` or `+y`. So the dashed line is not the floor's own pixels, and the
    whole "which slab wins the tie at the shared plane" reading above is wrong.
  - `0x0B01` `BLOCK|PLATFORM` at `z 27.0..30.0`, `0x0AFE` at `z 30.0..35.0`,
    `0x0E29` at `z 30.0..31.0` — the furniture standing *on* that floor. Every
    one of them has height, and every one is **`opacity 0`, `CLEAR`**: the same
    "point of nothing" the cornice entry above is about. `Builder::add` pushes
    nothing, `id_of` has no name — which is exactly the 32 of 66 specks that name
    no primitive at all — and `push_volumes` hands the impostor a `boxes_of` box
    regardless.
  <br>
  So the speck is a pixel of a *piece of furniture's* sprite overhanging its own
  box: a **miss**, clamped to the nearest point on the box, and along a
  silhouette the nearest face is a side one. It lands on the floor because that
  is what the sprite overhangs onto, and it repeats on a lattice of one tile
  because the boxes are per tile and the furniture tiles across the room.
  <br>
  ❌ **And that is wrong for 59 of the 66, measured by doing it.** A throwaway
  `if !hit(best) { discard; }` in `statics.wesl` — the whole of "just do not draw
  the fringe" — takes the count from **66 to 59**. So only seven of them are
  overhang misses. The rest are *hits*: the ray genuinely enters the box (or
  grazes it inside `TANGENT`) and leaves through a side face, which is a correct
  answer about the box and a wrong one about the picture.
  <br>
  What the other planes say about them, at the same pixels: the height plane
  differs from the four neighbours' only in the low bits of one channel — the
  same surface, not a different body — and only **2 of 66** have all four
  neighbours naming *one* primitive, so a speck sits on a boundary between
  primitives rather than marooned inside one. They come in dashes of four along
  the direction a tile's own `y` edge projects to.
  <br>
  The reading that fits all of it, and the part that is still inference: a
  sprite is 44 pixels wide and **overhangs its neighbours' tiles**, so the pixel
  drawn over the boundary belongs to a static whose box is a tile away; its ray
  enters that box near the edge and exits through the side. `nearest` only ever
  sees one static's own volumes, so no choice between neighbours is involved —
  which means the fix is not in the selection but in what a box is. It is the
  accepted cost this document already states — *"statics without a good prism get
  a rougher volume"* — read back as a lattice, and clipping the sprite does not
  touch it.
  <br>
  **Which makes seven of them the fringe, not a new defect** — the same open item the
  cornice case ends on, and **not with the same three ways out**, which this
  paragraph got wrong when it was written: the cornice entry names its own
  candidates "the zero vector is not one of them", ruling out a no-facing miss
  by reasoning alone — a fringe with no facing is lit from every side, which
  makes a *blaze* brighter, not fainter. What is actually open there is two,
  not three: keep the clamp, or give a miss the face the sprite's own volume
  presents. No-facing is a live, unmeasured candidate for a *different*
  backlog item (the serrated-edge entry below), not for this one — and
  `statics.wesl`'s own history ("One silhouette", read at
  `docs/render/design_frame_assembly.md`'s P4 step 2) is a third, prior data point again: giving
  every miss no facing, tried globally rather than scoped to either of these
  cases, measured a worse artefact (a lattice of lit dots across every floor
  and roof seam) and was reverted. Three mentions of the same shape of fix
  reaching three different verdicts is itself worth having written down.
  What this session adds beyond that correction is that the fringe is not a
  rare corner: it is a dashed line across every floor a person stands on
  indoors, and the pieces making it are `CLEAR` ones the grid holds nothing
  for, so no identity can excuse them either.
  <br>
  **What is left of the difference, ranked, and none of it measured yet:** the
  frame's rectangle and the live `Cutaway` (both feed the partition above); the
  **anchor** — the tool translates the place onto `SYN_ANCHOR (100,100)` while
  the client works at `(1507,1660)`, and `Solid::space` is absolute, so an `f32`
  ulp there is some sixteen times the one here and any tie on a shared plane is
  decided at a different precision in the two; the **atlas**, which has grown all
  session in the client and holds a screen's worth here, and which is where
  `boxes_of`'s `Shape` comes from; the **clocks** (`0.0` and
  `StaticAnimations::default()` against advancing ones, so an animated static is
  a different picture with a different box); the server's **ground items**; and
  the camera, which follows a walking player at smoothed sub-tile positions
  rather than sitting on a tile anchor.
  <br>
  🚩 **And the dash has moved off the floor onto the furniture's own top**
  — reported 2026-08-10 by a person looking at a client F12 dump (eye tile
  `(1496, 1659)`, `1919x2077`, night, a torch in hand), read back off the planes
  rather than argued: along an alchemist's counter, one stepped dashed line per
  tile boundary, and the line is on the counter's *lid* rather than on the floor
  beside it. Per pixel of a dash, against its own neighbours two rows away:
  `kind` static in all three, `height` the same `z 33` surface on both sides of
  the line, `shadow` white and `reach` unchanged — and `normal` flips from the
  lid `(0,0,1)` to a **side** `(1,0,0)` while `flames` goes from `(19,9,2)` to
  `(255,167,37)`. The light did not change; the facing did, and a vertical face
  turned at the torch takes a full cosine where the lid took a grazing one.
  Frame-wide, the signature — a static fragment whose normal is a side face with
  a lid two rows above *and* below — is **464 pixels, 442 of them with the
  sub-tile position pegged exactly to a tile edge**, 87 of them with the flame
  term blown out, which is the part a person sees.
  <br>
  **Why the floor's cure does not reach it.** `shows_a_side` refuses a face
  thinner than the grid that reads it, and that ends this for a floor because a
  floor is a lid — `LID_THICKNESS`, a sixteenth of a pixel of side. A counter is
  a *body*: its side face is several `z` tall, passes the same test honestly, and
  is what the graze at the top edge is handed. So the two halves of that repair
  (`FRAGMENT` for the seam, `shows_a_side` for the face) covered the population
  they were measured on and left the abutting-body case standing.
  <br>
  ✅ **Hit, not miss — settled by looking, once F2 could be believed.** The
  switch had to be repaired first (`docs/render/README.md`'s fringe entry: the
  silhouette pass was overwriting it inside the frame), and with all three states
  reaching the screen the reporter's answer was *the picture changes and the
  seams do not*. So the fringe is not this, on a lid, the way it was not this for
  59 of the 66 specks on a floor.
  <br>
  ✅ **And it is the box's own top edge, decided by a rounding the grid cannot
  show. `impostor::RIM`, landed 2026-08-11.** `meets` picks the face whose exit
  comes first; along the line where a body's lid ends and its side begins, the
  side's exit comes first by *less than the distance to the next sample*. Those
  fragments are one row wide, and one row along a projected diagonal is a stepped
  dashed line — which is what the reporter drew a finger along. The rule is the
  [`FRAGMENT`] argument a third time, after the hit tolerance and after
  `shows_a_side`: **a side wins only by more than the picture can show.**
  <br>
  It was nearly refused on a misread of its own probe, and the misread is worth
  keeping because it is a shape of mistake rather than a slip. Over a body of the
  shape `seam_probe` prints for this furniture (one tile, five `z` units), across
  its own sprite's samples: 1,010 fragments answered with a side face, the gap to
  the lid's own exit running 0.000 to 0.827 tiles against a `FRAGMENT` of 0.032,
  and 46 of them under it. Read as a *share* — 4.5%, a fringe, not the subject —
  that is a refusal. **Drawn instead of divided**, the same 46 are a band exactly
  one fragment wide running the whole length of both top edges, and there is
  nothing else on the box that is a line. A ratio and a picture of one population,
  and only the picture answers "is this the thing a person is pointing at".
  <br>
  **Priced on the real frame, `examples/discard_census.rs` over Britain's 121×121,
  the same run with the rule and without it:**
  <br>

  | | without | with |
  |---|---:|---:|
  | fragments given an east face | 57,687 | **55,971** |
  | fragments given a south face | 49,504 | **48,304** |
  | fragments given the lid | 1,514,304 | **1,517,220** |
  | the comb's control — two neighbouring **hits** disagreeing | 313,755 · 1.35% | **311,433 · 1.34%** |
  | comb inside an overhang | 6,393 · 0.22% | 6,343 · 0.22% |
  | comb where the overhang joins the art | 732 · 0.30% | 730 · 0.30% |

  <br>
  **2,916 fragments of 1.6 million move, 0.18%, and 2,322 disagreeing
  neighbouring pairs go with them.** The last row is the one that makes it a
  repair rather than a preference: the population this rule does not touch is
  unchanged to the pixel, and the one it does touch is where two neighbours
  stopped contradicting each other. Nothing anywhere got worse.
  <br>
  **And it is a rule about one box**, which is the property that was asked for
  and the reason a second candidate is not being built. That candidate: these
  pieces are `CLEAR`, `opacity 0`, so `Builder::add` pushes nothing, `id_of` has
  no name, and `push_volumes` keeps the **per-tile** box rather than
  `occlusion.solid(id).space` — `SOLID_NOBODY` in 270 of 270 of the seam's own
  pixels. Since `merge::merged` folds a named run into one `Solid` whose space is
  the union, naming these pieces would dissolve the join outright: a run of
  counters would be one box with no interior face to meet. It would work, and it
  is the wrong lever. A body's top edge is a line on an isolated table too, with
  no neighbour to merge with, and a rule that only comes out right when something
  folded is a rule that owes its correctness to an optimisation. The naming
  question stays open on `docs/render/design_frame_assembly.md`'s P4 step 2 for its own reasons —
  identity, shadows, and a grid 15.1% larger — and no longer has this defect
  riding on it.
- ~~🚩 **A sprite's own top edge is serrated**~~ **Measured 2026-08-10, and the
  clamp keeps the fringe.** The candidate this entry and the cornice entry both
  ended on — *give a miss the face the sprite's own volume presents* — was
  written on both sides, run, and refused on the numbers. It is
  `impostor::presented_face`, kept in the tree because
  `examples/discard_census.rs` calls it to price it, and nothing in the pipeline
  does. The instrument is that census's new **`Comb` pass**: it counts
  *disagreeing neighbouring pixels* rather than shares of a population, which is
  the only shape of number that can tell a serration from a two-toned overhang
  — the same face counts describe both.
  <br>
  Britain's `121×121` around `(1501, 1659)`, per neighbouring pair of drawn
  pixels, the clamp against the candidate:
  <br>

  | population | clamp | candidate |
  |---|---:|---:|
  | comb *inside* an overhang, 2,882,656 pairs | 0.22% | **0.02%** |
  | the join to the art it hangs off, 243,275 pairs | **0.30%** | 32.59% |
  | — panels alone | 0.85% | **97.68%** |
  | the control: two *hits*, 23,156,254 pairs | 1.35% | 1.35% |

  <br>
  **The number the candidate's argument never had: 91.79% of the art bordering
  an overhang is on the box's own lid.** An overhang hangs *above* its box, so
  the pixel beside it is where the view ray grazes over the top face — a `z`
  face even on a wall panel whose every other pixel is a side one. The clamp
  agrees with that neighbour by construction, being the same clamp one fragment
  along; a rule reading the *volume* contradicts it by construction, because a
  panel presents its side. That is a hard line drawn along the top of every wall
  in the world, traded for a comb that the control says was never the larger
  number: **two neighbouring pixels that both hit disagree six times as often as
  two misses do.** The overhang is smoother than the picture it hangs off.
  <br>
  What is left of this entry is a sentence rather than a plan: the clamp lies
  about *position* — up to 133 fragments, four tiles — and that lie is bounded
  by the overhang, which is bounded by how badly a box fits its art. So the
  fringe is downstream of the height nobody measures and of nothing else, and
  the population it is measured over is roofs (`0x05A2` loses 35.2% of its art
  to a box three `z` tall). It closes here and reopens only if that changes.
  The report that opened it, kept because it is what a person saw: seen at
  Britain's `(1459, 1693)`
  in `View::Light` and again in `View::Normal`: along a wall's top boundary the
  normal alternates pixel by pixel between the wall's own camera-facing face and
  the neighbour above it, and the light alternates with it. The rule it comes
  from is this document's own — *"a pixel of the sprite whose ray misses the
  prism takes the nearest point on it — the art overhangs its own volume by a
  pixel or two and that is what it means"* — and the accepted cost beside it
  (*"statics without a good prism get a rougher volume"*). What nobody wrote down
  is that the *nearest face* of a miss flips between two answers along a
  silhouette, so a smooth overhang reads as a comb. Three candidates and none of
  them measured: keep it (it is a fringe of one pixel, and phase 6d moves these
  fragments anyway); give a **missed** ray no facing at all, which is what the
  normal plane's third state means and is honest about a volume that does not
  describe the pixel — but it puts a fringe lit from every side against
  neighbours that are not, so it has to be looked at rather than argued; or take
  the *instance's* own single facing for a miss, which is the pre-6c answer for
  the whole sprite applied to its overhang alone. **Measure the flipping pixels
  first** — how many, how far out (`Meeting::outside`), and whether they are the
  same set as the ones a person can see. That instruction is what was carried
  out above; the third candidate it lists, "the instance's own single facing",
  *is* the one the census priced.
- **A billboard's brightness is a per-row estimate no longer, and what is left is
  ordinary sampling noise.** Phase 7's position half took away the correlation
  that turned eight rays into bands; it did not take away the eight rays. A
  mobile standing next to a flame is now dithered per pixel like everything else,
  which is the *same* grain the entry below names and is what the ray-count knob
  exists to trade against. Worth a look on a real figure before deciding
  anything: at `FLAME_RADIUS` the grain is small, and it was only ever a person's
  complaint at a flame size eight times that.

- ~~🚩 **Two world claims are asked about a fragment that is a point of no solid,
  and that is why `same_run` still reads as load-bearing.**~~ **Done, and it was
  three places rather than two.** `light_runs_along_a_wall_and_stops_across_it`
  and `the_two_faces_of_a_corner_are_lit_from_the_side_each_looks_at` built their
  spots with `Spot::face` and never called `Spot::part_of`, so `spot.solid` was
  `None` and `on_the_lit_surface` — the rule that would excuse a coplanar
  neighbour along a run — was never even consulted. Both now name their own solid
  out of the grid the same frame built: the run's panel by
  `Occlusion::id_of`, the corner's by its **edge mask** rather than by a `Part`
  number, since which panel is pushed first is `boxes_of`'s business and not a
  fixture's.

  **The third was an instrument, and it is the one that mattered.**
  `plan::elevation` — what `pictures.rs`'s two wall tests are pictures *of* —
  wrote `OwnerId::NONE` into every row it built, under a comment saying a
  diagnostic picture is never walked for shadows. `View::Flames` **is** a walk, so
  every pixel of an elevation was a point of nothing: exempt from nothing,
  shadowed by its own panel. A `Wall` now carries `of`, the static the run is made
  of, and `drawn` asks the caller for the owner where it builds the row — the
  same shape `statics::quad_of` has in a real frame. Stated by the caller and not
  searched for: a tile holds several occluders, and picking the wall-shaped one
  would be the instrument deciding what it is a picture of.

  **The measurement S4 was waiting for, on the whole crate, both sides
  neutralised:** with `same_run` returning zero in `light.rs` *and* in
  `blit.wesl`, all 510 tests of `openshard-client-render` pass except
  `same_run`'s own unit test — the brute-force oracles, the GPU parity sweep and
  both wall pictures included. Before the three fixes the same injection turned
  four tests red. The controls both ways: with `on_the_lit_surface` neutralised
  instead and `same_run` live, the crate is **also** green; with both neutralised,
  `a_room_lights_its_own_wall_and_not_the_storey_over_it` and
  `a_wall_lit_from_one_end_has_no_dark_stroke_at_its_seam` go red. So the pair is
  genuinely load-bearing and the two rules are **mutually redundant on every
  fixture in the tree** — which is a licence for S4's deletion and not a proof
  that the tree can choose between them. What chooses is the argument, and it was
  already written: D2 is a theorem about a box and a plane, `same_run` is cell
  arithmetic that excuses more than the theorem allows — a tile's *north* panel on
  the same row is excused by `same_run` and correctly is not by D2.

  **And it is deleted**, `docs/render/design_occluders.md`'s S4, first of that step's four: out
  of both walks in `light.rs`, out of `blit.wesl`, taking `on_surface` — the
  height half of the mask, whose only reader it was — and the two unit tests whose
  whole subject was either. The panel arm of all three walks is `pierced` and
  nothing else now. S4's gate in full: suite green with the rule neutralised
  before the cut, suite green after it, and the identity injection turning
  **exactly the same six tests** red before and after, so the self-shadow rule is
  demonstrably untouched.
- ~~🚩 **S3's surface exemption is now unreachable, and its gate is vacuous rather
  than green.**~~ **Refuted, and the entry above is why.** Phase 5b's numbers — `0`
  of 720 blamed with the rule neutralised, the whole of `tests/lighting.rs` passing
  without it — were taken while `same_run` was standing beside it answering the
  same cases, and while three fixtures named no solid so that the rule could not
  fire at all. With `same_run` deleted and every fixture naming its own solid, the
  same injection is anything but vacuous: `on_the_lit_surface` forced to `false` on
  both sides turns `a_room_lights_its_own_wall_and_not_the_storey_over_it` and
  `a_wall_lit_from_one_end_has_no_dark_stroke_at_its_seam` red. It is the rule the
  crate now stands on, and the general lesson is worth more than the entry: **a
  no-op measured beside a second rule that covers the same ground is a statement
  about the pair, not about the rule.** Neutralise one at a time *and* both.
- 🚩 **A test whose subject moved out from under it goes on passing, and this
  track has now found two of them in one place.** Both vertical-ray tests were
  written when a flame was a point; phase 5 made it a sphere whose samples are
  never its centre, and from that day neither test sent a vertical ray — the
  branch each is named for was entered zero times by the whole crate. Nothing
  said so, because a test that stops reaching its own case *passes*. The repair
  is a **positive control** in each (`flame_points` must return the fragment's own
  `x` and `y`), and the question it raises is general: **which other fixtures name
  a case a later phase took away?** The candidates are every test written against
  a point flame — anything whose scene puts the flame exactly on a plane, exactly
  on an axis or exactly at a corner, since an eighth of a tile of sphere is enough
  to move all three. Worth one sweep of `tests/` with that question rather than a
  guess.
- **`brute_force_blocked`'s step count comes from the *horizontal* run alone, so a
  steep ray gets almost no samples.** `steps = ceil(ground / BRUTE_STEP)`, and
  `ground` is `sqrt(dx² + dy²)` — a ray climbing twenty `z` over a hundredth of a
  tile is sampled once, and a vertical one not at all (`1..1` is an empty range,
  so it returns "open" without looking). Its own-column exemption covers the whole
  segment of a vertical ray besides, so it has *no opinion* about the case rather
  than a wrong one. Nothing has been convicted by it yet — the fuzzers aim
  horizontally — but this is the one non-circular oracle in the tree and its
  resolution should come from the segment it is measuring: `sqrt(dx² + dy² +
  (dz / Z_PER_TILE)²)`, the same isotropic metric everything else uses. Cheap, and
  it wants its own run before it is trusted, because a finer march can only turn
  "open" into "blocked" — which is a finding either way.
- **`walk_sun` answers an overhead sun by hand, and its reason is the one the
  vertical shortcut just lost.** `horizontal < 1e-6` returns `(1.0, None)` under
  the comment *"the only thing that could shadow the spot is on its own tile —
  which is exempt"*. Since phase 4 a fragment is exempt from **its own primitive**
  and not from its tile, so a floor under a roof, both on one tile, is a sun ray
  through a roof. Measure zero in practice — a sun exactly overhead is one instant
  of one day curve — which is why it is a backlog line and not a defect report;
  what it is *not* is a rule that still has an argument behind it. Deleting it is
  the same one-line change the vertical shortcut was, and the same census applies:
  find out whether any fixture reaches it first.
- **A flame of radius zero costs eight identical rays.** `flame_radius` is a knob
  now — `Lighting`'s field, and `examples/boxes.rs`'s `OPENSHARD_FLAME_RADIUS`
  since a person wanted to see how hard a shadow can be — and at zero every one of
  the eight sample points *is* the centre, so both walks and the shader walk one
  segment eight times and average it with itself. Nothing shipped asks for zero, so
  this is a diagnostic paying eight times over rather than a frame doing it; the
  fix is one branch, and the reason to write it down rather than add it is that a
  branch on a radius is a second code path through the hot loop and wants a
  measurement before it earns one. `pathtrace::Body::Point` already makes exactly
  this distinction on the reference's side, and `Image::is_exact` is how it says so.
- **A flame's eight rays are now in the brightness, and near the flame that is
  visible grain.** Phase 5b's accepted cost: where a fragment stands inside the
  sphere, how many of its samples clear its own plane is a coin flip, and 126 of
  256,711 pixels of the wedge scene are more than an eighth of full scale from the
  reference — worst `0.4896`, all within about a tenth of a tile of the flame.
  Nothing in the tree gates it, and the two things that would are the ones phase 5
  parked: more rays, or temporal accumulation. Worth a picture at a real lamp
  before either is built.

- ~~🚩 **The flame has extent for the shadow term and no extent for the cosine,
  and that is the wedge of shadow at every join.**~~ **Done — phase 5b, and its
  own account is up there.** Two of the claims below did not survive the landing:
  the prototype's "20,308 of them darker" is the wrong sign (163,492 pixels move
  on the gate's own fixture and 162,921 of them are *brighter* — an average of
  `max(N·L, 0)` over a body is never dimmer than the centre's cosine), and
  `same_run` was **not** retired by it. What follows is the measurement that
  produced the phase, kept whole, because the reasoning is what made the phase
  right even where its numbers were not.

  `shadow()` averages visibility over `SHADOW_RAYS` stratified points of a sphere
  of `FLAME_RADIUS`; `lit_from()` then multiplies by **one** cosine taken from the
  flame's *centre*. So the light is an area source for occlusion and a point source
  for shading — one state in two shapes, and the difference lands on the screen.

  What it draws: a lamp lower than `FLAME_RADIUS` above a floor puts half its
  sphere **below** that floor's own plane. Rays to that half are traced, and near a
  join they leave the fragment's own primitive and enter the neighbouring one — so
  they come back "blocked" and darken a surface that is flush and continuous.
  Further from the join the same dipping ray stays inside the fragment's own box and
  identity excuses it. Hence a **wedge**, widest at the join and tapering away, on
  three flights of stairs that are geometrically one landing. Same shape for two
  wall panels, two floor planks, any two abutting statics.

  The cure is the physical form, and it is exact rather than a mitigation: a sample
  point's contribution is `V(p) · max(N·L_p, 0)`, so a point below the fragment's
  horizon contributes **zero whatever stands in its way**. The set of rays that a
  join blocks and the set of rays that are below the horizon are *the same set* —
  which is why moving the cosine inside the loop removes the wedge entirely instead
  of dimming it.

  **Prototyped and rendered.** Per-sample cosine, outer multiply removed:
  the wedges vanish, and so does the eight-ray speckle on grazing surfaces — a
  below-horizon sample becomes a deterministic zero instead of a coin flip between
  blocked and open. 21,177 pixels move on the stair fixture, 20,308 of them darker,
  which is the overestimate the centre cosine was paying out.

  **The side-lit case is real and is not to be discarded for want of a picture.**
  It was reported from the client and reproduced in an earlier session; this
  session failed to render it, which is a fact about the fixtures reached for, not
  evidence against it. Treat it as present. It is also the case where a cosine
  cannot hide anything — a lamp beside a wall lights the face it grazes — so it is
  the configuration to check *after* the per-sample cosine lands, and the reporter
  expects it to go the same way.

  Two things it should also settle, and both want measuring rather than assuming:
  `docs/render/design_occluders.md`'s `same_run` is broad precisely because it was papering over
  these below-horizon rays for panels, so this may retire its real reason; and the
  seam that plan hands to its merge (S3b) may be the same defect seen from the
  geometry side. **The reporter's own hypothesis, kept as one:** the artefact first
  reported with a *side* light may go the same way once this lands.

  It is a shading question, not an occluder one, which is why it lives here. The
  gate is the reference path tracer: it samples an area light with 64 paths and a
  real Lambert term, so per-sample cosine should move the frame *towards* it.

- ~~🚩 **A shadowed floor leaks a one-pixel line of full light along every tile
  boundary.**~~ **Fixed, and the fix is `light::starting_cell`.** The cause is
  the last measurement below: the carried tile was allowed to *contradict* the
  position rather than only to break its ties. `starting_cell` keeps the carried
  tile for every point of its own tile's closure — both edges included, which is
  the whole of what it was for — and takes `floor` for a point strictly outside
  it; both walks and `blit.wesl` now seed from it. On the frame that found it,
  the narrow leaks over the building's floors went from **303 to 0** (99 remain
  in the count, all at the wedges' own penumbra edges, where a one-pixel run is
  what a shadow boundary is). Three gates, each fault-injected to red before
  being trusted: `a_walk_starts_in_a_cell_its_own_start_point_is_in` over
  fractions either side of both edges;
  `a_ray_starting_just_past_its_own_tile_is_stopped_by_the_cell_it_is_in` on
  both CPU walks; and — for the shader's own second spelling of the rule —
  `a_fragment_a_hair_inside_a_wall_is_shadowed_by_the_cell_it_drifted_into`,
  which needed `Fixture` to grow a `drift`, since a parity fragment's fraction
  runs to `112/127` and could never reach an edge at all. Neutralised in the
  shader, that pixel reads `241` against its open neighbour's `241`.
  ⚠ **The rule is gone since S4 and the leak stays fixed by construction** — a
  walk seeded from `from.floor()` is always in a cell containing its own start
  point, which is what the leak broke. Of the three gates, the first was
  repointed at `dda_walk`, the second deleted (it had stopped gating anything)
  and the third kept as a fixture; `docs/render/design_occluders.md`'s § *The starting cell*
  has which and why.
  **The direction was half the fixture** and the first version of the CPU test
  got it wrong: a ray heading *away* from the carried tile seeds a negative
  distance, leaves at once and reaches the true cell anyway, so it stayed green
  with the rule removed. The leak is the other sign — a ray heading back over
  the carried tile, seeded a whole tile of slack. What is *not* fixed is the
  geometry underneath: a run of coplanar floors is still N solids on N tiles,
  which is the merge `same_run`'s own backlog entry wants and would have made
  this class of boundary rarer rather than answered it.
- **`starting_cell`'s own proptest was describing a point nobody had built**, and
  a fresh seed found it during phase 5b. It asked the generator for an offset,
  handed `starting_cell` the sum `tile + off`, and then judged the answer against
  the offset it had *asked for*: at `tile_y = -6`, `-6.0 + 1.0000002` is not
  representable and rounds to exactly `-5.0`, whose offset from its own tile is
  exactly `1.0` — on the edge, where the carried tile is the right answer. Fixed
  by reading the offset back off the point. The shape is the one worth keeping: a
  generator's number and the number the function sees are two different values
  wherever the sum between them rounds, and an oracle built on the first is
  testing arithmetic it did not perform.
- ✅ **`starting_cell` is a repair and not a construction — closed 2026-08-09 by
  deleting it, `docs/render/design_occluders.md`'s S4.** The entry read: it carries no constant
  and no tolerance, and three fault injections say it is load-bearing, but what
  it *is* is a rule for what to do when a fragment's two statements of where it
  is disagree — the instance's tile through the id plane, and the position plane.
  A rule that arbitrates between two spellings of one fact is the shape this repo
  has a name for. **The construction it named is the one that landed**, and
  almost verbatim: the set of cells a ray visits is a property of the segment, a
  start point on a boundary is a point of two cells, and `ray_vs_solid`'s
  origin-touch rule already discards a box met only at the ray's own start — so
  there is no tie to break and a cell entered for zero length can produce
  nothing.
  Two corrections the doing supplied. **The walk does not need to test both
  cells**: it starts at `from.floor()` and reaches the other at `t = 0` if the ray
  heads that way, so the predicted cost — up to four cells at a corner on the
  first step — is **zero cells**, and `tests/cost.rs` did not have to be able to
  price it. And **`Spot::tile` did not keep the job this entry left it**:
  `same_run` was deleted before this, so the tile's last reader in the whole
  lighting pass was the arbiter itself. What survives of `Spot::tile` is
  `sky_at`, a question about a column of the map rather than about a ray.
- **The lateral seams were checked, and they are the tile's own plane rather
  than a constant — with one named exception.** Measured on the same real place,
  reading the grid's own boxes: every tread of the stair is `x 100.000..101.000`
  and every storey's floor the same, so a stack's *lateral* end is the `+x` face
  of a whole-tile box, at `tile + 1` exactly. Every panel in that radius names
  `EDGE_SOUTH` or `EDGE_EAST` — `y 100.800..101.000` and `x 100.800..101.000` —
  whose **camera-facing** side is likewise the tile's own boundary, which is the
  plane the art draws the wall on. Nothing on the visible side of any of them is
  an invented number.

  The exception is the one already in this backlog, sharpened by the reading:
  `PANEL_THICKNESS = 0.2` fattens a panel *inward*, so a `NORTH` or `WEST`
  panel's camera-facing side is `tile + 0.2` while a `SOUTH` or `EAST` one's is
  `tile + 1`. Two walls of one run, drawn by the artist on one plane, get
  positions **four fifths of a tile apart** according to which edge the art
  happened to name — and the constant is invented outright, by its own doc: "the
  art still cannot measure a wall's depth, so any number is invented". It did
  not show in this frame because the radius held no north or west panel; it is
  in every frame that holds a building's far wall. The construction that removes
  it is the one that entry already names — **one `0.2` slab straddling the
  shared edge**, so a pair of neighbouring walls is one wall and both faces land
  on the plane the art draws — and it is a seam that stops existing rather than
  one that gets chosen a side.

  What no reading here settles is how far a real static's **art** overhangs its
  own box laterally. That is phase 6's own second number, still untaken, and it
  is the only remaining lateral question that a picture rather than the grid has
  to answer.
- **How that one was found, kept because the method is the finding — and because
  four of its six steps were wrong turns.** The lines look exactly like an
  exemption leaking, and they are not: **four fault injections each left the
  frame unchanged**, counted rather than eyeballed. `same_run` neutralised, 303
  narrow leaks against 303. The identity compare forced `false`, 282. The
  origin-touch rule (`entered == 0 && leaves == 0`) forced `false`, 303. And
  `RAY_TANGENT_TOLERANCE` widened ten thousandfold from `1e-6` to `1e-2`, 295 —
  which is what says the answer was never a razor.

  Then four measurements narrowed it. The runs are **one pixel wide at 1:1, at
  2:1 and at 4:1**, so the thing they draw has measure zero in the world; a
  world-space stripe doubles with each notch. They stand **inside one facing
  with the same facing either side** (365 of some 600 runs are `+z | +z | +z`
  off the normal plane), so they are not a step's own edge, which would butt
  against a change of facing. Against `View::Place`'s checkerboard, which is
  drawn from the tile, **303 of 305 straddle a tile change**. And the last one
  is the one that named it: `View::Place` repainted for one run as "is this
  fragment's position outside the tile its own instance carries" separates
  *exactly on the edge* — 5,759 pixels, the ordinary state of every south and
  east face since 6c — from **strictly outside**, which is 474 pixels of the
  frame, and **324 of those 474 leak.** Two thirds of a set that is a third of
  a percent of the picture. `View::Shadow`'s own neighbours are on a mismatch
  4% of the time, so the enrichment is twentyfold.

  What made the last step available was the CPU twin disagreeing with the
  shader for a reason that is not the walk: `isolated_scene`'s profile mode
  builds its `Spot::tile` with `floor()` **on purpose**, to keep showing what a
  naively-derived tile does — so it never reproduced the leak, and that is what
  said the tile was the variable. `docs/archive/render/lighting_raymarch.md`'s tile-boundary
  hazard is the family; the specific defect is one rule that had drifted from
  its own contract.
- ~~🚩 **An emitter is black in its own light, and every free-standing one taller
  than `FLAME_LIFT` is.**~~ **Fixed 2026-08-11 — a body writes no facing**, which
  is the candidate the *next* entry named rather than any of the three this one
  listed, and it closes both. The record of the defect follows; the answer is at
  the end of it.

  Found by looking at a lit frame after phase 6c — the
  one instrument *How this is judged* names — and reproduced at one item and
  nothing else: `OPENSHARD_SCENE_RADIUS=0`, no ground, no statics, one lamp post
  by hand, `0 standing cells`, and the lamp lit by its own flame is a black
  silhouette with a green wick. The chain is three facts that were each right
  on their own. `light::burns` answers only for statics light gets *through*
  (`opacity == CLEAR`), so **an emitter is by definition not in the occlusion
  grid**. Phase 6c gives a shape to exactly those too — "a pane of glass has a
  shape whether or not it casts a shadow" — so a lamp post now has a volume.
  And `light::place` burns at the tile's own centre a `FLAME_LIFT` up, which is
  **inside** that volume: the impostor answers each of the sprite's own
  fragments with the camera-facing plane of its own box, whose normal points
  away from the flame, so `N · L ≤ 0` on every visible face. `View::Shadow`
  reads those pixels *visible*, which is what says it is the cosine and not the
  walk. `mounted_at` rescues the sconce alone — it moves a flame
  `MOUNTED_CLEARANCE` clear of a *panel's* plane, and a panel is another
  static's edge in the same cell, which a lamp standing in the open has none
  of. A campfire is unhurt because its box stops below half a tile and the
  flame clears its lid. **It arrived with 6c rather than being uncovered by
  it**: before the impostor a lamp post was `Stance::Upright`, whose normal is
  the zero vector, and the zero vector is the one value `blit.wesl` skips the
  cosine for. Three candidate answers, none of them measured yet: place the
  flame where the *art* draws it rather than at the tile's centre (the honest
  one, and the same unmeasured sprite reading `MOUNTED_CLEARANCE` wants);
  give an emitter no volume, which trades this for the billboard 6c retired;
  or say that a surface containing its own light source has no cosine, which is
  an exemption and therefore the shape this document exists to refuse. **Phase
  7's billboard question is this question one object over**, and the two should
  be answered by the same reading of a sprite.

  **And none of the three was the answer, because none of them was about the
  box.** A flame moved to where the art draws it is still inside a whole-tile
  box; an emitter with no volume trades this defect for the billboard 6c
  retired; the exemption is refused by construction. What is actually wrong sits
  one line above all three: the box a lamp post stands as is `Edges::ANY` — the
  tile's own walls, handed to a graphic whose facing the art would not name — so
  the face `impostor::meets` answers with is a plane **nobody drew**, and every
  fragment of a thin pole was being told it looks the way that tile's south or
  east wall does. A body writes no facing now (the entry below), and the emitter
  is lit by its own flame as a *consequence* rather than by a rule about
  emitters.
- **A wall a lamp stands against is barely lit, on a real place now.** Open
  question 1 had phase 3's synthetic frames under it; the lit frame above is the
  same shape at Britain: the plaster wall the lamp post is bolted beside takes
  almost nothing from it while the cobbles under it carry a full pool, because a
  flame half a tile out from a plane grazes it. Nothing here is a defect — it is
  the accepted cost, seen at last on art somebody drew — but it is the picture
  the exposure-and-ambient experiment should be judged against, and it is a
  better scene for that than any fixture in the tree.
- **The CPU's `Surface` is four fixed normals and land now has a fifth kind.**
  `light::sample`'s `Surface::Flat` looks straight up, which is exactly right for
  level land and wrong for a hillside — `ground.wesl` writes the bilinear patch's
  own normal per fragment and the CPU side cannot state one. It is not a
  regression: before phase 3 the two disagreed about *every* ground pixel, because
  the GPU wrote a zero there and the CPU wrote `(0, 0, 1)`. It is a new, smaller
  disagreement with a name, and what closes it is a `Surface` that can carry a
  measured vector rather than choose between four.
- **The reference tracer samples its own disc at random, and could stratify.**
  That single fact is why phase 5's penumbra gate is three aggregate statistics
  rather than a per-pixel one: at sixty-four samples the reference disagrees with
  *itself* by a third of a flame at the middle of a soft edge (measured — worst
  `0.3125`), so a per-pixel comparison there is a gate on the ruler. Stratified
  over its own sample index the error would be `O(1/N)` instead of `O(1/√N)` and
  sixty-four samples would be sharper than the engine's eight rays by an order of
  magnitude, at no extra cost — which would make the per-pixel claim available and
  would sharpen `penumbra`'s `over` count from a diagnostic into a gate. It needs
  `pathtrace::Emitter::sample` to know which of `settings.samples` it is being
  asked for, which is a signature and three of that crate's own tests.
- **Nothing on the GPU side tests the shader's own identity compare.** Forced to
  `false`, `tests/frame.rs` stays green from end to end while three tests in
  `light.rs` and `tests/lighting.rs` go red — so the rule the *shipped* walk uses
  is covered only through its CPU twin, which the phase's own commits also
  rewrote. What the one frame test in that shape reaches instead is `crosses`'s
  strictness: its fragment is flat and its own solid is a lid.
- ~~**`parity_frame`'s `Fixture` names an owner, and the shader compares a
  solid.**~~ **Done, 6f**, and not by the fix this entry proposed: the fixture
  names a `SolidId` now, because `gbuffer::Fragment` has a field for one and the
  plane it writes carries it. Writing a **mesh** row instead — this entry's own
  suggestion — would have been a third row table in that function *and* would
  have moved the fixture off the sprite path the shipped defect lived on.
  `the_shader_does_not_stop_a_vertical_ray_with_a_lid_it_is_not_under` states
  the bottom tread outright now, so the grid's reference order decides nothing.
- ~~**`statics.wesl` still clamps a face fragment to `INSIDE`.**~~ **Done, phase
  6c** — there is no clamp and no inverse projection to clamp: a face fragment is
  the point where its own view ray leaves its own panel's box, which is that
  panel's camera-facing plane exactly. The asymmetry the entry worried about is
  gone in the direction it did not expect — a *south* or *east* face lies on the
  tile boundary and a north or west one a `PANEL_THICKNESS` inside it, because
  that is where the slab's visible side is. See the next entry.
- **A north or west panel's visible face is a fifth of a tile inside its own
  tile, and that is the geometry rather than a bug.** `Solid::box_of` fattens a
  panel *inward* from the plane the art draws it on, so the camera-facing side of
  a north panel is at `y + PANEL_THICKNESS` while a south panel's is at `y + 1`.
  Phase 6c answers the picture with the volume, so a north wall's fragments moved
  a fifth of a tile into the room. Nothing compensates and nothing should — but
  the *geometry* has a better shape available: two neighbouring walls on a shared
  edge are two 0.2 slabs meeting, a doubled wall, where one 0.2 slab straddling
  the boundary would be one wall and would put both faces on the plane the art
  draws. `PANEL_THICKNESS`'s own doc argues the inward choice from the doubling
  it was avoiding, which the straddling form avoids better. Worth measuring
  before it is changed: it moves every panel in the grid.

  **And the merge does not answer it, which `occluders.md` first said it would.**
  A tile's north panel and its northern neighbour's south panel do share a whole
  face, but they are an `EDGE_NORTH` and an `EDGE_SOUTH`, and `Solid::edges` is
  documented as never naming two sides — the walk's panel arm reads it. So S3b
  refuses the pair, exactly, and the fattening stays where it was. What would
  answer it is a decision about what a two-sided panel means to `pierced` and to
  `on_the_lit_surface`: a change to what one primitive *is*, which is the same
  ground as the lateral fit (`facing::Prism` has no cross-axis term at all). The
  constant is still both *how thick a wall is* and *which side of its tile it
  sits on*.
- **Three scans a drawn static now, where there was one.** `statics::collect`
  asks `Occlusion::owner_at` for the quad, `Occlusion::id_of` per mesh face and —
  since phase 6c — `id_of` again per *box*, all linear scans of the cell; see
  `owner_at`'s own note about the join this design pays for. A four-tread flight
  scans its cell thirteen times. Nothing measures it as a cost yet, and
  `tests/cost.rs` cannot: it builds its frame against `Occlusion::EMPTY`, so no
  static there has a box to meet and the statics pass it times is the billboard
  fallback. **Both halves of that are work** — the scans, and a cost harness that
  prices the pass the client actually runs.
- **A corner is told apart by the screen half, and the boxes could say it.**
  `statics.wesl` still resolves a corner's two panels by `across > 0.0` — the
  half of the picture a pixel was drawn on — where the impostor already meets
  both boxes and picks between them for the *normal*. What stops the box from
  deciding is the **id**: the left half takes `in.twin`, the row
  `split_corners` appended, and a `Volume` carries a `SolidId` and no row
  number. Two answers to one question, and today they can disagree — the box in
  front and the screen half are not the same test near the tile's own corner,
  where the two slabs overlap. What would close it is the volume carrying which
  *instance row* it belongs to, which is one word in a struct that has a spare
  one.
- ~~**A sprite fragment's `stance` is still the art's reading, and `lit_plane`
  believes it.**~~ **Done, 6g** — see that phase's account. The objection this
  entry raised against the swap (for a wall, `lit_plane(FaceNorth)` is the panel
  box's `lo.y` and the normal names its `hi.y`) turned out to be the argument
  *for* it: the fragment is drawn on the box's camera-facing side, so `hi.y` is
  the plane it is actually in and `lo.y` was the far one. Every gate stayed
  green, which is what says the wall case moved by `PANEL_THICKNESS` and moved
  the right way.
- ~~**`own_solid` scans a cell to name a solid the fragment already met.**~~
  **Done, 6f** — see that phase's account. The missing piece this entry named
  ("a way to get it from the pass that knows it to the pass that asks") was the
  position plane's **fourth channel**, which every producer had been writing a
  constant `1.0` into: an id is three bytes and an `f32` carries every integer
  to twenty-four exactly. What this entry got wrong is the priority — it is
  filed here as a cost, and it was a live, visible defect from the hour 6d
  landed.
- ~~**A run of wall wants to be one solid, and until it is, `same_run` stands
  in.**~~ **Done, phase 6e** — `occluders.md`'s S3b merges a run of coplanar
  panels into one primitive (73 pieces to 9 on the crate's own two-storey house,
  and not one pixel moved) and S4 deleted `same_run`. What is worth keeping out
  of it is that the merge is *not* what retired the rule, which is what this
  entry predicted: `same_run` excused a neighbouring panel for rays dipping
  **behind** the surface's plane too, and those stopped being traced at phase 5b.
  The merge landed anyway, and the run being one primitive is what makes S3's
  half-space exemption enough on its own.

**Inherited from `occluders.md`, which is a record now.** Three of its findings
outlive the track and belong in this list, since this is the live one:

- ✅ ~~**An aperture is the last rule in the pass still stated in a tile, and it
  now costs a merge.**~~ **Closed 2026-08-09 — `occluders.md`'s S6**, and both
  halves went together as the entry said they would: `Aperture`'s `near`/`far`
  are world coordinates on the panel's own run axis, `light::run_v` is
  `along_the_run` with no `floor` in it, and the holes are a storage buffer of
  four `f32` indexed by `SolidId` rather than an `Rgba8Uint` texel folded into
  `LIST_ROW` rows. `Occlusion::list_rows`, `z_byte`, `Z_FLOOR`, `Z_CEILING` and
  the shader's `RUN_STEPS` and `aperture_at` are all deleted. **It fixed two
  live defects and refused the payoff this entry expected** — see
  `occluders.md` § *The aperture* for the readings:
  - a crossing exactly on a tile boundary floored into the *next* tile, so a
    window running to the far end of its own tile read as one at the near end of
    the tile beyond it — § *The oracle*'s own defect, one level up;
  - `z_byte` clamped a hole's two ends into the map's `i8`, and a hole's ends
    are not an `i8`: `Aperture::placed` adds the art's whole units to the
    static's base, so a window on a wall standing at 120 reaches 140 and the
    wire shut it at 127. The record and both CPU walks read it open, so this
    one showed on the shader alone. **The claim that a hole's `z` is quantised
    "and that is no defect" was wrong about the top end**, which is why it is
    written out here rather than merely ticked.
  - and **the merge gains nothing**, which the entry above assumed it would. Two
    pieces may only merge with an equal `Owner`, an `Owner` is a `(z, graphic)`
    and a hole is read off the *graphic* — so two mergeable pieces are windowed
    together or plain together, never one of each, and a wall with one window in
    it is a wall of two graphics that the `Owner` refuses whatever the aperture
    says. The refusal in `occlusion::merge` stays and its reason is now the true
    one: a primitive carries one hole and a run of windows is one per tile.
    That is the fifth time on this track that a step's decision held while its
    stated reason did not.
- ✅ ~~**Two instruments still cannot see a merge.**~~ **Closed, and "cannot see"
  was the wrong diagnosis for both** — measured 2026-08-09 under
  `occlusion::merge`'s own "the union does not grow" injection, live in a build
  where `tests/lighting.rs` goes 12 red. Neither instrument is unreached: the
  sweep carries five scenes that fold (a room 24 → 4 pieces, a carried beam
  24 → 4, a hole in a wall 9 → 3, a house corner 7 → 3) and `pictures.rs` draws
  six, so both walk the broken geometry.
  - `frame.rs`'s shader sweep stays green while its **own census** moves by up to
    934 pixels of 4,096 — a room 2,400 → 1,466 in shadow, a hole in a wall
    1,308 → 744, the room's penumbra 0 → 75. That is circularity with a number
    on it: it counts the wreck and cannot report it, because both sides read the
    same primitives. **Settled: a merged scene buys it nothing and none is
    added.** What gates a merged frame on the GPU is `traced.rs`'s twin.
  - `pictures.rs` was *drawing* the defect: the row behind the wall reads
    `0.094`/`0.111` against a flat ambient `0.063`, at the four columns either
    side of the one tile its assertion read. Closed by reading the band across
    the run — the shadow behind a wall is as long as the wall, nine columns each
    with its own lit-in-front control — which the injection now turns red at
    column 98. `docs/render/design_occluders.md` § *Neither instrument is unreached* has both
    readings.
- **`Solid::footprint`'s `i32` ranges are the one newtype the occluder sweep set
  aside on purpose.** Closing it means a real tile-coordinate type, whose call
  sites reach into `bake.rs`'s whole coordinate system (`origin`, `tile_of`,
  `spill_of`, block and cell indices) — D7's ground, and its own pass.
- **The hierarchy's cost is unmeasured on a real frame.** ~~S5 left `tests/cost.rs`
  reporting the tree, and the run itself is the user's — a heavy live run, not a
  suite gate. Until it is taken, "a BVH is cheaper than the grid" is an argument
  rather than a number.~~ **Taken 2026-08-11**, `tests/cost.rs`, Britain at the
  widest zoom (1/2×, world image 3840×2160), seven flames:

  | case | ms/frame | ns/pixel | over `dark` |
  |---|---|---|---|
  | copy | 0.482 | 0.232 | −29.1% |
  | dark | 0.679 | 0.328 | +0.0% |
  | far | 0.723 | 0.349 | +6.5% |
  | night | 1.865 | 0.900 | +174.5% |
  | sun | 1.165 | 0.562 | +71.4% |

  The tree itself is cheap: `far` (7 flames moved 1000 tiles off, so every
  fragment's broad-phase misses and no node is ever tested) sits 6.5% over the
  `dark` floor. The weight is the `night` row's own 174.5%, and that is the
  ray count and not the traversal — `arrival`'s eight rays per flame in reach,
  each its own `walk` of the tree — matching the same fixture's 4:1-zoom
  reading above (§ phase 5b, one ray vs eight). So "a BVH is cheaper than the
  grid" is now a number for the tree walk specifically, and the number the
  blit pass pays a real frame for is soft shadows' ray count, not the
  hierarchy under them.

  *Two levers follow from that, neither taken yet.* **`shadow_rays` itself** —
  already a runtime knob (`Tuning::shadow_rays`, default `SHADOW_RAYS = 8`) —
  is the cheap one: the cost above scales close to linearly with the count
  (phase 5b's table, one ray vs eight), and turning it down is a quality trade
  a person can look at, not a code change. **Packet traversal** is the one that
  is not a trade: `arrival`'s eight rays share an origin and nearly share a
  direction (a small disc on a distant flame), so `walk` pays for the same
  upper tree nodes eight separate times. Testing a node once against the
  bundle's own bound and only descending per-ray at the leaves would cut node
  visits without moving a single answer — packet/beam traversal, not a
  tolerance — but it touches `walk`/`arrival` in `blit.wesl` and their CPU
  mirror in `light.rs`'s `walk_primitives`/`arrival`, and every oracle both
  already answer to (`tests/lighting.rs`'s fuzz, `boxes.rs`, `synthetic_stair.rs`)
  would have to agree with it before it lands.
- **A sconce's own art says how far it stands out from its wall, and nothing reads
  it.** `MOUNTED_CLEARANCE` is `0.7` of a tile because half a tile reaches the
  plane and a fifth clears it; the sprite shows the real overhang and
  `crate::facing` already measures silhouettes for a living. That is what retires
  the constant honestly, and phase 4 found that deleting it without a replacement
  blacks out every wall carrying one.
- **A slope's normal now nudges its own shadow ray sideways.** `walk`'s `ahead`
  spends the normal's `x` and `y` on `STAND_OFF`, and until phase 3 a ground
  fragment's was zero on both. A hillside's is not, so a slope's ray starts a
  fiftieth of a tile out along the hill. That is more nearly right than not
  nudging at all — it is the direction out of the surface — but it is a behaviour
  nobody asked for arriving through a constant phase 4 deletes. **Closed at
  phase 4**: there is no `STAND_OFF` and no nudge of any kind, so a slope's ray
  starts where the slope is.
- **Two scenes moved because a flame stood in a surface's own plane, and the
  shape of that is worth keeping.** `z: 0.0` in a hand-built `Light` read as "a
  fire on the ground" for as long as the shading term was a half-space, which
  gave such a flame the band's own half. Under a cosine it gives nothing, and the
  tests said so at once. **Every hand-built `Light` in the tree should be asked
  whether it means a tile's `z` or `FLAME_LIFT` above it**; two were found by
  failing, and a scene that merely goes dim would not have said anything.
- **The origin-touch rule is stated three times and tested through none of them
  directly.** `if entered == 0.0 && leaves == 0.0 { continue; }` lives in both
  walks and in `blit.wesl`, and what says it is right is a *tool's* count going
  from 88 to 0 — `synthetic_stair`'s face oracle, which nothing runs under `cargo
  test`. The claim it makes is small enough to state as a unit test of
  `walk_cells_*` on a two-solid fixture (a lid whose edge is another solid's own
  plane, a ray leaving that edge), and that is what would catch it being deleted.
- ~~**A north or west face's normal contradicts the argument `outward` itself
  makes for it.**~~ **Done, phase 6c.** The impostor names the face the ray met
  and a ray from the camera can only meet `+x`, `+y` or `+z`, so there is no row
  left to be wrong; `place_format.wesl`'s `outward` is **deleted**, nothing else
  read it. `crate::place::Stance::normal` keeps the same table on the Rust side
  with the defect written down at its definition — its readers are hand-built
  G-buffers stating a scene by naming a stance, which is a question about the
  edge rather than about the picture.
- **Two pixels at flame height `1` survive the light oracle, and both sit where
  the reshaped tread put them.** `[tread 2's riser] at (100.80, 100.33, z 3.10)`
  — the engine reads it fully blocked and the geometry gives it `0.022`, a tenth
  of a tile above the top of the body that blocks it; and `[tread 1's riser] at
  (100.97, 100.67, z 1.02)` — both sides agree the flame is fully visible
  (`through 255/255`) and they differ by `0.017` in what it is lit *to*, four
  parts in 255. Neither is a visibility disagreement, so the face oracle is
  silent on both. What they share is a flame level with a tread's own top
  (`z 1` is tread 0's height), which is exactly the case
  `segment_clear_of_box`'s own doc calls out: every ray from that height runs
  *along* the plane of every surface at it. Worth one measurement each before
  phase 6d moves these fragments anyway — the mesh pass comes off real statics
  there, and their positions change with it.
- **`synthetic_stair.rs` rebuilds `statics::push_mesh`'s loop by hand**, and that
  is why it still asked the grid for `Part::nth(part)` a commit after the real
  pipeline started asking for `Part::nth(part / 2)`. It cannot call the real one
  — `push_mesh` is `pub(crate)` and an example is an external consumer — so the
  duplication is structural rather than lazy, and the join between a drawn face
  and the solid it names is now written in two places that have already
  disagreed once. The same shape as the seventh hand-built flight below.
- **The three-tread flight is now rebuilt by hand in a seventh place.**
  `statics::tests::flight` joined the five in `light.rs` and the one in
  `frame.rs`, and it is the same `Prism::new(Face::North, &[1, 3, 5])` again. The
  backlog entry below asking for one constructor is a line longer every time the
  scene is used, which is the argument for it.
- **A flame's size is a constant and belongs on the `Light`.** `FLAME_RADIUS` is
  one number for a candle, a torch and a campfire, and `Flame` already carries the
  reach, the colour, the intensity and the flicker — a size is the field that is
  missing, and a campfire is visibly wider than a candle. What stops it today is
  the uniform: `Light` on the GPU is three `vec4`s with no spare lane, so a fourth
  is 1 KB more at 64 lights. Worth doing when something else needs that lane.
- **`boxes.rs` now builds two mirrors of one scene** — the same `Mirrored` twice,
  differing only in the `LAMBERT_PI` on the flame's intensity — because the
  visibility comparison is in `Brdf::Flat` and the shaded strip in
  `Brdf::Lambert`. Phase 4 retires the first, and the second mirror should go with
  it rather than become a habit.
- ~~**The normal plane is sixteen bytes a fragment and needs four.**~~ **Done —
  see phase 2's own account.** An octahedral pair in an `R32Uint`, integers on
  both sides, and the two spare bits carry "nothing drew this" and "no facing"
  rather than the id word doing it. `ATTACHMENT_BYTES_PER_SAMPLE` is 32.
- `docs/archive/render/lighting_height.md`'s backlog does not disappear — most of its entries
  are *deleted* by a phase here rather than fixed, and each should be marked with
  which phase kills it rather than left reading as work.
- ~~The `ground < 1e-6` shortcut (both walks and the shader) is a real defect
  today and becomes moot at phase 4; if phase 4 slips, it is worth fixing
  alone.~~ **Fixed.** All three copies gate on the lid's own footprint now, by
  the horizontal half of `ray_vs_solid`'s parallel-axis rule — `light::
  over_footprint` and `blit.wesl`'s twin. Only the horizontal half, because a
  vertical ray's height answer is `crosses`'s soft one and `ray_vs_solid` would
  answer it hard, erasing the penumbra.
- ~~**There is no lit-against-lit picture, and three separate things stop one
  being drawn.**~~ **Done — this is phase 0, and its account is up there.**
  `<base>_lit_vs_traced.png` is the engine's shaded frame, the tracer's, and the
  difference amplified `8×`; `boxes.rs`'s `flat` scene is where it means
  something. All four blockers went: the albedos come from the frame, the flame
  is the engine's own, the encodes share `tonemap::encode`, and the ambient is
  nothing on both sides. The fourth — a mesh face has no albedo — is not fixed
  but *avoided*, by a scene with no boxes in it, and it is still phase 6's.
- **A body's albedo is still invented, and one scene is not a calibration.**
  What phase 0 now proves is that the engine and the reference agree about *one
  surface, flat, unoccluded, unhued*. Three things it says nothing about: a
  vertical face (no albedo on the engine's side until phase 6), a hued sprite
  (the ramp is decoded to linear before the light multiplies it, and nothing
  compares that against anything), and land that is not flat (`ground_albedo`
  panics on a textured floor rather than handling one — deliberately, because a
  single-albedo reference cannot judge one). Each is a scene the tracer could
  hold once the engine's side has a colour to compare.
- **A scene's flame reaching the whole canvas hid a conflation in two oracles for
  as long as every scene had one.** Fixed — see phase 0's account — but the shape
  of it is worth keeping: the oracles were right about every pixel they compared
  and wrong about *which pixels they had an opinion on*, and no amount of looking
  at their disagreement counts would have shown it, because the count was the
  thing that was wrong. What found it was a scene whose flame does not cover the
  frame. **Every detector in this crate that reads a `View::Shadow` pixel should
  be asked the same question**, and the two here are unlikely to be all of them.
- `examples/two_cubes.rs` still projects world points without asking whose pixel
  it got. Phase 2 moves every other reader to `ids`; this one should go with them.
- **`tests/traced.rs` and `examples/boxes.rs` still build the same scene twice.**
  The two gates inside `traced.rs` now share one `render(Shot)` fixture — which
  is what made the brightness gate cheap to add — but the tool has its own copy
  of the whole pipeline (floor art, synthetic map, atlases, mesh rows, blit), and
  a scene is authored in one and restated in the other. `line_scene` and
  `flat`'s flame are already two spellings of numbers that have to agree for a
  failure in the gate to be reproducible by the tool. The same argument as the
  three-tread flight below, one layer up.
- **The parity harness could not see a sub-tile lid, and still barely can.** The
  shader's copy of the shortcut above was fixed and forty-seven frame tests
  stayed green with the fix deleted again: no parity scene had a solid narrower
  than its tile, so the branch was never run. It has one now, and `Fixture` can
  state an *owner* — without which a fragment on a tread is shadowed by the step
  it stands on and every finer question about a flight is unreachable. What is
  still true is that this is one scene and one pixel of it: the vertical case
  needs the flame exactly over a swept fragment, so one flame buys one comparison.
  A sweep that varied the flame across the tile would buy the whole strip.
- ~~**Parity is circular for any defect both walks share.**~~ **Acted on.** It
  compared the shader against `light::sample`, so a rule wrong in the same way on
  both sides reported agreement — and the whole family is now deleted, see *How
  this is judged*. What is left of that test is its *direct* half:
  `the_shader_does_not_stop_a_vertical_ray_with_a_lid_it_is_not_under` reads two
  of the frame's own pixels and no longer calls a sweep at all. It is the direct
  claim that fires when the shader's gate is removed, and it was always the only
  half that could.
- ~~**The parity apparatus was built on `place`'s packing, which is why it could
  not have survived phase 2 anyway.**~~ **Done.** `parity_frame` and `plan.rs`'s
  `drawn` both go through `gbuffer::Fragment` for all three planes now, and
  neither spells a layout. The one thing that changed shape rather than moving:
  an **id is not a fact a fragment knows** — a world pass has one per instance
  from the rasteriser, and a fixture's is a row number it can only hand out once
  it has seen every fragment it means to draw. So both harnesses gather their
  fragments whole, key a row per distinct tile, and only then pack. `Fragment`
  carries the tile and `Fragment::ids` takes the id, which is that asymmetry
  stated in the type.
- The three-tread flight is rebuilt by hand in five tests in `light.rs` and now a
  sixth in `frame.rs`, each restating the same `Prism::new(Face::North, &[1, 3,
  5])` and the same tile bounds. It is the scene every stair defect is found on
  and it should be one constructor.
- ~~`renderer.rs`'s `depth_state()` has lost its doc comment: `PLACE_TARGET` was
  inserted between the comment and the function.~~ **Fixed.** The constant moved
  below the function it had been spliced into, and both have their own doc again.
- ~~Hand-copies of the third channel.~~ **Fixed, and then the channel went.**
  `gbuffer::Fragment` is what a G-buffer texel *is* — tile, sub, `z`, kind,
  stance — and `ids()`/`position()`/`normal()` are the only three spellings of
  the layout outside the shaders. `plan.rs`'s two closures and `frame.rs`'s
  `parity_frame` went through it; they had three copies of the fraction's
  `<< 2`/`<< 9` between them, and the id plane deleted the fraction outright.
- **A met box is not the same claim as a named solid, and one view was written on
  the assumption that it is.** `View::NormalGeometry`/`NormalSprites` (the normal
  plane split by where the vector came from — measured shape versus the picture)
  first tested `mine != SOLID_NOBODY` for "this fragment is a point of geometry".
  A picture of it is what refuted it: a static may meet a real volume the grid
  holds no *solid* for — a floor's own box stands with `edges 0b0000` and stops
  no light — so every measured face of one read as an unmeasured sprite. The rule
  landed as `statics.wesl`'s own invariant instead (a static's normal is zero
  only where the grid holds it no volume at all), and the general point is worth
  keeping: the position plane's fourth channel answers *which occluder*, not
  *whether anything was measured*, and nothing in the G-buffer answers the second
  question outright. A producer that ever gives an unmeasured static a facing
  breaks the derived test, and there is no assertion anywhere that would notice.
- **A diagnostic layer that keeps a landmark is not a layer.** The two normal
  layers claim to hold one category each, and `KIND_NOTHING` broke the claim:
  every diagnostic passes a pixel nothing drew straight through, on purpose, so
  the world's silhouette stays findable — which put a speaker's letters in the
  *geometry* picture, reported from a client dump. The three views whose subject
  is which pixels are which now paint it black instead. Worth keeping as a shape:
  a view that answers "what is here" can afford the passthrough, a view that
  answers "is anything here that should not be" cannot.
- **A gate over a real frame can be vacuous in the direction that matters.** The
  test built for the above was green with the rule deleted from the shader: this
  fixture draws no text, so its only `KIND_NOTHING` pixels are the background,
  and the background is black either way. The fix was a positive control — one
  background pixel painted white in a *copy* of the world image, which is the text
  pass's own shape (write the image, leave the id plane alone) with none of its
  machinery. Measured both ways: red with the control and the rule removed, green
  with the rule. Any future test about "a pixel nothing drew" needs the same
  planted pixel, because no scene in this repository's fixtures has speech in it.
- **`frame::Draw` filters the drawing and never the lighting**, and the two are
  one line apart in `assemble` with nothing but a comment keeping them apart. The
  cheaper implementation — hand the function fewer statics — reaches the same
  picture and silently empties the occlusion grid with it, so a room whose walls
  are "not drawn" would light up. `ticking_a_producer_off_narrows_the_drawing_and_not_the_light`
  asserts both halves because the picture cannot tell them apart. What is still
  unmodelled: `Draw::mobiles` is honoured by the *caller* (the client's own mobile
  pass), since `assemble` does not collect mobiles at all — a second caller that
  collects them and ignores the field would differ from the client with nothing
  to notice, and `Inputs::summary` printing the field is the only thing standing
  in that gap.
- 🚩 **A climbable's tread is marked a body, and a staircase is the one shape
  whose art does say which way a surface looks.** Reported by a person looking at
  a lit frame at Britain's `(1454, 1728)` — stone stairs, `0x0752`, which the art
  table reads as `corner E S prism W 1 3 5`: the flight comes out with no shading
  of its own, and what is left on it is a hairline along each step.
  <br>
  `occlusion::boxes_of` hands every tread `Edges::ANY`, and its own comment says
  why: *"a stair is solid: a body, whose occlusion is `ray_vs_solid`'s exact slab
  test rather than a lid's crossing test and a panel's run masking."* That is a
  statement about **which occlusion test this box takes**. The same mask is also
  the only thing that says **whether the art named a face at all** — the question
  the three open defects above are one question about — and for a tread the two
  have different answers. A tread's lid is a plane somebody drew and so is its
  riser on the climb axis; only the two faces *across* the climb are the tile's
  own walls. One field, two domains.
  <br>
  **Measured**, `examples/isolated_scene.rs` at that place, radius 6, `800×600`
  at `2:1`: 20,321 fragments in the flight's own window stand on a stair box. The
  shadow term on those same fragments averages **248.8 of 255** — the flame
  reaches them almost unobstructed, so what darkens the staircase is the cosine
  and not the walk. With the met box face as the normal they mean **111.8** of
  765 and 23.2% of them fall below 60; with the zero vector, lit from every side,
  they mean **260.7** and are *flat* — a tread's top and its riser come out the
  same colour and the flight loses every step it has. Neither answer is the
  staircase's.
  <br>
  **And the answer already existed once.** Phase 2's own *done when* was
  `two_mesh_faces_carry_their_own_two_normals` — "a tread's top and its riser,
  one draw, two normals" — off `facing::Prism::mesh`, whose five normals
  `place.rs` still round-trips in a test. Phase 6 replaced that pass with one
  body box a tread and dropped both vectors; the measurement is still in the
  `Prism` and nothing downstream asks it for a facing. So whatever the body
  question is answered with, a tread wants to be outside its scope rather than
  inside it, and the split wants to be two fields rather than one mask read twice.
  <br>
  Unmeasured, and the number that would size it: how many placements over
  Britain's `121×121` stand as a fitted prism under `CLIMBABLE` or
  `PLATFORM` — `examples/geometry_census.rs` already walks that window and counts
  the fitted-prism class as one line (3.2%), without separating the climbable
  from the tables and counters that reach the same branch.
  <br>
  ✅ **Fixed 2026-08-11 — `occlusion::named_edges`.** One expression with two
  readers now: `boxes_of` starts from it and keeps its own override, and
  `statics::push_volumes` asks the *graphic* rather than the box. The gate is
  `a_flights_volumes_name_the_faces_its_art_named_and_a_bodys_name_none`, and it
  is a **pair on purpose** — the same tile, the same flags, the same prism, only
  the measured `facing` differing — so the rule it holds is "the art's answer is
  what this field carries" and not "a climbable is special". Fault-injected: put
  `boxes_of`'s mask back and the first half goes red at `Edges(15)` against
  `Edges(6)` while the second stays green.
  <br>
  ⚠ **And it does not change the frame the defect was reported on.** Measured on
  the flight's own 20,321 fragments: with the art's mask they come out at mean
  **111.8** of 765 and standard deviation **59.0** — *identical to the frame
  before the body rule landed*, which is the frame in the report. What the fix
  actually bought is that the staircase did not become the flat, formless 260.7
  the body rule was about to make it: the zero-normal share on the flight is
  **0.0%**, where it had been **100%**. The thing a person is looking at is the
  entry below, and this one had to land first for it to be visible at all.
- 🚩 **A silhouette score cannot see inside its own outline, and that is where
  the surfaces are.** The finding, and it is a general one: `silhouettes_agree`
  is the only measure any fitted shape in this renderer is scored by, and it
  compares two **filled outlines**. Everything interior to the outline — where a
  step's riser stands, how deep its tread is, a moulding, a recess — contributes
  nothing to the score. Two prisms with the same silhouette and different insides
  are the same number, so a fit can be *confident and still wrong about where the
  surfaces are*, and the lighting is the pass that finds out: a facing is exactly
  what a cosine is computed from.
  <br>
  **Measured, on the reported flight at Britain's `(1454, 1728)`.** The fit is not
  ambiguous — `examples/prism_axis.rs` (new, `artscan`) ranks the whole 261-candidate
  sweep per graphic, and `0x0751` takes `North [1,3,5]` at **0.9752** with its
  entire top six climbing north and a margin of **+0.0775** over the best
  candidate on any other axis. `0x0752` the same, `West`, +0.0775. `0x0754` and
  `0x0758` `East`, +0.0945. `0x0750` is a plain `box [5]`, +0.0520. And `0x0756`,
  which the table holds no prism for, is refused with a margin of **+0.0024** —
  a coin flip between axes, which is the search saying so. Six pictures, six
  confident answers.
  <br>
  **And the insides are wrong anyway.** Over 37 east-face bands sampled across
  the flight, the model's riser and the artist's own step joint are parallel and
  roughly equal in number — median **2** model bands per screen column against
  **3** drawn joints — but the model's riser stands **10.5 view px** where the
  art's joint is **2.5**: four times too tall. So each riser band covers the upper
  half of what the picture draws as the step's *tread*. `blit.wesl` gives a
  vertical face a full cosine where a lid takes a grazing one, measured on this
  very flight at **165.4** against **11.6** of 765, so the model's misplaced
  riser draws as a bright stripe up the middle of every stone slab — which is
  what a person reported as *something extra being drawn there*.
  <br>
  **Where to take it.** The measure, not the fit. A score over filled outlines
  cannot be repaired by more candidates or a higher `PRISM_FITS`; it wants a
  second term that sees inside — the art's own interior edges against the model's,
  which is the same alpha the silhouette detector already walks. `MAX_TREADS` is 4
  and is a cap on the *measurement*, so it belongs in the same question rather
  than beside it. And the reach is not staircases: **every** fitted prism is
  scored this way, which is `geometry_census`'s 3.2% fitted-prism class — the
  tables, counters and display cases `boxes_of`'s `PLATFORM` branch admits on
  exactly the same terms.
  <br>
  **Step 1 (measure) and step 2 (`interiors_agree`) done, 2026-08-11 — steps 3
  and 4 still open.** Step 1: over the whole install, 373 multi-tread fits, 0
  with no confident interior edge at all, mean residual 8.35 view px (median
  8.45, p90 10.28, max 12.02) — `docs/render/README.md`'s 🚩 entry has the full
  breakdown. Step 2: `interiors_agree` (`facing.rs`) is a coverage fraction —
  of a candidate's sampled interior-boundary columns, how many find a confident
  brightness edge nearby — used by `best_prism` **only as a tie-break between
  rival climb axes within `TIE_MARGIN = 0.01` of each other on outline alone**,
  never summed into `silhouettes_agree` and never moving a fit-or-refuse
  verdict. `prism_axis`'s own duplicated projection math (`project`,
  `boundary_columns`, `luma`, `strongest_edge`) moved into `facing.rs` with it,
  so the tool and the production scorer share one copy. Measured effect: **27
  of the 309 accepted near-ties (8.7%) flip axis** under the tie-break.
  `DETECTOR` is 5.
  <br>
  **Steps 3 and 4 done the same day, and step 3 rewrote step 2's own
  measurement.** The gate is a pair in `tests/prism.rs`: a **hermetic** fixture
  (a shaded drawing of a known prism this test makes itself, so plain
  `cargo test` runs it with no install) and the six graphics of `(1454, 1728)`
  held to four decimals, both fault-injected on the art side (flattened to no
  brightness step) and the model side (rotated to every rival axis). It failed on
  its first run for a real reason: a west-climbing stair and *the same stair
  mirrored* both scored `1.0`. Two causes — `luma` counted material-over-nothing
  as a step, so the interior term was re-scoring the **silhouette**
  `silhouettes_agree` already covers; and it counted an edge's *presence* inside
  a ±16-row window while one tread rises 8 px, which answers yes to every rival.
  Both fixed: a transparent pixel is an absence, and the term measures
  **closeness** to either end of the riser (a joint is a face with two drawn
  edges, not one of them by convention). `DETECTOR` is **6**. What moved: the
  tie-break now flips **16** of 309 near-ties rather than 27, and the residual
  this whole track is about reads **4.97 px** to the nearer riser end (7.07 to
  the crest) against the 8.35 first reported — the defect is real and about half
  its reported size.
  <br>
  **Step 4 is measured and answered no.** `MAX_TREADS` at 6 and at 8 buys 15
  more fitted pictures of 2,985 (0.5%) and no accuracy at all — residual 4.97 /
  5.00 / 5.13 px at cap 4 / 6 / 8, and by profile size the three-tread fits (the
  real flights) agree at 3.98 px while every size above four sits at 5.2–6.8.
  The crowd sitting exactly on the cap never clears (120 at 4, 71 at 6, 87 at 8),
  which is an even climb approximating a shape that is not a stair rather than
  stairs the model cannot hold. It stays at four, and `facing::MAX_TREADS`'s own
  doc carries the table. **What is left of this entry is the original defect**:
  the model's riser sits ~5 px from the drawn joint, which is a *placement*
  problem — where `boundary_columns` puts a crest — and neither the tie-break nor
  the cap moves it. The move that would is using the found edge to **correct the
  profile** rather than only to choose between axes: the same three calls that
  measure a residual can solve for the tread heights that minimise it, at which
  point `interiors_agree` stops being a tie-break and starts being the fit.
  <br>
  *Two smaller things the gate's own run turned up, both now closed.* **The
  exact-tie rule is stated.** `best_prism`'s interior tie-break used
  `Iterator::max_by`, which keeps the *last* equally-best candidate — the
  opposite of the outline-score tie one line above it (`if score > best.1`,
  strict, keeps the *first*). Two unstated conventions for the same kind of
  tie in the same function; replaced with `earliest_of_best_interior`, which
  keeps the first candidate on an exact match and is pinned by its own unit
  test built from hand-chosen `f32`s rather than a picture, since real art
  essentially never produces two candidates that agree with it to the last
  bit. **The two-tread residual gap is
  answered.** `prism_axis --debug` on its worst offenders (`0x4702` Magencia
  QuarterWall, `0x51DF`/`0x5237` Virtue Floor, `0xB11B` Zen Garden Large,
  `0x4627`/`0x4617`/`0x4621` the three Spire Slope/Base graphics, all two-tread)
  against `0x42FE`/`0x42FF` Large Stairs Carpet (three-tread) for contrast —
  looked at by eye, confirmed 2026-08-11: every worst two-tread offender is a
  floor, rug, or ramp, not a staircase. The wall detector's corner test and the
  outline-only `silhouettes_agree` score both pass a shallow brightness
  gradient across a flat or sloped surface as a two-step climb; a real flight
  is rarely just two treads, so the two-tread bucket is disproportionately
  these false positives rather than short real stairs, which is why its mean
  residual (5.76 px) reads worse than the three-tread bucket's (3.98) — the gap
  is population composition, not a geometric effect the model is missing.
  <br>
  **Two earlier framings died on the way here and are worth the lines.** *The
  boxes disagree about the climb axis* — six graphics, four axes — is true and is
  not a defect: `prism_axis` says every one of the six is confidently its own
  direction, and the structure really is a stoop with steps down three sides.
  *Interior faces at the joins between abutting tiles* — the "garbage on the
  vertical joins" `statics::push_volumes`' own doc records — does not reproduce
  either: `isolated_scene` prints `0x0751`'s treads at `x 99.000..102.000`, three
  tiles folded into one primitive, so `occlusion::merge` is doing its job here.
  <br>
  The face census that was the first evidence, kept because the ratio is the
  reason the error is visible at all:
  <br>

  | face met | fragments | share | flame term of 765 |
  |---|---:|---:|---:|
  | the **lid**, `+z` | 25,216 | 71.8% | 11.6 |
  | **east**, `+x` | 4,957 | 14.1% | **165.4** |
  | **south**, `+y` | 4,927 | 14.0% | 12.7 |

  <br>
  **The two side families are the same size** — 4,957 against 4,927, which is the
  symmetry the projection has. What separates them is 165.4 against 12.7: the
  lamp stands east, so of the two only the east one is lit, and every placement
  error in it is at full contrast against a lid beside it taking a fourteenth of
  the same flame. A misplaced *lid* would be invisible in this frame. That ratio
  is why an error inside the outline surfaces as "something extra drawn there"
  rather than as slightly wrong shading.
  <br>
  **What it is not**, all measured on the same frame. Not the ground poking
  through: `View::Kind` says *static* on every one of the strips' pixels. Not the
  art: the same albedo jump is in a frame rendered with
  `OPENSHARD_LIGHT_BRIGHTNESS=0`, and under flat light the flight is an ordinary
  grey staircase with no strips in it at all. Not the height: `0x0750`, `0x0751`,
  `0x0752`, `0x0754`, `0x0756` and `0x0758` are all 44×65 pictures and 43 + 4·5
  is 63, so the fitted five `z` are the art's own — this is not
  [`design_footprints.md`](../design_footprints.md)'s missing height reaching a climbable. Not the
  shadow walk: visibility averages 248.8 of 255. Not the merge: `0x0751`'s treads
  stand at `x 99.000..102.000`, three tiles folded into one primitive.
  <br>
  ⚠ **The first census of this was taken on the wrong set and is superseded by
  the table above.** It classified only the fragments that *changed* between two
  builds, which is a set defined by a shader rule rather than by the staircase,
  and it came out 88.6% lid / 11.3% south / **9 pixels** east — from which the
  east faces looked absent and the defect looked like a tie in `meets`. They are
  not absent; they are half the sides and they are the lit half. A set chosen by
  what moved is not a set chosen by what is there.
