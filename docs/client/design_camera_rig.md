# The camera, and the bench it is chosen on

The client has a camera and it is the reference one: the eye is the body's, to
the pixel, every frame. That is what ClassicUO does and it is not what this
client will ship, because it inherits the walk's discontinuities whole — a step
starts and the world starts with it, a rollback puts the body back a tile and
the world jumps a tile, a kiting reversal is a hard stop and a hard start
120ms apart. None of that is a bug in the follow; it is the follow having no
opinion.

Several cameras are wanted, and which one is right is not knowable from a
document. So this document is mostly not about a camera. It is about the two
things that make choosing one cheap: **one pipeline every camera is a parameter
set of**, and **a bench that scores a parameter set against a scripted walk**.
The cameras themselves are then a short list, each of them a struct literal.

The eleven decisions and the bench below are built. How they were built, and the
catalogue of general practice they were cut from, is
[`evidence/2026-08-14-the-camera-rig-record.md`](evidence/2026-08-14-the-camera-rig-record.md);
the three stages that are still empty are
[`plans/client/camera/PLAN.md`](../../plans/client/camera/PLAN.md). The
*geometry* the rig moves an eye through — the two pixel spaces, the zoom ladder,
the lock — is [`design_camera_shell.md`](design_camera_shell.md).

Written against `crates/client/render/src/control.rs`, `camera.rs` and
`mobiles.rs`, and against the walk harness in `crates/client/app/src/dst.rs`,
which already produces the thing this needs most: a per-frame trace of where a
body was drawn, on a virtual clock, with a wire and a shard behind it.

## The decisions

Numbered so one can be argued with alone.

### D1 — one pipeline; a camera is data, not an implementation

There is no `trait Camera`. There is one ordered pipeline and a `Rig` — a plain
struct of numbers and two or three enums — and every camera named below is a
value of it.

The reason is the bench. Two cameras written as two implementations are two
bodies of code with two quantisers, two cut rules and two rounding habits, and a
bench comparing them compares those as much as the feel. As one pipeline they
differ in exactly the fields that differ, and a defect in the shared path shows
up in every row of the table at once rather than in whichever implementation
happened to get it wrong.

It also makes the reference camera honest. `Rig::HARD` is not a special case
written in a hurry — it is every filter's time constant at zero, every zone at
zero, the cut threshold at zero. If the pipeline cannot express "the eye is the
body" as a degenerate parameter set, the pipeline is wrong, and finding that out
on the first preset is the point of starting with it.

When something genuinely cannot be a parameter — the RTS anchor is the candidate
— the pipeline grows a **stage** that the other presets switch off. It never
grows a fork.

### D2 — the target is decomposed: the ground and the height are two signals

`project` folds `z` into the vertical axis: a tile one unit higher is four
pixels further up the same screen column. So a camera handed a projected pixel
cannot damp height at all — filter that value and the walk is filtered with it;
do not, and every stair is a four-pixel step in the world's position.

The rig is therefore told:

```rust
/// Where the eye is asked to look, before anything smooths it.
struct Gaze {
    /// The body's ground position in world pixels, read at `z = 0`. Sub-pixel:
    /// this is a filter's input, not a sprite's placement.
    plane: (f32, f32),
    /// What its height lifts it by, in pixels — `z * Z_STEP`, kept apart so it
    /// can have its own clock.
    lift: f32,
}
```

and the eye is `plane - (0, lift)` after each has been filtered on its own terms.

This is also why `mobiles::world_position` is not what feeds it. That function
rounds to whole pixels, because a sprite is drawn on a texel grid — and a filter
fed an already-quantised signal is a filter fed a staircase, which is the
classic way to build a smoother that smooths nothing at low speed. The camera
gets an unrounded, undecomposed-into-`z` sibling of it, and the rounding happens
once, at the very end of the pipeline, where D7 puts it.

**Built, and it went one step further than that.** `mobiles::gaze` is the
formula and `world_position` is now `gaze(m).eye()` — one arithmetic, not two.
Written as two they were a pixel apart on about one frame in five thousand,
wherever the exact answer landed near a rounding boundary, and a camera and a
body that round separately by a pixel is a shimmer nobody can name or reproduce.
The cost is that "the eye is on the body" can no longer be proved by comparing
two independent formulas; what pins the arithmetic instead is `project`, which
is older than both — see C0's gate.

### D3 — every distance is a fraction of the screen; only time is time

Dead zones, lead caps, lean caps and cut thresholds are stored as fractions of
the drawn image's half-extents and resolved to world pixels at the top of each
frame. A camera tuned at `1x` then feels the same at `1/2x` and at `4x`, which
it does not if the numbers are world pixels — at `4x` a 40-pixel dead zone is a
tenth of the screen and at `1/2x` it is a fortieth.

Time constants stay in seconds. They are the one quantity a zoom does not
change.

### D4 — the order of the pipeline, including the stages that are empty

Order is where camera bugs live: a clamp before the filter springs off the
boundary, a shake mixed into the follow state drifts and never comes back, a
quantiser in the middle turns a filter into a ratchet. So the whole order is
fixed now, empty stages included, and a stage is filled in later without
anything moving.

1. **Anchor** — what the rig is looking at: the body's `Gaze`, a pinned point, or
   later a centroid of several. This is the only stage that knows what a player
   is.
2. **Intent** — additive offsets that say where the player wants to look rather
   than where they are: velocity look-ahead, cursor lean, an RTS offset. Each
   is smoothed on its own, then summed, then capped as one.
3. **Zone** — the dead zone, plus the idle timer that recentres out of it.
4. **Filter** — per-channel damping: `plane.x`, `plane.y`, `lift`. Frame-rate
   independent by construction (D5).
5. **Cut** — the discontinuities the filter must not be dragged across. Decided
   *before* the filter runs and it resets the filter's state, so a cut leaves no
   tail.
6. **Clamp** — empty. Nothing needs it while the anchor is a body, which is on
   the map by construction. It is in the order so that the day a free camera
   wants a map edge, the clamp does not get written above the filter.
7. **Impulse** — empty. Shake, recoil, a hit's kick: additive, on the pose,
   never on the filter's state. Reserved here for that reason and no other.
8. **Quantise** — round the pose to whole world pixels and keep the remainder in
   the state (D7).

### D5 — frame-rate independence is a property, and it is testable

The filter is `alpha = 1 - exp(-dt / tau)` per channel, or the critically-damped
spring of the same time constant where a spring's overshoot is wanted. Never
`lerp(x, target, 0.1)` per frame: that ties the feel to the frame rate, and this
client already has two frame rates on purpose — `FRAME_DELAY` when nothing
glides and `GLIDE_INTERVAL` when something does — so the naive form would
change the camera's character at the exact moment somebody starts walking.

This is not a matter of taste that has to be remembered. The bench runs each
script at 8ms, 16ms, 33ms and a jittered dt and compares the eye at matching
timestamps; the naive form fails it by a wide margin and nothing else does.

### D6 — a cut is an event, not a distance, and the distance is a backstop

A teleport, a facet change, a resurrection and a relock from across the map are
all *cuts*: there is no path between the two poses that anyone wants to watch.
They arrive as events, from the code that knows what happened.

The distance backstop — a gap wider than the cut fraction of the screen — exists
for the ones nobody remembered to raise, and it earns its place with an
argument, not with caution: if the body is off screen, easing to it draws a
smear of world nobody is looking at, over a distance nothing bounds.

The mirror of this is the one the reference camera gets wrong and which is half
of what this whole plan is for: a **server correction is not a cut**. The
rollback puts the body back onto a tile it did cross; a filter should absorb it
over a hundred milliseconds and the current camera relays it whole. So the cut
threshold has a floor to stay above: several tiles, not one.

### D7 — the fraction lives in the state; the pose is whole pixels

**Superseded in part by D11, which names *which* pixel.** What survives is the
placement of the quantiser and the reason for it; what does not is the
assumption that the pixel it rounds to is the art's.

The pose the camera is given is whole pixels, and the remainder stays in the
filter's state. An eye carrying a fraction puts every sprite on a half-texel
boundary for half of all camera positions, which does not show on a screenshot
and boils the whole frame in motion. This is the same rule the drag remainder in
`control.rs` already follows, applied one stage later, and it is why the
quantiser is last: quantise before the filter and a slow camera ratchets — it
sits still until the accumulated error crosses a pixel and then jumps one.

**The state is `f64`, and the filter is not what decides that.** `f32` is
plenty for a smoother: the far corner of a 7,168-tile facet is about 157,000
world pixels out, where an `f32` still resolves to a hundredth of a pixel. It is
the *rounding* that wants more. The eye has to land on the pixel the sprite
landed on, and a hundredth of a pixel of slack is a hundred times the margin at
which two roundings of the same position disagree — which is a shimmer that
appears only far from the origin, on some frames, and never in a test written
near tile zero.

### D8 — the rig is a pure function of one frame's input

```rust
fn advance(&mut self, gaze: Gaze, dt: Duration) -> WorldPixel
```

What the pipeline is told is the gaze, the cursor's offset from the viewport
centre, the image's half-extents, the zoom, `dt`, and any cut raised this frame.
No `Instant`, no `Camera`, no window, no `WorldMap`.

**Two arguments and not a `Frame` struct, until there are more of them.** The
plan said `advance(&Frame) -> Pose` and the code that came out of C0 says the
line above: stages 2, 3 and 5 are empty, so the input really is a gaze and an
elapsed time, and a wrapper around two values and a pose of one field is
structure that carries nothing. The list above is what those arguments *become*
— it is the shape of C5, not of today.

That signature is the whole bench. It runs ten thousand frames in under a
millisecond, it is what DST drives, it is what the app calls once per frame, and
there is no second copy of the arithmetic anywhere for the three of them to
disagree over. `Control` keeps its present job — it arbitrates who may move the
eye — and delegates the following to a `Follower` that owns a `Rig` and the
state.

`Rig` is `Copy + PartialEq` and the state is separate from it, for two reasons
that are both about the bench: a preset can be swapped while the client is
running without the world jumping, and a slider edit is a value that can be
printed, pasted into the source and committed as the preset it turned out to be.

### D9 — nothing here decides the default camera

The presets are named after their mechanism — `HARD`, `LIFT`, `SPRING`, `LEAD` —
and none of them is called `DEFAULT` until one has won on the bench and in the
window. A plan that names a winner in advance builds the bench to confirm it.

### D10 — the ease is one filter, and the question is which side of the sprite it is on

A walk starts at full speed. It has to: a body crosses one tile per hold at a
constant speed because that is what the wire says it does, and a sprite that
eased into its first tile would either arrive late — desynchronised from the
schedule everything else keeps — or exceed the walk's own speed in the middle of
the step to make the distance back. There is no profile that starts at rest,
covers a tile in a hold, and never goes faster than a walk. So an ease is not
something a step can be given. It is a **lag**, and the only question is who
carries it.

Two placements, and they are *the same filter on the same signal* — stage 4,
D5's form, one time constant:

- **After the anchor**, which is what D4 already describes: the sprite is drawn
  at the raw gaze and the eye trails it. The world eases and the character does
  not, so the character slides across the screen by the lag while it walks — six
  pixels at `tau = 0.08`, twenty at `0.25` — and its feet slide over the ground
  by the same amount.
- **Before the anchor**: the *drawn position of the body* is the filtered one,
  and the eye is hard-locked to that. The character and the ground keep their
  exact relative position and the pair eases as one picture. The lag is now
  between the sprite and the tile the server named, which nothing on screen can
  see, and nothing logical reads — the depth order, the atlas key and the anchor
  arithmetic all take `Mobile::at`, which is untouched, and that was already true
  for the glide (`mobiles.rs`).

The second is what this client does, and the reason is that the first answers a
question nobody asked. What is being smoothed is the *walk's* discontinuities —
a step starting, a step stopping, a correction arriving — and all of them belong
to the body. Filtering the eye instead smooths them by moving the world relative
to the character, which is a second motion invented to hide the first.

Three things follow, and each is what keeps this from being a new mechanism:

**One `approach`, two subjects.** The body's filter is `follow::approach` called
on a `Gaze`, exactly as the eye's is — two implementations of a damper is the
thing D1 exists to refuse. What it is *not* is a `Rig` field. A rig is the
parameter set of the eye, and the eye's pipeline begins by being handed a body
to look at; this is a property of that body, one stage earlier and one subject
over. The two were one struct for a day, on the argument that they are tuned in
the same sitting, and it read as though the camera were what moved the
character. `crowd::Ease` is its own type, beside the state it drives, and the
panel shows the two together because that is a fact about the sitting rather
than about the types.

**The state is per body, so it lives with the body.** The eye has one filter and
there is one eye; every mobile on screen is eased, so the state is per tracked
mobile in `crowd.rs` — which already owns the per-mobile clock and the glide, and
is already the layer that says where a body is drawn.

**A correction is a cut, and here the event is in hand.** D6 says a cut is an
event and the distance is a backstop; at this level there is no inference to
make at all, because `Crowd::snap` *is* the event — a rollback, a teleport, a
`0x20`. It resets the filter to the new position, so nothing eases across ground
the body never crossed. That is the same argument the backlog makes for moving
the lift's cut onto an event, one layer down and already available.

What it costs is a body drawn behind its tile for the length of a walk, and that
cost is the ease: the catching-up at the end *is* the ease-out, for free and
without a second rule. `Ease::NONE` is what the harness's corridors
run at — a body deliberately behind the oracle is not a body that failed to keep
up, and only a scenario that says which one it is measuring can tell them
apart.

### D11 — two pixels, and the quantum is the real one

There are two pixel sizes in this client and D7 did not distinguish them, which
is the whole of the defect this decision exists to remove.

**The virtual pixel** is the art's. The client's files fix it and we do not get
to choose it: a land tile is 44×44, a step in `x` is 22 across and 22 down, a
unit of height lifts four. Every sprite offset in the `.mul` is in it, and so is
the projection, the depth order and the distance a walk covers. It does not know
what a monitor is.

**The real pixel** is the display's. `real = virtual × zoom`.

At zoom 1 they are the same, which is why the difference stayed invisible for as
long as it did — and why `camera.rs` could say, truthfully, that the third space
has no type because nothing carries it. The rule is:

> Motion is continuous, and the one rounding is to the **real** pixel.

D7 put the quantiser last, which is right, and rounded to the virtual pixel,
which is not. The offscreen image the world is drawn into is `viewport / zoom`,
so at `2x` the eye's whole-virtual-pixel step is **two** pixels of the display
and at `4x` it is four: a scroll visibly coarser than the screen it is on, and
the same quantum under the drawn body, so a walk reads as juddering rather than
as a slow pan. The arithmetic makes it worse the better the monitor. A cardinal
step is 22 virtual pixels an axis over `WALK_HOLD`, which is 55/s — 0.92 of a
pixel per frame at 60Hz and 0.38 at 144Hz. Under a whole-pixel quantiser that is
an irregular run of zeroes and ones, so a higher refresh rate turns *more* of
the motion into stalls rather than less, and the zoom multiplies each stall's
height.

**Rounding to the real pixel costs nothing in sharpness, and that is not
obvious.** A shift of a whole real pixel at an integer zoom leaves every texel
exactly `zoom` real pixels wide — uniformly translated, not resampled — so
`nearest` still holds and the art stays as crisp as it is at 1:1. What buys that
is the integer ladder (below); at a fractional zoom the texel widths alternate
and the pattern crawls as the camera moves, which is a shimmer no placement of
the quantiser fixes.

**The state stays virtual, and only the rounding is real.** The tempting reading
of "compute movement in real pixels" is to hold the position in them, and it is
the wrong one: a walk covers 22 virtual pixels in 400ms because of what the art
is, not because of what the display is, so holding the state in real pixels
would rescale every filter's state and every glide's target on each notch of the
wheel, and would put the zoom inside the kinematics instead of at its one exit.
The picture on screen is identical either way — the quantum is the real pixel in
both, because the rounding is single and last. What differs is how many places
know about the zoom: one, or all of them.

**Therefore the zoom leaves the blit and enters the vertex transform.** The
offscreen-then-upscale arrangement *is* the coarse quantum: an image of virtual
resolution cannot express a real-pixel offset, wherever the fraction is kept.
The three world passes already take instance positions as `f32` and already
convert to clip space against a `size` uniform, so what they gain is a `scale`
and a centre that carries the eye's remainder — `screen = art * scale + centre`
— and the CPU keeps doing its arithmetic in virtual pixels exactly as it does
now. Every pixel-exact assertion in those passes is about art space and survives
untouched. What the client gains beyond smoothness is that a magnified world is
drawn at the display's resolution instead of at half or a quarter of it.

**The ladder becomes integral above 1:1 — 1x, 2x, 3x, 4x — and keeps its
fractional rungs below it.** The asymmetry is the argument rather than a
compromise. Magnifying, `4/3` and `3/2` bought a finer choice and cost the
shimmer above, which is a bad trade once the motion is smooth: a coarse ladder of
exact rungs reads better than a fine ladder of rungs that crawl. Minifying, the
same fractions cost nothing, because that path goes through the blit's linear
sampler and a filter is exactly the right answer to several virtual pixels
landing on one real one — so `1/2`, `2/3` and `3/4` all stay, and zooming out
keeps feeling like a slider rather than a switch.

**The gates.** Two, and the second is the one that catches what the first
cannot: a shift of `1/zoom` of a virtual pixel moves the picture exactly one real
pixel, at every rung; and a texel of art occupies exactly `zoom` real pixels for
*any* camera position, which is what fails the moment a fraction leaks past the
quantiser.

## The bench

The camera's failures have names, and each name is a script.

### Scripts

**Built, and it is a body's path rather than a player's inputs.** The plan said
one `Script` type for all three consumers, generalising `dst.rs`'s `Act`. That
turned out to be two different things wearing one name: the bench needs a *body*
(a gaze as a function of time, with no steer, no wire and no shard, which is what
makes it fast enough to sweep), and the DST harness needs *inputs* (arrows, at
instants, driving the real four units). Forcing one type would have meant either
dragging a walk pipeline into `client/render` or measuring a camera against a
body that arrived by magic.

What is shared instead is the thing that has to be: `Sample` and `Metrics`. The
DST harness records the same samples from the real pipeline and runs the same
metrics over them, and a test holds the two walks against each other — the
scripted body and the real one peak within five per cent. That is the claim
"one type, three consumers" was really after, and it is the one that can be
checked.

- the **pure bench** drives `Follower::advance` from a scripted gaze;
- the **DST sim** drives the real `steer.rs`, `Walk`, `Crowd` and a shard over a
  wire with latency and jitter on it, and feeds the camera what actually came
  out;
- the **window** replays a script's *knots* as events into the real `Crowd`
  (C4), so what the eye follows is the client's own glide rather than a formula.

The scenarios, and what each is for. `mash` and `mouse_swirl` are not built:
the first needs the input-level script the DST harness has and the bench does
not, and the second needs the cursor, which is C5. `frame_jitter` turned out to
be an axis rather than a scenario — `Cadence` — so every script can be run
jittered.

| Script | What it is for |
|---|---|
| `stand_still` | A flat line. Any motion at all is shimmer, and it is the cheapest possible test of D7. |
| `ten_east` | The baseline walk: lag, and whether the eye's speed is constant. |
| `back_and_forth` | The kite. A reversal every few steps — overshoot, settle time, and the reason the spring exists. |
| `mash` | Direction changes faster than the walk can answer, which is where the queue rule shows through into the camera. |
| `rollback` | A `0x21` mid-step. The correction must be absorbed, and *not* cut across. |
| `teleport` | A recall. Must be cut, and must leave no tail. |
| `stairs` | `z` up a few units at a time, walked — which the glide already smooths, as C2 found out. |
| `kerb` | Two units arriving *whole*, as a correction does. What the lift filter is really for, and what keeps the cut from firing on everything sudden. |
| `ledge` | Fifteen units arriving whole — one short of the cut, so the worst case a filter is ever asked to absorb. What picked `lift_tau`. |
| `dungeon` | A large drop at once: past the cut, and the boundary the rule is drawn at. |
| `mouse_swirl` | The cursor circling while the body stands. Lean's own jitter, with nothing else moving. |
| `frame_jitter` | Any of the above, with `dt` drawn from a jittery distribution. D5's property. |

### What is measured

Everything in **world pixels**, which is screen pixels at zoom 1 and the bench
does not zoom, and everything over the *eye's* trace rather than the body's —
but over **which** eye's trace is not a detail, and getting it wrong makes half
of these measure the quantiser. See C1's first finding; the split is written
next to each:

- **lag** — max and RMS distance from the drawn eye to the body. How far the
  camera trails.
- **overshoot** (`ahead_max`) — the furthest the drawn eye got *past* the body
  along its direction of travel. Negative everywhere means it never overshot,
  and `NaN` means nothing walked, which is a different claim from zero.
- **speed, acceleration, jerk** — the first, second and third differences per
  unit time, off the **unrounded** trace. Jerk is the number that means
  "ragged": a camera that changes its acceleration abruptly is one the eye reads
  as stuttering even when its path is smooth.
- **step variance** — the unevenness of the *drawn* eye's per-frame movement.
  The one a continuous metric misses: at a constant body speed, an eye that
  moves `0,0,3,0,0,3` and one that moves `1,1,1,1,1,1` have the same mean
  velocity and only the first is a ratchet. Measured only over the frames where
  the body was moving, and paired with `still_frames` — how often the body moved
  and the eye did not.
- **travel** — how far the drawn eye went in total. Half a metric and half a
  companion: it is what says the run was a run.
- **cut count** — not built, because nothing cuts yet. C3's, and the number that
  catches a camera that smooths beautifully by cutting whenever it falls behind.

Every one of them is asserted with a companion that says the data is real: more
than *k* frames drawn, more than *n* pixels travelled, the rollback actually
delivered, the two rigs given the same number of frames. A metric over a scene
where nothing moved is green and means nothing, and this repository has produced
that result before.

### What comes out

Three outputs, and the third is the one that decides anything:

1. **A table** — presets down, scripts across, one metric per cell, printed by
   the runner. The comparison is the primitive: a single camera's jerk figure is
   uninterpretable, and the same figure next to `HARD`'s is not.
2. **CSV and SVG** per preset and script, under `target/camera/`. The SVG is a
   polyline this repository draws itself — no plotting dependency for six lines
   of `<path>` — and presets are **overlaid on one chart**, because two curves on
   one axis is how raggedness stops being a feeling. A number that disagrees with
   the picture means the metric is wrong, and that has to be visible. Two panels
   per script: the eye's own speed, where a reversal is a square corner or a
   rounded one, and how far behind the body it was, which is what that corner
   cost.
3. **A scope in the window** — a strip chart of the last few seconds of eye
   velocity and jerk, drawn with `egui::Painter` lines, beside a preset picker
   and a slider per `Rig` field. From the moment this exists, choosing a camera
   is looking rather than arguing. **Built** — C4.

The metric functions take a slice of samples and nothing else, so the offline
runner and the live scope compute the same numbers from the same code. C4 took
that one step further: `bench::readings` is the *only* place a difference is
taken, `Metrics` is its peaks and every curve — the SVG's, the window's — is its
values. A number that disagrees with the picture beside it now means the metric
is wrong, which was the claim; two differencing loops would have made it an
argument.

