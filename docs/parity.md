# Parity: one frame, however it was asked for

A living plan. The backlog at the end is where the next session starts.

## The root

**Nobody holds parity, so it is not a property — it is a coincidence.** A frame is
assembled by calling `light::collect`, `ground::collect`, `statics::collect` and
`items::collect` in the right order with the right arguments, and that sequence
is written out by hand in at least seven places: `client/app/src/lib.rs`,
`examples/isolated_scene.rs`, `examples/two_cubes.rs`, `tests/cost.rs`,
`tests/frame.rs`, `tests/traced.rs`, `tests/attachment.rs`. Every one of them is
free to pass a different cutaway, a different grid, a different clock — and each
of them does.

(Eleven, counted properly while P1 was being done: `tests/onsite.rs`,
`examples/boxes.rs`, `client/render/src/scene.rs` and two more in a **different
crate**, `client/artscan`'s `examples/probe.rs` and `examples/grid.rs`. That the
first count was low by four is itself the point — nobody holds the list either.
P2 has all of them.)

What that costs is not theoretical. In one session (2026-08-10), chasing a
one-pixel artefact a person could see in the live client:

- `tests/cost.rs` collects statics against `Occlusion::EMPTY` **on purpose**, so
  every fragment in its frame takes the billboard fallback. Its `View::Normal`
  is not the client's, its `View::Solid` is uniformly black, and nothing said so
  — the frame it dumps looks like a frame.
- `examples/isolated_scene.rs` passed `None` for the occlusion bake where the
  client passes one, `Cutaway::OPEN` where the client passes the player's own,
  and translated every scene onto a synthetic anchor near the origin where the
  client works at Britain's coordinates. Each was found by reading, not by a
  gate, and each changed the picture: the anchor alone moves 760 pixels, 746 of
  them one-pixel runs.
- The artefact itself reproduced in the tool the whole time. It went unseen for a
  session because the tool's frame was being *searched* rather than *compared* —
  there was nothing to compare it to.

**The client is the thing that is broken, and the tool is the only thing that can
be inspected.** So long as the two are different assemblies, a defect visible in
one and absent in the other says nothing about either.

## The target

**One assembly, one set of inputs, and a gate that a frame is the same frame
whoever asked for it.**

Parity by *construction* first and by *comparison* second. A comparison gate over
two hand-written assemblies only tells you they diverged; a shared assembly makes
most of the divergences unexpressible, and then the gate is about the handful of
inputs that are genuinely allowed to differ.

## The decisions, made

**D1. One function assembles a frame, and every caller goes through it.** It
lives in `client/render` — the app is a caller like any other, not the reference
implementation. Nothing outside it may call `light::collect` /
`statics::collect` / `items::collect` / `ground::collect` in sequence.

**D2. Its inputs are one struct, and a caller that wants to differ says so by a
field.** Not by omitting a call, not by passing a different constant at one of
four call sites. Every field this session found divergent is a field: the
cutaway, the bake, the atlas, the tuning, the flame and animation clocks, the
ground items, the camera. A field that has no honest default has no default.

**D3. `Occlusion::EMPTY` stops being a way to assemble a frame.** A caller that
wants to price the pass without the impostor asks for that by a field on the
inputs, and the field is named for what it does to the picture, not for what it
saves. `tests/cost.rs`'s frame becomes a real one.

**D4. The tool builds at real coordinates by default.** `OPENSHARD_SCENE_ANCHOR_REAL`
exists and is off because the synthetic map near the origin is cheaper. Once the
cost of a Britain-sized `WorldMap::from_blocks` is measured and acceptable, the
default flips and the knob keeps the old behaviour for whoever wants it. An ulp
at 1660 is sixteen times an ulp at 100, and every tie at a shared plane is
decided in `f32`.

**D5. The gate compares planes, not pictures.** The lit frame folds every
disagreement into one colour; the G-buffer's planes — position, normal, solid,
place — each answer a different question, and a parity failure names which. The
gate reports the count of differing pixels per plane, and zero is the target.

**D6. A deliberate difference is a listed one.** Where a caller sets a field that
must change the picture (no roofs, no impostor, a different zoom), the gate is
run with that field *equal* on both sides. There is no allowance, no tolerance
and no ignore-list: an input that differs is set the same, or the case is not
gated.

## Phases

### P1 — the assembly, extracted ✅ 2026-08-10

`client/render/src/frame.rs`: [`frame::assemble`] takes one [`frame::Inputs`] and
returns one [`frame::Frame`] — the lighting, the ground quads and one
[`StaticGeometry`] with the server's items already absorbed into the map's. The
app's own order is kept and stated there (the grid before the pictures, since a
drawn row carries the number the grid gave the static it draws). The app and
`isolated_scene` are its first two callers, and neither calls a collector any
more.

**The open design question is settled** (it was the third backlog item): the
assembly takes *borrows in a struct built at the call site*, `bake: Option<&mut
Bake>` included. The app hands over `&self.map`, `&self.items` and `&mut
self.occlusion_bake` in one literal — they are disjoint fields, so nothing had to
move out of `App` and nothing is cloned.

Eighteen fields, no `Default`, and three of them are new as fields rather than as
constants somebody passed:

- **`sky: Option<Ambient>`** — `None` is the client's daylight, where no grid is
  built at all. It is what F10 actually switches, which is worth stating because
  it is *not* the impostor field below.
- **`impostor: Impostor`** — `Met` or `Billboards`, D3's field. The app passes
  `Met` always; `isolated_scene`'s `OPENSHARD_SCENE_IMPOSTOR=0` is the only
  caller that asks for the other.
- **`sun` / `carried` / `view`** — the three things the app used to do to
  `Lighting` *after* collecting it, three hundred lines further down the
  function. They are inputs now, and the comment where they used to be says
  nothing may touch the lighting between the assembly and the blit. A frame the
  client draws and a frame a tool dumps are the same frame only for as long as
  neither has an adjustment of its own afterwards.

*Done:* `examples/isolated_scene.rs` dumps seven planes of Britain's
`(1501, 1659)` — Lit, Place, Kind, Height, Normal, Solid, Occluders — **byte for
byte identical** before and after the extraction. `cargo test -p
openshard-client-render -p openshard-client-app` is green (454 + 142 and the GPU
suites), clippy silent.

*Not done, and it is the first backlog item below:* **the app's own half was
verified by reading rather than by a gate.** The client has no frame dump, so
there is nothing to compare; what was checked is that every argument arrives
where it used to, that `Lighting::NONE.sun` is `None` so the unconditional
`lighting.sun = sun` is the old conditional, and that nothing between the old
collect site and the old adjustment site reads anything of `lighting` but
`occlusion`.

### P2 — the remaining callers ✅ 2026-08-10, scoped down to what was real

The nine call sites this section used to list were never nine migrations —
reading each of them (`tests/cost.rs`, `tests/frame.rs`, `tests/traced.rs`,
`tests/attachment.rs`, `tests/onsite.rs`, `examples/two_cubes.rs`,
`examples/boxes.rs`, `client/artscan`'s `examples/probe.rs` and `examples/grid.rs`)
found that seven of them are hand-built diagnostic rigs whose whole point is to
bypass one or more of the four collectors on purpose, and one entry
(`artscan/examples/grid.rs`) never called any of the four collectors at all —
it calls `occlusion::collect` directly, a level below the four `frame::assemble`
wraps, and the count of "nine, not the five this plan first named" was wrong by
one in the other direction.

- **`tests/frame.rs`, `tests/traced.rs`, `tests/onsite.rs`,
  `examples/two_cubes.rs`, `examples/boxes.rs`, `artscan/examples/probe.rs`**
  stay outside the assembly. Each is a point-probe (`onsite.rs`, `probe.rs`
  print numbers about one coordinate and draw nothing) or a research rig that
  needs geometry no real static produces (`two_cubes.rs`, `boxes.rs`, most of
  `frame.rs`'s 57 tests build quads by hand to test one shader in isolation;
  `traced.rs` builds adversarial boxes to compare against a path tracer).
  Forcing any of these through `Inputs` would not remove a divergence — it
  would hide the fact that they are deliberately not drawing the client's
  world.
- **`artscan/examples/grid.rs`** is dropped from the list. It was never a
  caller of `light::collect`/`ground::collect`/`statics::collect`/
  `items::collect`; the original count was wrong.
- **`client/render/src/scene.rs`'s `Scene::lighting`, and by extension
  `tests/attachment.rs`'s use of it,** stay outside too, and this is not an
  oversight left for later — `scene.rs`'s own module doc says why: "It has no
  art. Nothing here can be drawn — `Scene::lighting` is the whole of what a
  scene produces." `Inputs::land`/`texmaps` are non-optional references with
  no honest empty value (D2: "a field that has no honest default has no
  default"), so satisfying `frame::assemble`'s shape would mean inventing one
  just to force a lighting-only fixture through a struct built for drawing.
  `tests/attachment.rs` rides the same fixtures to check a G-buffer owner
  attachment round-trips against the occlusion grid — a narrower question than
  "is this a frame" — and its one `items::collect` call is answering that,
  not standing in for `frame::assemble`.
- **`tests/cost.rs`** was the one real migration candidate, and even it isn't
  a `frame::assemble` caller: the whole file exists to time the collectors
  *individually* (CPU per stage, GPU per pass), which is exactly what
  `assemble()`'s one opaque call would stop being measurable. What was real
  and still open was **D3**, named here since this plan was drafted and never
  landed: `statics::collect` was still called against `Occlusion::EMPTY`,
  pricing a frame where every fragment took the billboard fallback —
  `View::Normal` not the client's own, `View::Solid` uniformly black, exactly
  the defect the plan's own opening section describes. Fixed: a real grid,
  built once over `light::lit_tiles` (the same rectangle `light::collect`
  grows its own grid over) and not itself timed, replaces `EMPTY`.

*Done:* zero of the nine originally-listed call sites needed to become
`frame::assemble` callers; D3's fix is the one line of work P2 actually had.
`cargo check`/`clippy -p openshard-client-render --all-targets` silent.

*Measured, per D3's own "record both", `OPENSHARD_CLIENT=… cargo test --release
-p openshard-client-render --test cost -- --ignored --nocapture` at
`(1495,1629,0)`, widest zoom, before and after the grid swap:*

```
case    EMPTY grid   real grid
copy       0.466ms     0.480ms
dark       0.647ms     0.647ms
far        0.654ms     0.656ms
night      1.876ms     1.875ms
sun        1.088ms     1.118ms
```

**The number D3 predicted moving does not move.** What was priced the whole
time is the lighting pass's own walk — the light-source raymarch inside
`statics.wesl`/the blit shader — and that reads `lighting.occlusion`, which
`light::collect` builds fresh in every case regardless of what grid the
*statics pass* met its boxes against. The grid this fix changed only decides
where a static's fragment sits (a real box's plane, or the billboard
fallback) — a correctness fact about the picture (`View::Normal`,
`View::Solid`), not a term in this file's own timing loop. D3's text assumed
the two were coupled; measuring says they are not, and that is worth having
written down rather than re-discovered by whoever next reads for a
performance regression here.

### The dump ✅ 2026-08-10 — P3's prerequisite

**Both ends can now be asked for a frame, and both answer in the same bytes.**
This was the first backlog item and it stood in front of P3: the gate needs two
frames of one place and only the tools' existed.

`client/render/src/dump.rs` is the one readback. [`dump::planes`] draws one
assembled frame once per [`debug::View`] — the same blit, the same lighting, the
same world image and G-buffer, nothing collected in between — and hands back a
PNG a plane. [`dump::read_rect`] is the copy underneath it, and it pads its own
rows and honours its own origin, which is what lets a dump come off an
arbitrarily-sized window and off a viewport a docked panel has pushed away from
the corner.

- **The client dumps on F12.** `App::frame_dump` is armed by the key and spent in
  `App::draw` after the frame's own submit: the ordinary frame, drawn by the
  ordinary passes, blitted again into a texture of its own once per plane. Not
  the surface — what is presented has the HUD and the solids overlay on it, and a
  tool's frame has neither. One directory a press, `<root>/frame-<n>/<plane>.png`,
  under `OPENSHARD_FRAME_DUMP_DIR` or the system temp. **Not**
  `OPENSHARD_FRAME_DUMP`, which the tools already read as the *file* their one
  picture goes to; one name meaning two things is the divergence this plan is
  about, in miniature.
- **`frame::Inputs::summary` is the other half of a dump.** A picture nobody can
  reproduce is what every dump before this one was: two frames that differ said
  nothing about *which* input differed, and the client's arguments were readable
  only by reading `App::draw`. Every field gets a line — including the four that
  cannot be stated, which say so — so two dumps diff. `isolated_scene` writes it
  beside its picture as `<dump>.inputs.txt` and prints it; the client writes it as
  `inputs.txt` in the dump's own directory.
- **`tests/dump.rs` assembles Britain's `(1501, 1659)` headlessly** — the map's
  own statics, the player's own cutaway, night with a flame in hand — draws it,
  and dumps every plane. It gates one picture per view at the size asked for, the
  view *reaching the shader* (Lit, Place and Normal cannot agree on a real
  street), and a readback off the corner at an unaligned width. Both controls
  were witnessed by mutation: made to ignore the view, and made to ignore the
  origin — each turns the gate red.
- Two hand-rolled readbacks died with it (`plan.rs`'s and `isolated_scene`'s),
  and with them `OPENSHARD_SCENE_VIEWPORT`'s 256-byte alignment rule, which was
  only ever that copy showing through.

**The first press killed the client, and that is the entry worth keeping.** The
dump drew into a texture of the *surface's* format, on the reasoning that the
blit's pipeline is built for it — and a surface is whatever the compositor
offered. Here it is `Rgba16Float`: eight bytes a texel, against a readback
measuring a row as `width * 4`, which is not a shorter row but a copy `wgpu`
refuses outright. Every test passed, because every test drew into
[`blit::WORLD_FORMAT`] — the tools' own format, four bytes, the one place this
could not go wrong.

Two things came out of it, and the second is the point:

- `dump::read_rect` takes the texel's size from `texture.format()`, and
  `tests/dump.rs` reads a rect out of `Rgba8Unorm`, `Bgra8Unorm` and
  `Rgba16Float` — a test that needs neither client files nor a drawn frame, and
  that fails on the old arithmetic.
- **The dump draws into `WORLD_FORMAT` and builds its own blit pipeline for it.**
  Even a four-byte surface would have been the wrong four: `Bgra8Unorm` reads
  back with red and blue swapped and nothing says so, and a dump exists to be
  compared against `isolated_scene`'s picture, which has always been RGBA8. The
  surface's format is a fact about the compositor, and a comparison cannot depend
  on one.

**Then it worked, and this is what a press leaves.** Two dumps from one session
at Britain, `/tmp/openshard-frame/frame-0/` and `frame-1/`: thirteen pictures of
`1919x2077` — the viewport at the magnifying zoom the client was on — and an
`inputs.txt` each. Their diff is four lines, and every one of them is something a
person did between the two presses:

```
camera     eye tile (1503, 1654) → (1505, 1657)      the body walked
sky        None → Some(Ambient { … })                F10, night on
flame_time 3.443795s → 13.605293s                    ten seconds passed
view       lit → normal                              F11
```

That is the whole of what the summary is for. Nothing else moved — the same
facet, the same 2,906,871 statics, the same cutaway (`max_z: 47`,
`no_draw_roofs: true`: the player is indoors), the same tuning, `bake = kept
across frames`, `impostor = Met` — so a difference between these two frames is a
difference in those four lines and in nothing that had to be reconstructed by
reading `App::draw`.

One line of `inputs.txt` reads oddly beside the directory and is deliberately
left alone: `view` is what the *window* was showing, while each picture beside it
is named for the plane it is. A note explaining that would be a line the tool's
summary does not have, and the two exist to be diffed.

### The shard's own furniture ✅ 2026-08-10 — the tool stops answering about half a street

**`isolated_scene` reads the shard's database, so what the server placed is in
its frame.** This was the first backlog item, and what it cost is written there:
four tools agreed there was no cabinet at Britain's `(1504, 1655)` because all
four read `statics.mul`, and the cabinet is two `decorations` rows.

`examples/shard/mod.rs` is the reader, a shared example module in
`examples/oracle/mod.rs`'s own shape and for its own reason — the alternative is
a second copy of two queries the day `tests/` wants them. **It cannot be a
library, and the rule this time is the workspace's**: `openshard-persistence` is
a *server* crate and `crates/client/*` may not depend on one, so the two tables
are read by SQL rather than through its `Store`. That duplicates seven column
names and six JSON keys, bounded on purpose: a rename in `sqlite.rs`'s `SCHEMA`
fails here loudly — SQLite has no such column — rather than quietly returning
nothing, which is the failure mode the whole entry is about.

- **`items` where `loc_kind = 0`** (the ground; `1` is inside a container and `2`
  is worn) and **`decorations`**, whose record is one JSON blob and so is
  windowed by `json_extract`. Both over the same `_AT ± _RADIUS` rectangle and
  the same facet the map came off — named once now, as `FACET`, because three
  readers agreeing about it by writing `0` out three times is three places to
  stop agreeing.
- **On by default, and every way it can fail to read is a panic naming
  `OPENSHARD_SCENE_SHARD=0`.** An in-memory `database`, a `postgres://` one, a
  file that is not there: each would otherwise draw a frame missing everything
  the server placed, which is the original defect with a new cause. The knob is
  the honest answer ("the map's art alone") and it has to be *asked for*.
- **Read-only** (`SQLITE_OPEN_READ_ONLY`), so a mistyped path cannot create the
  empty database that would report "the server placed nothing here", and a live
  shard's own file is readable while it holds it open.
- `OPENSHARD_SCENE_CONFIG` says whose shard (default `openshard.toml` in the
  working directory) and `OPENSHARD_SCENE_SHARD_DB` names a database directly. A
  relative `database` resolves against the **config's** directory, not the
  process's: a shard is run from where its config sits, and resolving against
  the tool's own working directory would make the answer depend on where
  somebody typed `cargo run`.
- `OPENSHARD_SCENE_EXTRA` survives, with its job changed: it was how a live
  decoration got in, by hand, and it is now how a *hypothetical* one does — a
  torch put where no torch is, to find out what it would light.

**`tests/shard.rs` gates it, and the rows that must *not* come back are the
gate.** A reader with no `WHERE` clause at all passes a test that only checks
the cabinet arrives, so the fixture holds a contained item, a worn one, the same
graphic on the next facet, and one tile past each edge — none of which may
appear — beside the two the entry is about and one on the window's own corner,
which must. No GPU and no client files: the database is written row by row. All
three controls were witnessed by mutation, each turning the gate red: the
`loc_kind` filter dropped, the decorations' facet condition dropped, and the
east bound made exclusive.

**And the summary says which.** `Inputs::summary` counts the items a frame drew
and has no way to say where they came from, so a frame missing everything the
shard placed and a frame with nothing to place read identically there. Three
lines beside it — `scene.map`, `scene.shard`, `scene.extra` — name the three
sources this tool's list is assembled from, `scene.shard` carrying the database's
path and a count of each table. The client's dump has no such block and cannot:
its list *is* the server's, arriving on the wire as it is placed.

*Witnessed, and by the thing that started it.* `_AT=1504,1655,27 _RADIUS=4` at
Britain, the player's own cutaway (`max_z: 47`, `no_draw_roofs: true`), run
twice either side of the knob:

```
scene.map   = 114 statics pulled from the map          both runs
scene.shard = 0 ground items and 6 decorations         vs. off
                → 120 items, 1 flames                  vs. 114 items, 0 flames
```

The six are the two bookcases the person asked about (`0x0A97`/`0x0A98` at
`(1505, 1656)` and `(1506, 1656)`), two more at `(1501, 1656)` and
`(1502, 1656)`, a door, and the street lamp at `(1507, 1658)` — and the lamp is
where the flame came from. **The frame the tool drew with the reader off has no
light in it at all**, which is the entry above in one number: not a dimmer
picture, a different world.

*Not done, and it is a backlog item below:* `tile_probe`, `onsite.rs` and
`geometry_census.rs` still read no database. The reader is a module three lines
of code away from each of them, and each is its own decision about what its
answer is *about*.

### P3 — the gate ✅ 2026-08-10

`tests/parity.rs`: one real place, assembled twice — the map's own statics
through `statics::collect` (the client's route) and the same statics pulled
into `GroundItem`s and drawn through `items::collect` (the tool's route) — and
every one of `View::ALL`'s G-buffer planes compared, pixel for pixel. The list is
read off `View::ALL` rather than counted here, because it grows: it was thirteen
planes when this was written and fifteen by the time the gate first ran green,
`normal` having been split into `normal-geometry` and `normal-sprites` by work
landing beside it (`5e52279`).
`dump::plane_bytes` is the prerequisite's other half: `dump::planes` with the
PNG step split off, so a comparison zips raw bytes instead of decoding files.

**The anchor is real, not translated.** `isolated_scene`'s ordinary anchor
(`SYN_ANCHOR_NEAR_THE_ORIGIN`) moves a place next to the synthetic map's
origin, which is irrelevant to a byte comparison and worse than irrelevant: a
G-buffer's position and place planes carry absolute world coordinates, so two
frames anchored at different numbers differ everywhere before a single input
is allowed to. The gate's own synthetic map is `WorldMap::from_blocks` filled
from the real map's own land, one for one, wide enough to hold every place it
looks at — D4's own knob (`OPENSHARD_SCENE_ANCHOR_REAL`) with the cost it
asked to have measured: 32ms in a debug build for a block covering all three
places below. D4's backlog item is closed for the size this gate needs; the
live tool's own default is untouched.

**Two real divergences turned up before it was green, and both were the
gate's inputs rather than the assembly it was gating:**

- **The occlusion grid is wider than what is drawn.** `light::collect` builds
  its grid over `light::lit_tiles` — `Camera::visible_tiles` grown by the
  widest flame's own reach — and the first version of this gate pulled the
  tool's statics over the narrower `visible_tiles` alone. A wall standing in
  that margin (off screen, still occluding) was missing from the tool's grid
  and present in the client's, and every plane downstream of the grid's shape
  disagreed by about 1.2% at all three places: `place`/`kind`/`height`/
  `normal`/`solid` from a fragment meeting a differently-shaped neighbour,
  `occluders` and `sky` from the grid itself. Pulling over `lit_tiles` instead
  closed it everywhere except the second finding below.
- **An atlas grown for what is drawn is not always an atlas grown for what
  occludes.** `occlusion::shape_of` reads a graphic's facing off the same
  static atlas the sprite pass uses, and falls back to the whole tile when the
  atlas does not hold it. The tool's item list already spanned `lit_tiles`
  (the first fix), but its atlas was still built from the *narrow* bound
  (`statics::visible_graphics`, `Camera::visible_tiles`) — the same bound the
  live client's own atlas grows from (`App::wanted_now`). A margin-band wall
  therefore had a real shape on the side whose atlas happened to hold its
  graphic for some other reason and the whole-tile fallback on the other, for
  the same occluder: forty-six percent of `solid` at the stair corner
  (`1497,1626,10`) before this was found, nothing at the corner this file
  already dumps (`1501,1659`), where no such graphic exists only in the
  margin. Both routes now grow their atlas over `lit_tiles` too, which is
  correct for what this gate is asking and is not a claim about whether the
  *live* client ever meets the same gap — see the backlog entry below.

*Done:* green at `(1501,1659)`, `(1497,1626,10)` and `(1504,1655,27)` —
5,507, 6,744 and 6,127 items respectively, every plane byte-identical at each. `the_gate_is_red_when_the_tool_forgets_the_maps_statics` is the
positive control: the map's statics dropped from the tool's route on purpose,
and every plane downstream of a drawn fragment goes red. `cargo test -p
openshard-client-render` is green apart from one failure in `tests/frame.rs`
already present in the tree from work landing concurrently with this
session's own (`843fec1`, unrelated to `dump.rs`/`frame.rs`'s assembly).

### The window's own parity — found 2026-08-10, and it is why no tool ever drew the client's artefact

**A vertical green line runs down every wall in the client's `View::Normal`, one
per tile, and not one tool has ever drawn a single one of them.** The plan's
opening entry says the artefact "reproduced in the tool the whole time"; that was
about a different artefact, and this one is the opposite case — the tool cannot
reproduce it at all, on any place, any route, any anchor. What decides it is the
**parity of the viewport**, which no tool and no gate has ever varied.

*The measurement, at Britain's `(1503, 1657)`, radius 20, `View::Normal`, the
same scene each time — a run of `+Y` pixels one column wide standing inside a
`+X` wall face:*

```
tool  1920x2080  4x       0        tool  480x520  1x    14
tool  1919x2077  4x    3484        tool  481x521  1x   835
client 1919x2077 4x    3285
```

One pixel of window width turns it on. The tool's own frames match the client's
in every other way the run distribution can be read: the genuine `+Y` regions
come out `29/44/264` real pixels wide at an even extent and `30/45/265` at an
odd one, and the client's are `30/45/265` — the same geometry, drawn one pixel
wider, plus a sliver everywhere a `+Y` face was zero pixels wide to begin with.

**Every one of the 3,484 slivers stands at `x ≡ 3 (mod 4)`, on eleven columns
88 real pixels apart** — half a tile, which is one wall static — and the
client's 3,285 stand on *the same eleven columns*. That residue is the whole
mechanism:

- `statics.wesl` and `mesh_face.wesl` place a vertex at
  `(screen - origin) * scale + viewport.size * 0.5`. At an **odd** extent
  `size * 0.5` is a half real pixel, so the world is centred half a pixel off
  the pixel grid; at an even one it is not.
- A fragment samples at `i + 0.5`. Even: the world coordinate behind it is
  `(i + 0.5 - 960)/4` — always `.125`, `.375`, `.625`, `.875` of a virtual
  pixel, **never a whole one**. Odd: `(i + 0.5 - 959.5)/4 = (i - 959)/4`, a
  whole virtual pixel exactly when `i ≡ 959 ≡ 3 (mod 4)`. At `1x` the same
  arithmetic makes *every* column whole, which is the 835.
- A box's own corner is at a whole virtual pixel by construction, so an odd
  viewport is the only way a ray ever passes exactly through one — and there,
  `impostor::meets`'s `far.x` and `far.y` are equal and its documented tie rule
  ("ties go to `z`, then `y`, then `x`") answers `+Y`. The line down the wall is
  each box's own **vertical corner edge**, drawn as a face. `place` says so: the
  sliver names the same tile as the pixel to its *right*, not the one to its
  left, so it is the near box's leading edge and not a crack between two.

The shader already states the principle this breaks, three lines above the tie,
for the lid: **"A face with no area is not a face."** A lid's `x` and `y` faces
are lines and are retired by `hi.z > lo.z`. A box's corner is a line too, and
nothing retires it — because it is not a *face* that is degenerate there but the
place where two faces meet, which no extent test can see.

*Why it is invisible in half the world, which is why it reads as a wall-only
defect:* the tie always answers `+Y`. In a wall whose visible face is `+Y` the
sliver is the wall's own colour and nobody can see it. Only a `+X` wall shows it
— and the client's frame has **no** `+X` slivers inside a `+Y` wall at all,
which is the asymmetry a symmetric geometry should not have.

*What this says about the gate:* P3 compares two routes at `900x700`, `tests/dump.rs`
draws at its own even size, `isolated_scene` defaults to one, and every screenshot
anybody has taken of a tool has been even. The client's window is whatever the
compositor hands it. **A gate that never varies a viewport's parity is green
about a defect that a person can see from across the room**, and it would not
say so.

**Repaired where the sampling is, not where the tie is** ✅ 2026-08-10. The
three vertex stages that end on `(pixel - origin) * scale + size * 0.5` —
`ground.wesl`, `statics.wesl`, `mesh_face.wesl`, and they must end on the same
line or they draw two pictures — now `floor` that centre. The world's middle
then sits on a pixel join at every extent, a sample sits at a half-integer over
`scale`, and no integer `scale` divides a half-integer: **no primary sample can
land on a whole virtual pixel at any rung of the ladder**, which is a proof and
not a margin. It costs half a real pixel of centring at an odd extent — below
the quantum `Zoom` is built around — and is a literal no-op at an even one.

*Measured, same scene, same command, only the build changed:* `1919x2077` at
`4x` went from **3,484 one-pixel slivers to 0**, and its whole run distribution
became the even viewport's, width for width — `4/8/12/29/44/264` against the
odd build's `1/5/9/13/30/45/265`. `cargo test -p openshard-client-render` is
green but for `frame.rs`'s `britains_statics_cover_part_of_a_frame_that_is_still_whole`,
which was already failing in the tree before this session and cannot be this
change: its viewport is `768x512` and `floor` of a whole number is that number.

**What this does not do is repair `impostor::meets`,** and the backlog says so
rather than letting a green screen stand in for a correct rule.

### What the two findings teach, and it is not about pixels

Written down because both cost a session's worth of looking in the wrong place,
and both mistakes are the kind that repeat.

- **A tool that never varies an input cannot be a control for it.** Every
  viewport ever used here has been even, by unanimous accident and never by a
  decision. The list of inputs a tool holds *fixed* is as consequential as the
  list it varies, and nothing anywhere wrote that list down. `Inputs::summary`
  is the closest thing we have to it — it names eighteen fields and does not
  name the one that decided this.
- **A written suspicion is not evidence.** D4 has said since it was drafted that
  the synthetic anchor makes ties at a shared plane "sixteen times more
  precisely decided" than the client's — an excellent story, the plan's own
  first hypothesis, and worth nothing here: real anchor against synthetic gave
  *identical* counts, plane for plane. The suspicion that is already in the
  document is the one that gets tested first and believed longest.
- **A detector fixed at one width answers about the zoom, not the geometry.**
  The first sliver counter looked for a run exactly one pixel wide and reported
  **0** at `4x` — where there were 272 runs of four, eight and twelve. Widths
  had to be a *distribution* before the picture said anything. A detector must
  report what it counted and at what scale, or its zero is unreadable.
- **Two pictures with the same colours can differ entirely in distribution.**
  Eyeballing a downscale showed nothing; a histogram and a run-width table
  showed the whole thing. The client's frame has eight colours in it — there was
  never anything to *see* at a glance.
- **When a repair makes a bad case unreachable rather than correct, say what
  stays broken and where it is still reachable.** The floor above is a proof
  about primary samples and says nothing about `light::sample` at a face's own
  centre.
- **Record the candidate you refuted.** The selection loop looked like a second
  tie and is not one; without that written down the next session proposes it
  again and spends the same hour.
- **A share is not a picture, and only one of the two answers "is this the thing
  a person is pointing at".** The fourth, and it nearly cost a repair. The rim
  rule's own probe reported 46 of 1,010 side-face fragments inside the quantum;
  read as 4.5% that is a fringe, and it was written down as *refuted*. Drawn, the
  same 46 are a band one fragment wide running the whole length of both of a
  body's top edges — and a line is the one shape the report was about. A ratio
  answers "how much of this population", never "what shape is it", and a defect
  reported as a *line* has already told you which question to ask. See
  `docs/lighting_rebuild.md`'s seam-probe entry for the numbers and the picture
  side by side.
- **A pass no tool draws is an input no tool varies** — the third one, added
  2026-08-10 and the first to cost a *feature* rather than a measurement. The
  client's frame has a pass in it that nothing else here draws
  (`SpriteRenderer::render_mask`, the silhouettes), it shares the statics pass's
  uniform block by design, and it wrote that block with a stale literal in the
  slot `impostor::Fringe` had just moved into. Since `queue.write_buffer` is
  applied at the *submission* and not where it sits in the encoder, the later
  write decided the earlier draw: F2 changed nothing on screen while every tool
  and every test measured all three states correctly. The list of what a tool
  holds fixed is not only its arguments — **it is also every pass it does not
  run**, and the frame the gates draw was a frame nobody actually renders.

### The place that was under the ground ✅ 2026-08-10 — why five planes gated nothing

**The backlog offered two readings and the measurement is a third one neither
covered.** The entry recorded that dropping the map's statics from the tool's
route reddened nine planes while `light`, `flames`, `shadow`, `reach` and `sun`
came back `0 of 630,000`, and asked whether those planes were blind to a
difference that large or were not being drawn from the frame under test at all.
Both are wrong. **At the place the control ran, those planes held one colour.**
A plane of one colour agrees with every other plane of one colour, and no
mutation of the geometry can make it disagree — the zero was never about the
lighting.

*What was measured, per plane, on one assembled frame at each of `PLACES`
(distinct colours where something was drawn, and the dominant colour's share of
the whole viewport):*

```
(1501,1659,0)    1 light    78.0% of the frame is cleared background
  kind 3 colours · light 2 · flames 2 · shadow 2 · reach 2 · sun 2
(1501,1659,27)   8 lights   no background at all
  kind 2 colours · light 81 · flames 81 · shadow 13 · reach 2 · sun 1
```

**The cause is one field, and it is a `z`.** `PLACES[0]` was
`Point::new(1501, 1659, 0)`, where the land is at `z = 20` and the floor
standing on it at `27` — a camera aimed twenty units under the ground. A place
is a *stance* and not a column, and `Cutaway::at` takes its storey from the same
number: the whole building above was cut away, 78% of the frame came back as the
cleared background, and `light::collect` found **one** flame — the carried one,
which no walk of the map can miss — where the same place at `27` finds eight.
Nothing reached a drawn pixel, so four of the five planes were background plus
one constant. The fifth, `sun`, is one colour at all three places and always
was: every frame here sets `sun: None`.

**`occluders` at 1,562 pixels beside `sky` at 41,925 has the same root and is
worth stating** rather than left as the anomaly the entry made of it. Both read
the same grid and the grids genuinely differ; what differs is *how much of a
frame each can speak about*. The sky field is a property of a **column**, so
removing a building opens the sky over every tile it stood on — 30% of what that
frame drew. The occluders view answers about the **cell the drawn fragment
itself stands in**, and in a frame that is three-quarters background and
cut away besides, almost every surviving fragment is land outside the house. It
is not a plane disagreeing with its neighbour; it is two planes with different
domains, read as though they had one.

*Repaired, and the repair is two lines and a gate:*

- **`PLACES[0]` is `(1501, 1659, 27)`.** The frame is a frame — no background,
  eight flames, and `light`/`flames`/`shadow`/`reach` carry 81/81/13/2 colours.
- **`plane_colours` runs before every comparison, on the client's own side, at
  every place and both parities.** A plane that is one colour over everything
  the frame drew is asserted red *there*, with its name in the message, so a
  count of zero differing pixels is only ever reported about a plane that could
  have differed. The background is excluded by its alpha and not by its colour:
  counting it would let a frame that drew nothing at all read as two colours and
  pass.
- **`CONSTANT_BY_CONSTRUCTION` is the list, and it has one entry.** `View::Sun`,
  because `sun: None`. D6 says an input that differs is set the same or the case
  is not gated; read from the other end, the same sentence says a plane the
  inputs flatten is not gated either — so it is *listed*, not tolerated, and the
  backlog carries what varying the sun would cost.

*Witnessed by mutation:* `PLACES[0]`'s `z` put back to `0` turns both
`the_map_route_and_the_item_route_agree_pixel_for_pixel_at_three_real_places`
and `the_gate_is_red_when_the_tool_forgets_the_maps_statics` red, naming the
`light` plane. Restored, all three tests in the file are green, and **the
positive control now moves sixteen planes where it moved nine**:

```
lit 213,581 · place 216,665 · kind 217,847 · height 217,631
normal 126,050 · normal-geometry 126,050 · normal-sprites 5,199
silhouette-art 16,999 · silhouette-box 16,999 · solid 191,058
occluders 145,407 · sky 309,198
light 79,068 · flames 77,956 · shadow 113,859 · reach 54,100
sun 0   ← the listed constant, and the only one
```

*What this teaches, and it is the same lesson as the window's parity:* **a
detector must report what it counted.** The gate had been printing "every plane
byte-identical" over planes that held one colour, and a person reading that line
has no way to tell a comparison that passed from a comparison that had nothing
to compare. `normal-sprites` at 90 pixels in the old run was the same signal and
was read as "no geometry in it by construction" — which is true, and was not the
question.

### P4 — the geometry, in census order

`examples/geometry_census.rs` counts what each box claims. Over 11,184 statics
around Britain's `(1501, 1659)`: 3.2% a fitted prism, 39.6% a lid, 25.4% panels,
31.8% a whole tile standing in for a shape nobody has. Crossed with that, 32.7%
are a point of no primitive and 15.1% are a `CLEAR` piece handed a box with real
height anyway.

The order follows the counts and the blast radius:

1. **A floor is a body** ✅ 2026-08-10 — see below.
2. **A `CLEAR` piece with a box** — 15.1%, and the pair that ends both the
   cornice entry and the floor entry. **Reframed 2026-08-10, not resolved: the
   two-way choice this line offered was never the one the code or the defect
   are shaped like.** Phase 6c already settled "stand in the grid or be a
   billboard" — every such piece has had a real box regardless of grid
   membership since then (`statics::push_volumes` calls `boxes_of`
   unconditionally). What none of them get is a *name* (`SolidId::NOBODY`), a
   third state this line never named, and "stand in the grid" would not have
   fixed the defect this line is paired with even if it were still open: the
   cornice glow and the furniture-seam dashes
   (`docs/lighting_rebuild.md`'s cornice entry and its seam-probe follow-up)
   come from a *neighbouring* lid's shadow test deciding a missed pixel by
   clamp geometry, and a piece's own identity only ever exempts it from
   shadowing *itself*. The candidates that would touch it are the cornice
   entry's own two ("keep the clamp" or "give a miss the sprite's own volume's
   face") — a third, "give the miss no facing instead of clamping it", is
   *ruled out there by the cornice entry's own reasoning* (lit from every side
   makes a blaze brighter) and was, separately, already tried as a *global*
   policy in `statics.wesl` ("One silhouette") and reverted for measuring a
   worse artefact elsewhere (a lattice of lit dots across every floor and roof
   seam, 2.38% of one frame). The seam-probe entry had mis-cited this as one
   of the cornice entry's own three ways out; both docs are corrected now. No
   code changed this session — this is a documentation-only pass.
   <br>
   **A defect that a name would have fixed, fixed without one** (2026-08-11), and
   it is written here because this line very nearly acquired a second reason to
   land. The dashed seam along a run of abutting counters is a fragment meeting
   an **interior** face, and `SOLID_NOBODY` in 270 of 270 of those pixels: a
   `CLEAR` piece is never named, so `push_volumes` keeps its per-tile box instead
   of `occlusion.solid(id).space`, and `merge::merged` — which folds a named run
   into one `Solid` whose space is the union — never gets to dissolve the join.
   Naming these pieces would have removed the seam by removing the surface.
   <br>
   It was the wrong lever, and `impostor::RIM` is the repair that landed instead:
   the same edge exists on an isolated table, with no neighbour to merge with, so
   a rule that only comes out right when something folded owes its correctness to
   an optimisation. **What that leaves for this line is its own case, undisturbed
   and no longer carrying somebody else's defect** — identity, the shadow rules
   that turn on it, and a grid 15.1% larger for surfaces no ray should stop at.
   <br>
   **And of those two, one is now measured and refused (2026-08-10).** "Give a
   miss the sprite's own volume's face" is `impostor::presented_face`: it ends
   the comb inside an overhang and draws a hard line where the overhang joins
   the art (0.30% → 32.59% of those pairs, 97.68% for panels), because **91.79%
   of the art bordering an overhang is the box's own lid** — an overhang hangs
   above its box. The clamp stays. `docs/lighting_rebuild.md`'s serrated-edge
   entry carries the table and `examples/discard_census.rs` can re-take it.
3. **The whole-tile stand-in** — 31.6%, the expensive one, because reducing it
   means measuring more art rather than writing a rule.
4. **`PANEL_THICKNESS`** — one slab straddling the tile boundary instead of two
   inset ones, which is `docs/lighting_rebuild.md`'s own backlog item.

Each of the four re-runs the census as its own done-when, and the numbers go in
`docs/lighting_rebuild.md`'s census section beside the ones above.

#### P4.1 — a floor is a body ✅ 2026-08-10, and the thickness is not the one the plan named

`occlusion::LID_THICKNESS`: a lid's box is `top - 1/64 .. top`, and every rule
that existed because a lid was a *plane* is gone with the plane.

- **`light::crosses` is deleted, in both languages.** It answered whether a lid
  was in the way as a crossing of a plane rather than a passage through a box,
  and it needed a strictness argued at length — a candle standing on the floor
  it lights sends every ray from that floor's own `z`. `ray_vs_solid`'s exact
  slab test is both halves now. `blit.wesl`'s copy went with it, which is the
  only way a formula written twice goes away at all.
- **`walk_primitives`'s `Edges::NONE` arm is the body's arm**, one `match` arm
  for both.
- **`solid::drawn` and `DRAWN_LID_THICKNESS` are deleted.** The debug view
  fattened a lid by two `z` units so a person could see it — twice as deep as
  the walk met it. A view of the geometry that draws somewhere the renderer is
  not is the one failure an instrument may not have; it draws `solid.space` now,
  and `drawn` had no other caller.
- `occlusion::merge` needed nothing: it keys on `edges` and spans, so lids fold
  as bodies without being told. `impostor::meets`'s "a face with no area is not
  a face" guard stays — it is a rule about the geometry it is handed, and a
  test may still state a degenerate box — but no box `box_of` builds trips it.

**The gate said no to `Z_STEP`, and that is the entry worth keeping.** The plan
named one `z` unit, "the quantum the wire states a height in". A floor's depth
is invented *into the room below it*: the client's model has the wall of one
storey and the floor of the next meeting at exactly one plane, so any downward
thickness lowers that room's ceiling. Measured on `scene::storey_over_a_torch`,
with a sconce two units under it:

```
z 20.00   0.0298  ← the ambient exactly
z 19.94   0.0298
z 19.50   0.0298     the whole of 19..20, in shadow
z 19.06   0.0298
z 19.00   0.2872  ← lit
z 18.00   0.2914
```

One `z` unit is four screen pixels at 1:1: **a dark band along the top of every
interior wall under a storey**, where everything below it is lit.
`a_room_lights_its_own_wall_and_not_the_storey_over_it` is the fixture
`docs/lighting_rebuild.md`'s floor entry named as the one that catches both
directions, and it caught this.

So the number is argued from **both** ends instead of taken from the wire, and
`LID_THICKNESS`'s own doc carries it: above the wire's `f32` ulp at `z = 128` by
a factor of a thousand, and under a quarter of a *real* pixel at `4x`, the
deepest rung the ladder reaches. At that depth the ray in the measurement above
drops out of the slab before it reaches the neighbouring lid's footprint at all
— the geometry answers "not in the way" rather than a rule answering it — and
every scene is green.

**A rule that was tried and refuted, recorded so it is not tried again**: *a ray
that starts inside a primitive is not stopped by it.* It rescues the band on its
own terms — a fragment buried in a volume is not shadowed by the volume around
it — and it breaks the wall run: `light_runs_along_a_wall_and_stops_across_it`
and `a_merged_run_answers_every_ray_the_way_its_own_pieces_did` both go red,
because a face pixel is drawn `INSIDE` its own panel's slab and the panel beside
it contains that point too. The exemption for a surface cut into several
primitives is `on_the_lit_surface`'s plane test and it is already there.

*The census, re-run as this step's own done-when* (`1501 1659 20`, and the
shares are not the ones at the head of P4 — `8ba38f1` moved four of them under
this step, so read it as this tool's answer today rather than as a delta):

```
1381 statics on 41x41 tiles, 138 distinct graphics
   87   6.3%  a fitted prism, one body a tread
  484  35.0%  a lid — measured, LID_THICKNESS deep
   18   1.3%  a measured footprint
  418  30.3%  panels on the named edges, PANEL_THICKNESS deep
    7   0.5%  whole tile, a climbable that would not fit
  367  26.6%  whole tile, the art would not say
  403  29.2%  a point of NO primitive, 300 of them (21.7%) with real height
```

**The last line is the one this step could have moved and did not.** "Real
height" is a box with a side face a fragment can be answered with, and every
lid in the world has one now — so the count was made to ask
`max.z - min.z > LID_THICKNESS` rather than `min.z != max.z`, which is the same
question it was always asking. A share that jumped without a box growing a face
anybody can meet would have been the census lying about its own change.

*Done:* `cargo test -p openshard-client-render` is green apart from
`frame.rs`'s `a_sprite_pixel_meets_the_same_box_on_both_sides`, which is
`62a18a5`'s — that commit takes the `discard` out of `statics.wesl` and the test
still asserts a miss is not drawn. The lighting suite (44), `traced` (8),
`parity` (3) and `dump` (6) all run at Britain with the client's own files.

### P5 — the window-parity finding, made permanent ✅ 2026-08-10

**It was held by an argument in a comment; it is held by four gates now, and
building them found the case the argument was wrong about.**

*Done:* removing the `floor` from `ground.wesl` turns
`tests/parity.rs`'s `a_frame_at_an_odd_extent_is_the_even_one_with_a_column_added`
red at **54,252 of 630,000 pixels**, and the failure names the plane. Witnessed
by mutation, restored, green again. `cargo test -p openshard-client-render
--lib` (465) and `--test parity` (3, at Britain with the client's own files) are
green, clippy and fmt silent.

**G1 ✅ `camera::tests::no_primary_sample_lands_on_a_whole_virtual_pixel`.** The
arithmetic, no GPU: every rung of the ladder, both parities of both axes, every
eye fraction the quantum can express, ~121,000 samples. It asserts the *quantity*
and not the absence — with the eye on a whole virtual pixel the nearest any
sample comes to one is exactly `0.5 / scale` — and it reports how many samples it
looked at, because a sweep that silently covered one column would satisfy every
assertion in it. Witnessed by mutation: `Projection::centre` halving as a float
turns it red at the odd extents.

`Projection::centre` is the new home of `floor(viewport.size * 0.5)` on this
side, and making it one function found a second copy that had already
diverged: **`Camera::to_viewport_exact` centred on `width / 2.0`** where the
passes centre on `floor(width / 2)` and `Camera::pick` already floored. Half a
*real* pixel of disagreement between where the world is drawn and where a
highlight is painted over it, at every odd extent — the finest offset a display
can show, on the axis nothing ever varied. Fixed with the rest.

**G2 ✅ `tests/parity.rs`, and it is the gate the repair itself hangs on.** Two
frames of one place, `900x700` and `901x701`, compared plane for plane over the
rectangle they share. What makes it a comparison about *one line of shader* is
integer division: `render_width() / 2` is 450 for both, so the eye, the
projection's origin, `visible_tiles`, every collected static, every atlas and the
whole occlusion grid are identical — both premises asserted in the test rather
than described. All that is left to differ is `floor(viewport.size * 0.5)`, and
floored, the odd frame is the even one with a column and a row added.

The route gate runs at both parities at all three places too (six assemblies
where there were three; the whole file still runs in six seconds).

**G3 ✅** `Inputs::summary`'s camera line ends `image 901x701 (odd by odd)`. It is
derivable from the two numbers beside it and it is written anyway, because the
whole finding is that nobody ever read it *as an input*: a person diffing two
dumps scans field names. Gated in G2's own test — two frames a pixel apart have
to diff in the summary, and they do.

**G4 ✅ `impostor::a_ray_through_a_boxs_own_corner_is_answered_by_the_order_of_three_ifs`.**
The tie is now *stated*: a ray exactly through the vertical corner reads `+Y`,
the `z` ties read `+Z`, and a lid answers with its own plane. The ties are built
as exact `f32` equalities and each premise is asserted, so a case that stopped
being a tie cannot pass quietly. And the knife edge is in it: one ulp either side
of the corner flips the answer between `+X` and `+Y`, which is what "emergent
rather than stated" means. It is a record and not an endorsement — the backlog
entry above it stands unchanged.

**What building the gates found, and it is a correction to this section's own
proof.** "No integer `scale` divides a half-integer" is about the *extent*, and
the eye contributes a fraction to the same sum. The eye is snapped to
`quantum = denominator / numerator`, which at **`2/3x` is 1.5** — so half of all
camera positions there put the eye exactly half a virtual pixel off, the sum
comes out whole, and the ray goes through the box's corner again. Only that rung:
magnifying, every fraction is `m / scale`, which is the case the proof covers; at
`1/2x` the quantum is 2 and the fraction is always zero; at `3/4x` it is `4/3`,
whose fractions are thirds. G1 names the rung in
`AN_EYE_ON_A_HALF_PIXEL_REACHES_THE_CORNER`, asserts that the defect *reproduces*
there, and turns red if any other rung reaches it — a list that covered nothing
would be indistinguishable from a repair. The backlog carries what repairing it
would cost.

*Not done:* **`tests/dump.rs` still draws at even extents only.** G2 named it
beside `tests/parity.rs` and it was left alone because another session was
editing that file; the frame-comparison gate above is the stronger half and it is
in place. One case at `901x701` there is still worth having, and it is a backlog
item now.

<details><summary>The plan as it was written</summary>

**Today the fix is held by an argument in a comment.** Delete the `floor` and
every test in this repository stays green, because every one of them draws at an
even extent — which is the same unanimity that hid the defect for as long as it
existed. A finding that only a person can re-check is a finding on its way to
being re-derived.

**G1 — the invariant, with no GPU in it.** For every rung of `Zoom::LADDER`,
both parities of both axes, and every eye fraction that is expressible, assert
that no primary sample lands on a whole virtual pixel: the sample sits at a
half-integer over `scale`, and no integer `scale` divides a half-integer. This
is arithmetic on `Camera` and `Projection`, it runs in microseconds, and it
states the property rather than the symptom. **Witness it by mutation** — the
`floor` removed has to turn it red, or it is not the gate it claims to be.

**G2 — an odd case in the picture gates.** `tests/parity.rs` at `901x701`
beside its `900x700`, and `tests/dump.rs` likewise. Cheap, and it is the half
that catches a *different* commensurability nobody has thought of yet — G1 only
gates the one we now understand.

**G3 — the summary states what the parity is.** `Inputs::summary` prints
`image 1919x2077` and nothing says that the odd is an input. Two dumps whose
only difference was the window's width by one pixel would today diff in no line
at all, which is precisely the failure the summary was built to end.

**G4 — the tie in `impostor::meets` gets an answer or a stated reason.** The
Rust twin is reachable by anything that samples deliberately rather than through
the pixel grid: `light::sample` at a face's own centre, a probe handed whole
coordinates, a test. At minimum a case that feeds it a ray exactly through a
box's corner and asserts what comes back, so the rule is *stated* rather than
emergent from the order of three `if`s.

*Done when:* removing the `floor` from any one of the three vertex stages turns
a test red, and the test names which grid met which.

</details>

## Backlog

- 🚩 **A dump overwrites the last one, and the pair it exists to make is the
  pair it destroys.** `App::frame_dumps` counts from nought at every launch, so
  the second run's first press lands on `frame-0` — the directory the first
  run's first press wrote. Two dumps to be diffed therefore survive only inside
  one session of the client, which is exactly the case a person does not have
  when they close the window, rebuild, and press F12 again to see whether a fix
  landed. It cost real evidence this session: the frame the seam's first
  measurements were taken from is gone. The counter wants to continue from what
  the directory already holds, which is one read of `frame_dump_root()` at
  start-up.
- 🚩 **`inputs.txt` does not say which fringe drew the frame, and a key now
  changes it between two presses.** `frame::Inputs::summary` states every field
  of a `Frame`, and `impostor::Fringe` is not one — it lives on the renderer
  (`SpriteRenderer::set_fringe`), read from the environment once at start-up and
  cycled by F2 since `e4c51b2`. So the switch that changes **6.7% of a lit
  frame** (`docs/lighting_state.md`'s own table for `discard`) is exactly the
  kind of difference the summary exists to name, and two dumps of one session
  can now differ in it while their `inputs.txt` diff is empty. This is the same
  lesson as the window's parity one line over: *a detector must report what it
  counted.* What it is **not** is a one-line addition — the field is on the
  renderer and the summary is built from the frame, and `isolated_scene` reads
  `OPENSHARD_FRINGE` for its own copy, so both halves of the diffed pair have to
  gain the line together or the diff grows a permanent difference.
- 🚩 **A body of no height is still the plane a lid stopped being.** P4.1 gave
  `Edges::NONE` a span; `Edges::ANY` with a `tiledata` height of zero comes out
  of `Solid::box_of` with `min.z == max.z` exactly as a floor used to, and every
  argument in `LID_THICKNESS`'s own doc applies to it word for word. What is
  *not* the same is where its invented depth would go: a lid's is under a
  surface a walker stands on, and a zero-height body has no such surface to hang
  it from — which end it grows at is a question about the art rather than about
  the geometry, and `occlusion::Solid`'s type doc names this pair as the reason
  the kind is carried rather than derived. Nobody has counted them: the census
  reports the *claim*, not the span, so the share of the world this is about is
  unmeasured.
- 🚩 **P4's own census numbers are older than the tool that produced them.** The
  head of P4 reads 11,184 statics, 39.6% a lid, 15.1% a `CLEAR` piece with real
  height; P4.1's re-run at the same place and radius reads 1,381 statics and
  35.0%/21.7%. Some of that is `8ba38f1` ("four shares move") and some of it may
  be a different radius nobody wrote down — the run above states its arguments,
  the older numbers state none. Steps 2–4 each re-run this census as their
  done-when, so whichever of them goes first wants the head of P4 restated from
  a run whose arguments are in the document beside it.
- 🚩 **`impostor::meets` still answers `+Y` for a ray through a box's vertical
  corner, and nothing above reaches it any more.** The centring below makes a
  primary sample unable to land on that edge, which takes the artefact off the
  screen and leaves the rule underneath as wrong as it was. It is reachable by
  anything that samples a boundary deliberately rather than through the pixel
  grid — `light::sample` at a face's own centre, a probe given whole
  coordinates, a test. **P5's G4 is that test**, and it changes nothing here
  except that the rule is now written down where it can be read (`+Y` at the
  vertical corner, `+Z` at either `z` tie, a lid's own plane for a lid) with the
  one-ulp flip beside it. What the edge honestly wants is the face the
  *neighbouring* box continues, which no per-box meet can see: flipping `x` and
  `y` in the tie only moves the artefact into `+Y` walls, where it happens to
  be invisible, so it is a change of which half of the world lies rather than a
  repair. **A candidate considered and refuted:** the selection loop in
  `statics.wesl` is not a second tie — it ranges over the *one instance's* own
  volumes (`in.volumes.y`), and the neighbouring wall is a different instance
  decided by the sort and the depth test, so there is no adjacent box for it to
  have preferred.
- 🚩 **At `2/3x` the eye's own quantum puts a sample back on a box's corner.**
  Found by P5's G1, which is the first thing that ever varied an eye fraction
  and an extent parity together. The centring repair proves a property of the
  *extent*; the eye contributes a fraction to the same sum, and at `2/3x` the
  quantum is `1.5`, so half of all camera positions there sit exactly half a
  virtual pixel off and the sum comes out whole. Only that rung — see the P5
  entry for why the other six are covered. What it costs on screen is milder
  than the original artefact (minified, the blit's linear sampler blends the
  column away) and the *G-buffer* is as wrong as it ever was, which is what the
  lighting reads. Repairing it is a decision about **motion**, not about
  centring: dropping the eye's fraction from `Projection::origin` on the
  minifying path would cost about a third of a real pixel of smoothness at
  `2/3x`, which is `docs/camera.md` D11's own subject and not something to
  change in passing. `AN_EYE_ON_A_HALF_PIXEL_REACHES_THE_CORNER` in `camera.rs`
  is the list, and G1 turns red if it ever covers nothing.
- 🚩 **`tests/dump.rs` still draws at even extents only.** P5's G2 named it and
  P5 did not do it — another session held the file. One case at `901x701` beside
  its `900x700`, which is cheap, and `isolated_scene`'s own default is even too.
  `tests/parity.rs`'s odd-extent comparison covers the *shader's* centring
  already; what this would add is the readback and the PNG path at an unaligned
  odd width.
- ✅ **The live client's own atlas may be narrower than the grid it is read
  for** — fixed 2026-08-10. Found while building P3's gate, and not something
  the gate itself needed to fix: `App::wanted_now`/`wanted_since` grew the
  client's static atlas over `camera.visible_tiles()`, but `light::collect`
  builds the occlusion grid over the wider `light::lit_tiles` — the same
  bound, grown by the widest flame's own reach — and reads *that same atlas*
  for an occluder's facing (`occlusion::shape_of`). A wall standing only in
  the margin between the two bounds — off screen, still occluding — fell back
  to the whole-tile shape there whenever no other reason had already put its
  graphic in the atlas, on the *live* client and not only in a tool. P3's own
  gate had already closed this gap for the tool by growing both routes' atlas
  over `lit_tiles`; the live client's three atlas-growing call sites
  (`wanted_now`, `wanted_since`, and the eviction-rebuild fallback in
  `draw()`) now do the same — `light::lit_tiles(camera, tuning)` in place of
  `camera.visible_tiles()`, with `tuning` threaded into `wanted_since` for it
  and `self.covered` set from the same widened rectangle at every write, so
  the band-difference walk stays in one convention throughout. `cargo test -p
  openshard-client-app` (143 tests), clippy and fmt are silent.
  **Still unverified: nobody has walked a build past a margin-band wall
  before/after this to see the picture actually change** — no test in
  `client/app` asserts what rectangle an atlas is grown over, and this fix
  closes the gap by reading rather than by a gate. That absence of coverage,
  not the wiring, is what is left to look at next.
- ✅ **P3's positive control left five of the lighting's own planes untouched**
  — diagnosed and closed 2026-08-10; see "The place that was under the ground"
  below.
- 🚩 **`(1501, 1659, 0)` is in three more places, and it is the same empty
  frame.** `tests/dump.rs`'s `AT` is that point verbatim, so every picture that
  file gates is three-quarters cleared background with one flame in it — the
  planes it dumps are the ones this section just showed to be constant there.
  And `docs/silhouettes.md` has the *symptom* written down already, diagnosed
  as far as it could be: `frame::assemble` at that place "returns 595 quads of
  land and **zero** static quads", read there as "the cull is right and the
  scene was the wrong scene". It is right, and the reason is the `z`: the land
  is at 20, the floor at 27, and a camera at 0 cuts the building away above
  itself. Three documents met the same number from three directions and none of
  them owned it. What is *not* obvious is what each of those callers should be
  aimed at instead — `dump.rs` gates a readback and may not care, and the `4x`
  measurement in `silhouettes.md` would have to be re-run — so this is a
  finding to spend, not a rename to apply.
- 🚩 **No frame this gate draws has a sun in it.** `sun: None` at every call
  site, so `View::Sun` is one colour at all three places and is in
  `CONSTANT_BY_CONSTRUCTION` for that reason. The sun is the one lighting term
  with no falloff and no place — one direction for the whole world, walked
  through the same grid by `sunlight()` — and nothing here has ever compared
  two routes with it on. A fourth case with a sun would gate it; what it costs
  is one more assembly per place.
- 🚩 **The sky *field* never reaches `lit` in any frame anybody compares.**
  Both the gate and the client's own default flatten the night ambient
  (`Ambient::flattened`, `App::sky_field` off), which folds the sky's share into
  the ground term and leaves `lighting.sky` at zero — so `share` multiplies
  nothing and the field cannot move a lit pixel. `View::Sky` still draws it, and
  that is how the positive control reddens 309,198 pixels there while `light`
  moves for an entirely different reason. It is an honest default on both sides
  and therefore not a parity defect; what it means is that the whole of
  `docs/lighting_world.md`'s subject is gated by exactly one plane, and by no
  frame anybody looks at.
- 🚩 **The other three tools still read no shard database.** `isolated_scene` now
  does (see the section above), and `tile_probe`, `onsite.rs` and
  `geometry_census.rs` do not — so each of them still answers "there is no
  cabinet at Britain's `(1504, 1655)`" about a cabinet a player can see. The
  reader is `examples/shard/mod.rs` and reaching it is a `mod shard;`, so the
  work is not the plumbing: it is deciding, per tool, what its answer is *about*.
  A census of *the art's* geometry is honest to exclude what no art file holds
  and dishonest to be read as a census of the world; a probe of a tile is
  dishonest either way, because a person points it at a place and not at a file.
  Say which in each tool's own doc, whichever way it goes.
- ✅ **The shard reader windows by the tool's radius; the client windows by what
  the server sent** — fixed 2026-08-10. `_AT ± _RADIUS` is a rectangle chosen to
  keep a house from standing beside the thing under test, and it decided which
  *lights* were in the frame too — the street lamp four tiles outside it lit the
  pavement in the client and nothing here. `light::light_margin_tiles`, the same
  margin `light::lit_tiles` grows the occlusion grid by, is now public for
  exactly this: a caller with no `Camera` (a database window keyed on a stated
  point, not a viewport) still needs the reach a light can cross. `isolated_scene`
  reads `env_tuning()` before building the shard window and pulls both
  `items`/`decorations` over `_AT ± (_RADIUS + light_margin_tiles(tuning))` —
  wider than the map statics' own `_AT ± _RADIUS`, which stays as it was: the
  geometry a house is made of does not reach past its own walls, only the light
  a lamp inside one does. `cargo test -p openshard-client-render` (468 + 6 in
  `tests/shard.rs`), clippy and fmt are silent.
  **Not done:** nobody has run this against a real lamp sitting in the widened
  band to see the frame's own `light` plane actually pick it up — the fix closes
  the window by reading `light_margin_tiles`'s own reasoning, not by a gate on a
  place chosen for it. The item below (a gate needs a place where the lighting
  is reachable) is the natural place to witness this once it is done.
- 🚩 **A stacked ground item's graphic may not be the column's.** The reader
  takes `items.graphic` as it stands, and an `items` row also carries `amount` —
  which `crate::items`'s own doc says is deliberately not a `GroundItem` field,
  because "a pile of 500 gold is one sprite, and which sprite is the caller's
  question". Whether the *server* resolves that before the wire or the app does
  it on the way in was not established this session. If it is the app, a pile the
  tool draws is the wrong sprite and nothing says so. One reading of
  `crates/server/items` settles it; the reader either inherits that rule or
  states that it does not.
- 🚩 **Map statics reach the tool's frame through `items::collect` and the
  client's through `statics::collect`.** `isolated_scene` builds a synthetic map
  that carries no statics at all (`WorldMap::from_blocks`) and pushes the real
  map's statics into `Inputs::ground_items` as `GroundItem`s. Both paths call
  `statics::push_volumes` with the same `boxes_of`, so the *boxes* agree; what
  does not obviously agree is everything around them — the owner key, the sort
  (`items::collect` sorts by `depth::Order` and ties by the caller's order,
  which for the server is serial and here is a nested `x`/`y` loop), and
  `highlight`. D1 gave the two a shared assembly; it did not give them a shared
  *route through* it, and no gate compares the two routes on one place.
- 🚩 **A dump is 156 MB and the pictures are uncompressed.** Thirteen planes of a
  `1919x2077` viewport is twelve megabytes apiece, because `png.rs` writes stored
  deflate blocks on the argument that a debug dump is a file nobody keeps — which
  was true when a dump was one picture from a tool. A press of F12 is now
  thirteen, and a session's worth of presses fills a temp directory faster than
  anyone will think to empty it. `png.rs`'s own doc names the answer (the `png`
  crate as a dev-dependency) and rules it out for the library; the client is not
  the library, and a dump is exactly the caller that changes the arithmetic.
- 🚩 **The tool advances no animation clock at all.** The first thing
  `Inputs::summary` caught, on the first run: `isolated_scene` passes
  `StaticAnimations::default()` — nought cycles — where the client builds 1068
  out of `animdata.mul`. So every animated static in a tool's frame draws its
  base graphic while the client draws whatever the cycle is on, and a fire is the
  most likely thing anyone points either at. A field on `Inputs` already; what is
  missing is the tool building the table and a knob for the instant. P2's work,
  named here because it is the first *found* divergence rather than a suspected
  one.
- 🚩 **A summary cannot state the files it read.** `map` says the facet and the
  static count, the atlases say their sizes, and `tiledata` says only that it is
  the client's own table. Two frames off two different installs compare equal in
  every line. A digest of the loaded tables would close it, and until then the
  gate's answer is "equal given the same client files".
- 🚩 **Every GPU test binary keeps its own `gpu()` and `client_dir()`.**
  `tests/frame.rs`, `tests/dump.rs`, `tests/cost.rs` and the rest each carry the
  same adapter request and the same environment lookup, because integration test
  binaries share nothing without a `mod common`. Four copies of a device request
  that has to ask for `gbuffer::required_limits` is four places to forget it.
- 🚩 **A parity gate needs a place where the lighting is reachable.** At Britain's
  `(1501, 1659)`, a torch dropped in by `OPENSHARD_SCENE_EXTRA` at the client's
  own default brightness and reach changes the Lit plane **not by one byte** — it
  is shut inside a house and everything its pool would touch is under a roof. The
  seven-plane comparison P1 was checked with only became sensitive to the
  lighting at `_BRIGHTNESS=4 _REACH=3`. A gate laid on a frame with no flame that
  reaches anything is green about the geometry and blind about the light, and it
  would not say so. P3's three places have to be chosen for a lit pixel, not only
  for a house.
  <br>
  **The shard reader changes where to look for one.** A real place's flames are
  mostly the pack's: the street lamp at `(1507, 1658)` is a `decorations` row,
  and with the reader off the frame above has *no* light in it at all. So the
  three places are chosen out of the database — a lamp the shard placed is a lit
  pixel both ends can be asked about, while an `OPENSHARD_SCENE_EXTRA` torch is
  one only the tool has.
- 🚩 **The cost of a Britain-sized synthetic map is unmeasured** (D4). It is a
  `WorldMap::from_blocks` of roughly 200×210 blocks with a land lookup a cell —
  cheap in principle, unmeasured in fact.
- 🚩 **`examples/two_cubes.rs`, `synthetic_stair.rs` and `boxes.rs` build meshes
  by hand** — four hand-built diagnostic scenes, each its own copy, called out in
  `statics.rs`'s own note at `push_mesh`'s grave. They are not frame assemblies
  and may not belong in P2; decide when P2 reaches them.
- 🚩 **No gate holds that a debug view is drawn from the same planes the lit
  frame is.** `View::Solid` came out black in `cost.rs` for a whole session and
  nothing said so. A view that draws nothing on a frame that drew something is a
  finding, and it is cheap to assert.
