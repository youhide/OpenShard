# Every finding the client's construction turned up

Thirty backlog sections, each written where the work was, kept in the order they
were filed and moved here word for word. A finding with a defect behind it
also has a ranked row in [`../README.md`](../README.md); a finding with nothing
behind it is only here, which is why this file is read for a measurement and
never for a queue. Struck-through entries are the ones since closed, and the
prose that closes them is the account of how.

## Backlog it leaves behind, from `client/net` and the playground

- **The facet is read twice in one process**, once by the shard and once by the
  window, because `WorldMap` is loaded from a path by each end and neither knows
  the other is in the room. A few hundred megabytes, paid twice, and the same
  question the "a container is read whole into memory" item below is about. The
  honest fix is a `WorldMap` that can be handed over rather than opened again,
  and M3b's `Arc<WorldMap>` cache is where that belongs.
- **Nothing tests that the playground boots.** `tests/in_process.rs` covers the
  transport and `e2e/shard`'s others cover the wire; what the binary adds — a
  config with `client_files` set, and a window — is covered by running it. An
  `#[ignore]`d test that starts the in-process shard with a real install and
  enters the world *without* a window would cover everything but the GPU.
- ~~**The in-process shard has no way to stop.**~~ **Done.** A shard now stops
  on one word, and it is the same word wherever it comes from. See "Stopping"
  below.
- **A virtual player is now one type away.** `Dial` is the seam; what is missing
  is something that drives `client/net` without a window — the walk, the speech,
  and an oracle for what the world should have said. It belongs beside
  `crates/e2e/playground` rather than inside it.
- **It is the obvious place for M3b.** One process already holds a shard and a
  client, and `InProcess` clones: two sessions against one shard is one more
  dialler, and it would be a real test of "the files are loaded once per
  install".

## Backlog, found while planning the camera and the shell

- ~~**`ScreenPoint` is two spaces under one name.**~~ Split into `WorldPixel`
  and `ViewPixel`, neither with `From` or `Into`.
- ~~**Two writers move the camera.**~~ `eye` is private and `look_at` is the one
  door; `Follow` decides which writer may open it.
- **`depth::base_for` takes a tile and a free camera has none.** It now gets the
  eye's unprojected tile, and it takes `i32` because that tile may be off the
  map. `DEPTH_TILES` is 512 either side and a zoomed-out viewport spans a few
  hundred, so the margin holds — but the slack is now a function of the zoom
  rather than a constant, and the test that pins it still does not say so.
- ~~**`the_bounds_do_not_grow_without_limit` asserts a constant.**~~ Now
  `the_bounds_do_not_grow_faster_than_the_image`, re-run at every rung against a
  bound derived from the image's size. So is
  `every_tile_that_lands_on_screen_is_inside_the_bounds`, whose "did anything
  land on screen" floor had to become the image's area in tiles for the same
  reason: a constant there is either a failure at 4x or an assertion about
  nothing at 1/2x.
- **The world texture and its depth buffer are recreated on every zoom step and
  every resize.** Correct and not free — two allocations of a few megabytes on a
  wheel notch. Nothing has measured whether it matters; a pool keyed by size
  would be the answer if it does.
- ~~**Nothing in `client/app` is tested.**~~ The arithmetic moved:
  `crates/client/render/src/control.rs` is the camera, who may move it, and the
  fraction of a world pixel a drag has not yet spent, with thirteen tests and no
  device anywhere near them. The lock came with it — `follow_body` is the rule
  `App::step` and `App::entered` used to write out as an `if` each — and the
  device's refusal became a `TooLarge` value the caller prints, because a
  renderer with a stderr cannot be run twice in one process. Writing the tests
  found one defect: `fit_to_device` had no exit at the top of the ladder, so a
  viewport larger than `max_texture_dimension_2d` — which no zoom can answer —
  spun. What is still untested in `client/app` is the glue: which egui rect
  becomes the viewport, and when a redraw is asked for.
- **`Camera::pick` exists and nothing picks.** Screen to *ground tile* is
  `unproject(pick(cursor), z)` and is one line away; screen to *what you
  clicked* is the depth ordering read backwards, which is M5. Worth not
  designing around the easy half.
- ~~**Zoom-out makes the whole-atlas rebuild fire far more often.**~~ There is
  no whole-atlas rebuild on a miss any more — the atlases grow, and a zoom step
  is one band of new tiles like any other camera move. What survives of this
  entry is the half it was really about: four times the screen is four times the
  graphics against a 2048 atlas that is also the browser's guaranteed floor, so
  zooming out is still the fastest way to *fill* one. That now lands on the
  eviction rather than on the frame rate.
- **Picking is half-built the moment `to_world` exists.** Screen to *ground tile*
  is the inverse projection at a known `z`; screen to *what you clicked* is the
  depth ordering read backwards, and a static or a mobile in front of the ground
  is what M5 will actually want. Out of scope here, and worth not designing
  around a ground-only answer.
- **A free camera can lose the character entirely**, with nothing on screen
  saying which way to look. An edge marker, or a "return" button in the camera
  panel next to the lock, is the cheap answer; the lock key already exists so
  this is polish rather than a hole.

### Backlog, found while discussing decision 40's G-buffer (`lighting.md`)

- **Four near-identical CPU pickers want consolidating.** `statics::pick`,
  `items::pick`, `mobiles::pick` and `gump::pick` each re-derive the same
  placement math their own `collect`/`place` already computed, then test
  `opaque_at` and break ties the same way (topmost `depth::Order`, later-drawn
  wins). The rules are already written down once, correctly, above — what is
  not shared is the code: four call sites re-implement "walk the list this
  frame draws, replay its placement, hit-test the opaque texel, keep the
  topmost." Worth a common walker parameterised over the list and its
  placement function, once a fourth near-duplicate makes the pattern
  undeniable. Not started; this is a shape observation, not a design.

### What M6 still owes

- **`Sound.def` is not read.** It is the alias table the plan named — an id
  whose own archive slot is empty is redirected to another id's bytes — and
  this install's copy carries 437 live redirects (`654 {487} 0`: id 654 *is*
  487) beside 351 explicitly dead ones (`{-1}`). Without it a legitimate sound
  is reported as "absent from this install", which is a working id silently
  reading as a missing one. Same shape as the `.def` files `equipconv.rs`
  already reads.
- **The two effects packets still do not decode.** `0x6E` and `0xE2` do, and the
  client folds them onto the crowd (`link.rs`), but `0x70` and `0xC0` have no
  arm in `ServerPacket::decode` at all — so a bolt or a sparkle the shard throws
  is a packet this client drops. A spell is currently heard and not seen.
- **Distance is rodio's, not the reference's.** A `0x54` goes through
  `rodio::source::Spatial` in tile units: an inverse-distance law with no
  cutoff. ClassicUO (`Game/Managers/AudioManager.cs`) takes Chebyshev
  `max(|dx|, |dy|)`, attenuates linearly, and plays *nothing* past the view
  range. What hides the difference today is the shard's own broadcast range —
  a different number, in a different crate, deciding a client rule the plan said
  this client owns. Nothing tests it.
- **A track change blocks the caller.** `Player::clear` calls `sleep_until_end`,
  so swapping tracks parks the thread that reads the socket until the mixer has
  drained the previous source. Brief, and on the wrong thread.

## Backlog, found while pointing the readers at a real install

`crates/common/uofiles/tests/client_files.rs` is the suite that found these: the
readers had good tests, and every one of them was against a fixture the reader's
own understanding had written.

- ~~**Ter Mur loaded as Malas.**~~ Fixed. Malas is 320×256 blocks and Ter Mur is
  160×512, and both are **81,920** — so the block count, the only thing the file
  offers, names the wrong facet. Their `staidx` files are the same length too,
  which means the one consistency check that exists also passes. The result was a
  facet read at 256 blocks per column instead of 512: everything past the first
  column somewhere else, no error, no complaint. `facet_size` now takes the facet
  number `load_facet` already had. `WorldMap::load`, which has only a path,
  still cannot tell them apart, and its doc now says so.
- **`WorldMap::load` is public and called only by its own tests.** It is also
  the one entry point that cannot resolve the collision above. Either it grows a
  facet argument or it stops being `pub` — but that is a decision about who is
  supposed to call it, and nobody does yet.
- **The land table's record 0 is written in the pre-High-Seas shape.** Its name
  sits six bytes into a 30-byte record, so read at the modern offsets tile 0 has
  flags `0x4E55_0000_0000_0000` and the name `"ED"` — the tail of `"UNUSED"`.
  Every other record in the file is fine, so this is the file's quirk and not the
  stride. It is deliberately not special-cased: the junk lands entirely above bit
  32 and every flag movement reads is below it, so tile 0 cannot come out
  walkable, water, or a floor — which is what the test asserts instead.
- ~~**`hues` and `art` are missing.**~~ Written. `hues.mul` is 3,000 ramps of 32
  colours; `artLegacyMUL.uop` holds the land diamonds and the run-length encoded
  statics in one index space. `texmaps` followed, for the slopes. What is still
  missing: `unifont`, `light`, `sound`, `verdata`. `gumpart`, `anim`, `cliloc`
  and `multi` were written for M4 and M5; `radarcol` **is written now**, for the
  radar — see the section above, including the install that is thirty-six
  entries short of the canonical size. The first picture no longer needs any of
  them.
- **Gump art is deflated and nothing here can inflate it.** Every one of
  `gumpartLegacyMUL.uop`'s 5,556 entries has compression flag 3, where the map
  and art containers have none. `UopError::Compressed` says so rather than
  skipping. Whoever writes the gump reader — M4 — is the one who brings an
  inflater into the workspace, and it is worth deciding then whether that is a
  dependency or forty lines of stored-block DEFLATE.
- **`.mul` art is not read, only the UOP.** A modern install ships no
  `art.mul`/`artidx.mul` at all, so there is nothing here to test a `.mul` path
  against. The index is the same twelve-byte entry `staidx` uses, so it is an
  hour's work — but an hour spent writing something nobody can run, and the
  engine claims to support old clients, so this needs an old install rather than
  confidence.
- **A container is read whole into memory.** `Uop::open` holds all 155MB of the
  art and `TexMaps::open` another 45MB. Harmless for a shard, which never opens
  either, and not obviously right for a renderer holding several at once — the
  client app now reads 200MB of pictures before its first frame. The place to fix
  it is `Uop::open` and `TexMaps::from_files`, and the browser is the deadline:
  neither can call `std::fs` there anyway.

## Backlog, found while drawing the ground

- ~~**A sloped tile is textured with the stretched land sprite, not
  `texmaps.mul`.**~~ Read and drawn. `uofiles::texmaps` reads the
  `texidx.mul`/`texmaps.mul` pair — 4,116 squares of 64 or 128, and the *length*
  is what says which — and `TexmapAtlas` packs them on a 64-pixel cell grid,
  where a 128 takes a 2×2 block. Both atlases are keyed by the land graphic, so a
  quad asks them the same question.
- ~~**A `tiledata` flag may force a tile flat.**~~ Answered, and it is not a
  flag. `Land.ApplyStretch` stretches a tile only when it *has* a texture and is
  not wet; `IsStretched` is initialised to `TexID == 0 && IsWet` and then read as
  "do not". So "no texture" is the whole of the rule, and `WATER` is the only
  flag in it.
- **The client stretches a wider neighbourhood than we do.** `ApplyStretch` sets
  `IsStretched` from the four corner *normals*, each of which reads the tile
  beyond the corner — so a tile whose own four corners agree is still stretched,
  and therefore textured, when a neighbour differs. `ground.wgsl` decides from
  the four corners alone, so such a tile is drawn from its art here. The shapes
  agree exactly (a flat quad is the diamond), so this is a difference in
  *texture* along the edge of every slope, and closing it means computing the
  normals — which is also what lighting will need.
- **Nothing computes a normal, so nothing is lit.** The client shades stretched
  land from the four corner normals it already has. Ours is flat-lit, which is
  right for a first picture and stops being right beside a real client's
  screenshot.
- **`TexTerr.def` is not read.** The client remaps texture entries through it
  (`2500 {3} 1645` — entry 2500 is entry 3, hued), which matters for a land tile
  whose texture id lands in the aliased range and has no entry of its own. Such
  a tile falls back to its art here. The `.def` format is shared with several
  other files, so whoever needs the first of them writes the reader.
- **The stretched-art fallback has no half-texel inset.** Where a sloped tile has
  no texture it samples the land atlas at the diamond's own coordinates, and the
  bottom vertex lands exactly on `v + dv` — one texel into the sprite packed
  below it. One vertex of one triangle pair, so it is a hairline rather than a
  fringe, and the fix is the same inset `TexmapAtlas` already applies.
- **Void land tiles are drawn as black diamonds.** Under a building the map's
  ground is a "nothing here" graphic that the real client covers with statics.
  Until statics are drawn this leaves black holes in the picture, which is
  honest, but it is worth checking whether `tiledata`'s flags name these
  explicitly before deciding whether the renderer should skip them.
- ~~**The atlas is rebuilt whole whenever the camera walks off it.**~~ It grows
  instead: `LandAtlas::add` and its three neighbours pack what is new beside
  what is there, and only the rows that changed are uploaded. The eviction this
  entry asked for exists too, as the answer to an atlas that has filled up
  rather than as the answer to a miss.
- **`WorldMap` cannot be built in memory, so the renderer has no offline
  tests.** *(Planned: [`unenforced.md`](../../unenforced.md) S4.)*
  Every assertion about `ground::collect` lives in `tests/frame.rs` behind
  `OPENSHARD_CLIENT` and a GPU, because the only way to get a `WorldMap` is to
  load one from a file. A constructor taking cells — or a small fixture facet —
  would let the projection and the visible-set logic be tested with neither.
  Both atlases now take pictures directly (`LandAtlas::pack`,
  `TexmapAtlas::pack`), so the *art* half of that no longer needs an install:
  the test that a slope is drawn from its texture and a level tile from its art
  is green with no client at all. The map is what is left.
- **Nothing reads `Feature` or the client version yet.** The renderer draws what
  the files hold. That is right for ground, and it stops being right at the
  first packet the client draws from.
- **A pier or bridge's deck has no ground plane of its own.** `GroundQuad`
  (`ground.rs:17-24`) builds its four heights from the land layer only —
  documented as deliberate ("there is no single height to fold" from a single
  land tile) but a platform static (a pier, a bridge, stairs) is exactly a
  second surface at its own height, and `statics.rs` draws it as a sprite over
  the land quad without ever raising the quad or adding a plane at the static's
  own top. Combined with the walk backlog entry below (the avatar's predicted Z
  on such a tile also comes from the land layer, never the static), a walk onto
  a pier draws the avatar sinking into a ground plane that was never the deck to
  begin with — reported by a player 2026-08-02 as falling underground,
  specifically on piers and bridges. The two entries are one bug with two
  causes: the drawn floor is wrong here, the predicted Z is wrong in
  [`Walk::step`](#backlog-found-while-joining-the-window-to-the-wire) below.

## Backlog, found while drawing the statics and the mobiles

- ~~**Nothing is hued.**~~ Statics and mobiles are now, from a real
  `HueRamp` (`crates/client/render/src/hue.rs`) built once from `hues.mul` and
  bound alongside every sprite atlas. No second texture carries the palette
  index: the atlas already stores each texel's red channel widened to eight
  bits — `Color16::rgb8`, the same widening every other reader here uses — and
  `statics.wgsl` recovers the file's 5-bit index from it exactly with
  `round(r * 31.0)`, because that widening is a bijection on 0..=31. A full hue
  replaces the pixel outright; a partial one (the wire hue's own top bit) only
  where the sampled pixel is genuinely grey (`r == g == b`). Not done:
  `tiledata`'s own `PartialHue` flag, which forces the same grey-only rule on
  an item regardless of what the wire hue asks — nothing in `crate::atlas`
  carries a tiledata reference for a static's sprite yet, so this needs a
  second graphic to test against before it is worth wiring in.
- ~~**Nothing animates.**~~ Given a clock. `crates/client/render/src/animation.rs`'s
  `AnimationClock` advances by real time and picks a frame out of however many
  the atlas actually packed, looping over the animation's own length rather than
  a constant. What times it turned out not to be `animdata.mul` — that file
  times an *animated static* (`AnimatedStaticsManager`, a torch or a fire) and a
  spell *effect* graphic (`GameEffect`), each cycling a short run of consecutive
  graphic ids, and neither is a mobile's body. `Mobile.ProcessAnimation` reads
  `Constants.CHARACTER_ANIMATION_DELAY` instead: a fixed 80ms, unscaled unless a
  server explicitly set the animation with its own interval byte (`0x6E`/`0xE2`)
  — and that constant, not the file, is what `FRAME_DELAY` cites. `client/app`
  also did not draw the mobile pass at all until now: the atlas and pipeline
  existed and nothing called `mobile_pass.render`.
- **The clock assumes a packed animation has no gaps.** `AnimAtlas::build`
  keeps a frame's own index and only drops the entry when the frame is blank,
  so a blank frame in the *middle* of a real group — which the file format
  allows, see `AnimFrame`'s own docs — leaves a hole in the key space that
  `frame_count` does not report. `AnimationClock::frame` cycles `0..frame_count`
  assuming those are the packed indices, so a caller unlucky enough to hit such
  a body would have the mobile vanish for one tick rather than loop cleanly.
  Body 400 group 4 does not hit this, which is why the clock does not
  compensate for it; the fix is `AnimAtlas` exposing the actual packed indices
  rather than a count, whenever a body that needs it turns up.
- ~~**Equipment is not drawn.**~~ Worn items are, now. A worn item is not
  drawn from its own art — its *default* picture is its own tiledata entry's
  `AnimID` field (`StaticTile::anim_id`, `crates/common/uofiles/src/tiledata.rs`
  — present in the file and in this reader's own layout comment the whole
  time, but never actually read until now), the same index space and the same
  `anim.mul` machinery the body itself draws through. `Equipconv.def` only
  *overrides* that default, for the `(body, AnimID)` pairs where this body
  needs a different picture — chiefly a race or gender variant of the same
  garment — which is why an ordinary shirt has no entry in it at all. Getting
  this backwards was the first cut at this feature: treating "no entry" as
  "draw nothing" instead of "the default already draws right" silently
  dropped every piece of plain clothing on every NPC, caught only by looking
  at a live client rather than by any test, because every test here packs its
  own atlas and never has an item *without* a conversion entry to notice the
  gap. `openshard_uofiles::equipconv::EquipConv` reads `Equipconv.def` — text,
  not one of this crate's binary formats, the same shape as `Body.def` — and
  `mobiles::collect` pushes one extra `SpriteQuad` per layer at the *same*
  `depth::Order` as the body, so the existing stable sort draws it on top with
  no second depth pass. Only a layer whose resolved `AnimID` the atlas has no
  frame for this frame is dropped, the same rule a missing body animation
  gets.

  **Mounts and corpses are still not drawn** — a mount replaces or extends the
  body draw rather than layering over it, and neither was touched here.
  Left out of the same pass, deliberately:
  - **`mobtypes.txt` and the gargoyle graphic offset.** Nothing here tells a
    gargoyle body from a human one, so a gargoyle's equipment resolves through
    the same table a human's does and comes out wrong or absent.
  - **Paperdoll layer ordering.** `PaperdollOrder`'s T1/T2/T3 tables and
    point-fix rules (cloak-by-facing, helmet-over-hair) have no counterpart
    here — `Layer` is a bare `u8` on purpose (see its own doc comment), so
    layers draw in whatever order the server listed them in `0x78`, which is
    usually close to right and not guaranteed to be.
    *Built since, in M4 — see decision 3 there: `paperdoll::order` for the
    tables and `paperdoll::world_order` for the cloak the facing moves. "Usually
    close to right" was wrong: a cloak listed after a tunic is drawn over the
    front of it on a character facing the camera.*
  - **Incremental equip/unequip.** `Equipment` only ever changes as a
    full-list replacement inside a fresh `0x78`; there is still no `0x2E` or
    per-item wear/drop decoder, so a shard that only updates one worn slot
    rather than resending the whole list will not be reflected here.
- **`anim2` through `anim5` are openable and not addressable.** `Anim::from_files`
  takes a pair, but the index arithmetic implemented is the first file's, and the
  others re-base the three body kinds differently. Left undone rather than
  guessed: a wrong base reads a real creature's frames.
- **The UOP animations are not read at all.** A modern install ships both
  `anim.mul` and `AnimationFrame1.uop`, and the client prefers the UOP where a
  body exists in it. Everything a human needs is in the `.mul` on 7.0.116.0, so
  this is not blocking — but a body added after the `.mul` stopped being updated
  would be missing here and present in ClassicUO, which is a confusing way to
  find out.
- **A mobile's own `z` is the caller's problem, and getting it wrong looks like
  a bug in the renderer.** A body at the camera's height rather than the
  ground's is *correctly* hidden by the terrain in front of it, which is
  indistinguishable from a mobile that failed to draw — it cost a debugging pass
  here. Whatever eventually feeds `WorldView` into this will get the height from
  the server and be fine; `client/app` reads it from the map.

  *And read it from the wrong place, which is the second half of the same
  entry.* A land cell stores **one** height and it is the diamond's northern
  vertex, not the height of the tile: a body stands at the average of the four
  corners (`WorldMap::average_land_z`, RunUO's `GetAverageZ`, ClassicUO's
  `Land.AverageZ`). `link.rs` predicted each step's `z` from the raw corner while
  `MapTerrain::ground_z` on the server had always used the average — the two ends
  each land their own step because a `0x22` carries no `z`, so the disagreement
  was silent and permanent, up to the whole relief of a tile. On a slope that
  draws the character sunk into the hillside *and* behind it: the ground's own
  depth is that same average less two, so once the body's `z + 1` falls under it
  the land is not merely near the walker, it is in front of him. One formula now,
  on the map where both sides can reach it, with the corner walk beside it.
- **A body between two tiles sorts at the nearer of them, and taking the
  destination is wrong for half the compass.** `View.CalculateDepthZ` hides the
  rule behind eight arms of a `switch` over the sprite's drift, because the
  reference keeps `Mobile.X/Y` on the tile a step *started* from until the step
  lands. Read whole, the eight arms are `max(from, to)` — plus one where the two
  are equal, which is the north-east/south-west pair that moves straight across
  the screen. Sorting at the destination unconditionally is that maximum only for
  the four headings that go *down* the screen; for north, west and north-west the
  destination is the farther tile, so the ground being stepped off — and every
  wall standing on it — was drawn over the walker for the length of the step.
  `depth::mobile_tile` is the rule, `Mobile::from` is what it needs, and
  `Crowd::stepping_from` is where a step's clock decides it: tied to the *glide*
  and not to `Tracked::step`, which outlives the crossing by the half step the
  animation is deliberately held for.
- **`CalculateDepthZ`'s `z += max(0, Offset.Z)` is not ported.** The reference
  raises a body's priority by its mid-step *height* offset, on the east and south
  arms only. Two of eight is the kind of asymmetry that is either load-bearing or
  a leftover, and nothing here can tell which without stairs to walk up; the tile
  half of the ordering is ported and this is not. Revisit when a body first walks
  a staircase facing east.
- **A tile with no texmap is stretched here and flat in the reference, and that
  now reaches the *ordering* too.** `ground.wgsl` already records the divergence
  in what is drawn. What is new is that `Land.ApplyStretch` gives such a tile
  `AverageZ = MinZ = z` — the raw corner — so ClassicUO both draws it flat and
  sorts it at the corner, where we draw it stretched and sort it at the average.
  Ours agrees with the *server*, which is the agreement that matters for where a
  body stands; but a client-side height that disagreed with the reference client
  on the same map is worth knowing about before the flat/stretched decision is
  ever revisited.
- ~~**Two sprite passes mean two atlases and two pipelines... if a third
  sprite layer arrives, does it need a third?**~~ It arrived, and it did not:
  an equipment layer's resolved body-anim graphic reads through `Anim::frames`
  exactly the way a body does, so it packs into the *same* `AnimAtlas` under
  its own `FrameKey.body` — no tag needed, because two different resolved
  graphics are already two different keys, and two equipment layers that
  happen to resolve to the *same* graphic correctly share one packed frame,
  the way two mobiles of the same body already do. Still two passes: the third
  layer turned out to be more entries in the mobile atlas, not a third pass.
- ~~**The labels still are not drawn.**~~ They are now, above whoever the
  crowd last heard from. `crates/common/uofiles::font::AsciiFonts` reads
  `fonts.mul`; `crate::atlas::FontAtlas` packs every glyph it defines — all
  ten faces, unconditionally, because unlike a graphic there is no "not
  currently visible" character — into a fixed grid cell-sized to the tallest
  one packed, the way the backlog entry below guessed. `crate::text::collect`
  turns a line into glyph quads, left to right by each glyph's own width
  (`fonts.mul` carries no kerning table) and centred on an anchor rather than
  left-aligned to it, drawn through the same `SpriteRenderer` and `statics.wgsl`
  as everything else: font pixels are grey and a wire hue replaces them exactly
  the way a partial hue replaces a static's, so no shader of its own was
  needed. `crates/client/app/src/crowd.rs` gained the other half — `Crowd::hear`
  and `Crowd::speaking`, a serial's last line and when it arrived — and
  `App::entered` calls `hear` once per new journal tail line, matched to a
  drawn mobile through `mobiles::head_anchor`, the top-centre of exactly the
  rectangle `mobiles::collect` would draw for it (refactored out from under
  both so the two cannot disagree about where a body's head is).

  Three corners cut, each because the wire does not yet carry what a correct
  answer needs:
  - **`fonts.mul` is still unconfirmed against a shipped file.** No client
    install was on hand while writing either the reader or the atlas above
    it, so — alone among `uofiles`'s readers — it has no counterpart in
    `tests/client_files.rs`, and the byte layout is the widely-documented
    community format rather than a fact this crate has confirmed on real
    bytes. First thing to do with a client tree in hand.
  - **The hold is a flat five seconds, not the message's own length.** The
    reference client's `Mobile.m_SpeechTime` grows with the text; nothing
    that reaches here — not the wire's `0x1C`, not `SpokenMessage` — carries
    an expiry, so `crowd::SPEECH_HOLD` is a constant standing in for one.
    Threading a real duration through means deciding whether it is computed
    at the packet boundary or carried on the wire, which is `client/net`'s
    question and not this pass's.
  - **A label is always drawn nearest, not depth-tested against the world.**
    `Label::depth` is wired but `client/app` always hands it `0.0`: there is
    no `depth::text_priority_z` the way there is a `mobile_priority_z`, so a
    line is never hidden by a wall in front of its speaker. Reads right for
    every case tried so far — overhead text is an overlay in the reference
    client too — and is the thing to revisit first if that stops being true.
  - **Only the newest journal line is ever heard.** `App::entered` compares
    `view.journal.back()` against the previous view's, so two speakers
    between one `WorldView` update and the next share one slot and the
    older one's line never appears, however briefly. The journal itself
    drops nothing — this is a gap in what gets a bubble, not in what gets
    remembered.

  `unifont.mul` — the Unicode faces gump text and gump title bars use — is a
  separate reader again and is not started; `fonts.mul` was picked first
  because it is what `speech::Font` already names.

- **`fonts.mul`'s faces do not survive a non-integer scale, and nothing here
  picks an integer one.** The classic faces are what the game is supposed to
  look like, and they are ~6×10 one-bit glyphs: at 1× on a 1440p screen they
  are unreadably small, and at any fractional scale the filter turns them to
  mush. Three ways out, in the order they should be tried, because each is
  more work than the one before and the cheap one may be enough:

  1. **Render the atlas at an integer multiple of 1×** and pick the multiple
     from the window's DPI. This is what every pixel-art renderer does and the
     only option that keeps the face *itself* — no resampling, no new asset,
     no derivative of a client file. It should be tried and looked at before
     anything below is planned.
  2. **Depixelize to outlines, then MSDF.** Kopf & Lischinski's
     *Depixelizing Pixel Art* (SIGGRAPH 2011) — the algorithm behind
     Inkscape's "Trace Bitmap → Pixel art" — resolves 8-neighbourhood
     connectivity and emits real curves rather than tracing the staircase, and
     `msdfgen` over those curves gives one atlas that stays sharp at any
     scale. **A generative upscaler is the wrong tool here**: at ~60 pixels of
     input there is nothing to reconstruct, and a per-glyph model has no idea
     the glyphs form one typeface — weights and x-heights drift apart between
     letters, which is the first thing anyone notices.
  3. **Redraw the face by hand as a real TrueType** and load it through
     [`ttf_font.rs`](../../../crates/common/uofiles/src/ttf_font.rs), which already
     reads a face from a path. Best quality — an autotracer cannot make the
     calls that 60 pixels do not contain, such as which bump is a serif and
     which is a rasterisation artefact — and it collapses two render paths
     into one, since the Unicode face already goes through that reader.

     Two hand-redrawn candidates, at opposite ends of what may be committed:
     **CLF-UoClassic** (uo.clife.work, v1.1, SIL OFL 1.1 — free and
     redistributable, sitting unwired at `assets/fonts/CLF-UoClassic/` with its
     own `README.txt`/`LICENSE.txt`) and
     [An Corp](https://blazetype.eu/typefaces/an-corp/) (Blaze Type, 2025 — a
     from-scratch revival of the same client face, "low contrast, orthogonal
     terminations, and slab serifs," but a paid proprietary EULA with no free
     distribution, so it is a reference to look at, not a file to fetch and
     bundle).

  **What may and may not be committed.** (2) produces a mechanical derivative
  of a copyrighted client file, so it is a build tool the player runs against
  their own install, writing to a cache beside `OPENSHARD_CLIENT` — the same
  rule as every other `.mul`, and the same shape as `ttf_font.rs` reading a
  path instead of embedding bytes. (3) is a different question: in the US a
  *typeface* is not copyrightable (*Eltra v. Ringer*) and the Copyright Office
  holds bitmap fonts uncopyrightable for the same reason, while a scalable
  font *program* is protected as software — so a hand-drawn face in the same
  letterforms is a much stronger position than a traced one, and is the only
  one of the three whose output could plausibly ship in this repository. Other
  jurisdictions do not all follow the US here (Germany protects typeface
  designs outright), so that is a question for someone qualified before
  anything is licensed, not a decision this file makes.

- **A repack that fails is a `eprintln!` and a frame that draws anyway.**
  *Half answered — the atlases now evict, so "full" is recoverable.* When a
  growth returns `AtlasError::Full`, `App::draw` packs an atlas for what is on
  screen *now* and throws away everything the camera has walked past. That is
  the eviction policy this entry asked for, and it was not optional: an atlas
  that only ever grows has no other way to reclaim a graphic, where the old
  rebuild-on-every-miss did it by accident. Measured against a real install, the
  start tile (1495, 1629 on Felucca) sees 187 distinct static graphics and 136
  tiles out it is 588, so the fill is reachable by walking rather than a corner
  case. What is *still* an `eprintln!` and a frame that renders is the case
  underneath it — one screen's statics not fitting one 2048 atlas, which no
  eviction can help. That wants a texture array or several atlases, and it is a
  different statement from "the atlas filled up".
- ~~**`stale` cannot become false when a visible static has no art.**~~ Fixed,
  and the fix is that the question is no longer asked. Every atlas records what
  it was *offered*, not only what it packed (`StaticAtlas`'s `asked`), so a
  graphic the client ships no art for is read once and skipped for ever after —
  where before, one such tile on screen repacked every atlas on every frame,
  because a graphic that cannot be packed is never packed. The staleness check
  it fed is gone too: what a frame needs that the atlas has not seen is now a
  question about the tiles the camera crossed, not about the graphics on screen.
  See the entry on the four walks below.

## Backlog, found while joining the window to the wire

- ~~**Everything stands.**~~ ~~**One animation clock for everybody.**~~ Both
  were the same missing thing, and it is `crates/client/app/src/crowd.rs`: a
  position, a group and a clock per serial, above the view and below the
  renderer. It lives in the app because it reads `client/net` and writes
  `client/render`, which may not depend on each other.

  What that turned up is worth more than the animation: **the three body kinds
  number their groups differently, and group 4 is not "standing".** It is
  `PeopleAnimationGroup.Stand` for a player, `HighAnimationGroup.Attack1` for a
  monster and one past `LowAnimationGroup.Eat` for a horse — all three exist, so
  the old constant failed at nothing and drew the wrong action forever.
  `BodyKind::standing`/`walking`/`running` answer it now, pinned in a test
  beside the enums; a monster's `running` is `None`, because `High` has no run.

  Nothing on the wire says "stopped walking", so a step is heard rather than
  seen: a position change starts a walk and `WALK_HOLD` — `WALK_INTERVAL`
  twice, from `common/movement` rather than chosen to look right — ends it. A
  *turn* is not a step, which is what a layer watching the facing instead of the
  position would get wrong while passing every other test.

- ~~**A step is a teleport of one tile.**~~ Glided. `Mobile` carries a `Glide` —
  the tile stepped off and how far along the body is — and
  `mobiles::world_position` hangs the sprite between the two projections.
  Everything else still reads `Mobile::at`: the depth order is the *destination*
  tile, or a body would change sides of a wall halfway through a step. Three
  things it turned up, each of which the glide alone would have looked wrong
  without:

  - **The eye has to glide too.** `Control::follow_body` took a tile, so a
    character sliding smoothly across the world had the whole world jumping a
    tile at a time underneath it — worse than the teleport, because it is the
    *ground* that jerks. It takes a `WorldPixel` now, and there is still one door
    to the eye.
  - **The hold is also the step's length, so it is not one number.** A runner
    steps every 200ms — ServUO's `RunFoot`, `RUN_INTERVAL` doubled the way
    `WALK_HOLD` doubles `WALK_INTERVAL` — and glided over a walk's 400ms it would
    be half a tile behind itself and jump forward at every step. `RUN_HOLD` is
    the wire's own running flag applied to both.
  - **The window redrew on the animation clock**, 80ms, which is right when the
    only thing that changes is a frame index and gives a glide five visible jumps
    instead of a slide. `App::redraw_interval` has two rates and
    `Crowd::anyone_gliding` picks between them; the crowd is advanced by
    *measured* time now rather than by the interval that was waited for, since
    `WaitUntil` is a floor and a stepping animation hides the overshoot where a
    glide does not.

  A move of more than one tile is not glided at all: a gate, a recall or a `0x22`
  putting a mispredicted body back would otherwise slide the character across
  half a facet over 400ms, which is a stranger picture than the teleport it hides.
- ~~**Nothing ever asks to run.**~~ Shift does. Everything downstream of the
  wire's running bit was already built — the server's `WalkPace` charges
  `RUN_INTERVAL`, `Crowd` holds and glides a runner for `RUN_HOLD`, and
  `BodyKind::running` picks the group — and the only thing missing was a client
  that set the bit. What writing it turned up is that **the pace is input, not
  output**: a step used to be sent from the key event, which made the operating
  system's auto-repeat the walking speed — half a second of nothing, then thirty
  steps a second, and the fast half is exactly what `WalkPace` refuses as a
  speedhack, so the shard answers `0x21` and the body is pulled back. That reads
  as the walk stuttering rather than as the client asking for too much, which is
  the wrong bug to go looking for.

  So a direction is *held* rather than pressed, and the clock is ours:
  `crates/client/app/src/keys.rs` says which way the keyboard is pointing and
  `steer.rs` sends one step every `WALK_HOLD`, or every `RUN_HOLD` with shift
  down. The rate is the hold and not `common/movement`'s interval on purpose —
  those are anti-speedhack *floors*, deliberately half the real rate, and walking
  at the floor would move a body twice as fast as the crowd glides it. Two
  releases that never arrive have to be caught for a held key not to walk for
  ever: the window losing focus, and egui taking the event.

- ~~**The mouse cannot walk.**~~ A right click is a move order: the body walks to
  that tile on its own, and holding the button steers it to wherever the cursor
  is — the strategy game's idiom and the 2D client's right-hold, which turn out
  to be the same feature stated twice. Left stays the Tile panel's and the middle
  button still pans.

  What writing it decided: **the pace belongs to neither input.** A second timer
  beside `keys.rs`'s would take two steps a beat the moment somebody nudged an
  arrow while walking to a destination, so `steer.rs` owns *one* clock and the
  two inputs are only sources of a direction — the keyboard winning, and a press
  dropping the destination rather than queuing behind it. `keys.rs` is now the
  arrow stack and nothing else.

  ~~The route is greedy — the straight-line direction, a step at a time —
  because this end has no walkability to plan over.~~ It plans now — see the
  next entry.

- ~~**A click-to-walk destination does not route around anything.**~~ Planned.
  `Steering::go_to`/`due` take a `&dyn Terrain` and run `common/movement::find_path`
  on the click, then walk the returned route one direction per step instead of
  `direction_toward(from, goal)` (`client/app/src/steer.rs`). The three decisions
  the entry named, and how each landed:

  - **Where the check lives.** Lifted: `MapTerrain` and its `check` moved from
    `server/world/src/terrain.rs` into `common/movement/src/terrain.rs`, beside
    `find_path`, generic over `M: AsRef<WorldMap>, T: AsRef<TileData>` so the
    server keeps building one owned at boot (`MapTerrain::new(map, tiles)`) and
    the client builds one borrowing (`MapTerrain::new(self.map.as_ref(),
    &self.tiledata)`) fresh per click rather than cloning the facet. `world`'s
    own `terrain.rs` is now a two-line re-export plus the one test
    (`the_layer_byte_reads_the_hand_a_weapon_is_held_in`) that needs
    `openshard-state`'s layer constants, which `common/movement` may not depend
    on.
  - **A client cannot see the dynamic half.** Unchanged, and still the reason a
    plan is a guess: `Obstructions`/`LiveTerrain` (`server/state/src/obstruct.rs`)
    stay server-side. The `0x21` stall detector (`STUCK_STEPS`) is still the
    correction for what the plan could not know.
  - **Replanning cadence.** Plan on the click (`Steering::go_to`); on a step that
    left the body exactly where it was — a refusal — `Steering::take` replans from
    the body's real position before trying again, rather than repeating the same
    refused step until `STUCK_STEPS` gives up.

  Two things the fix turned up that were not in the plan:

  - `find_path`'s A* is tied on Chebyshev cost between a straight cardinal line
    and any equal-length route that drifts diagonally off it and back, so an
    axis-aligned click could come back zig-zagging. `common/movement::path`'s
    open list now breaks ties by Manhattan distance to the goal, which is
    smaller for the route that stayed straight — a tie-break, not a second
    heuristic, so it does not change *whether* a shortest route is found, only
    which equally-short one A* settles on. This also straightens whatever else
    calls `find_path` (an NPC's chase), not only the click.
  - **The first cut of this ran `find_path` on every mouse-move, not every
    step, and froze on some routes.** `go_to` is called on the click *and*
    again on every raw `CursorMoved` event while the button stays down —
    `client/app/src/lib.rs`'s `walk_to_cursor` — which is tens of events a
    second while dragging, not one. Planning eagerly there meant an A* search
    that many times a second, and a destination expensive to search (out of
    reach, so every search burns the whole node budget) froze the window for
    as long as dragging lasted. `go_to` now only ever restates *where* — it
    drops the stale route and leaves the new destination unplanned — and
    `Steering::take` is the sole place a search runs, gated by the same step
    cadence as a step itself, so a plan costs at most once per
    `WALK_HOLD`/`RUN_HOLD` no matter how fast the cursor moves.
    `restating_a_destination_mid_step_does_not_search_the_terrain`
    (`client/app/src/steer.rs`) pins it with a terrain that counts its own
    calls. `PLAN_BUDGET` also came down from a first guess of 4,000 to 600,
    in line with `common/movement`'s own "a few hundred is ample for a town" —
    the eager version's cost per search was the bigger problem, but an
    unreachable destination still pays the full budget once a step, `STUCK_STEPS`
    times over, and a smaller cap bounds that too.
  - **The default right-hold was still `go_to` — a destination, not a
    heading — which is the wrong idiom for "run toward the cursor" and is its
    own entry next: a body doing nothing but chase the cursor around a room
    would occasionally lurch at a pillar directly under it, refuse, snap back,
    and restart the walk animation for it.

- **The mouse's held-right-button idiom was one input made to do two jobs, and
  the seam showed as a lurch into a pillar.** The click-to-walk fix above made
  a destination's refusal-and-fallback behaviour reasoned about and tested,
  but a player dragging the mouse to say "run this way" was still, underneath,
  issuing a stream of *destination* orders — one per cursor tile — and a destination
  that cannot be reached degrades to walking at it anyway, refusal by refusal,
  which is exactly the honest behaviour a real move order wants and exactly
  the wrong one for a heading a player is only pointing, not aiming.
  `client/app/src/steer.rs` now answers two different questions from the
  mouse, matching what was already true of the keyboard versus a click:

  - **`Steering::steer(direction)`** — the default right-hold, no modifier.
    Not an order to reach a tile: a compass heading from the body to the
    cursor, recomputed every move and driven exactly like a held arrow key.
    It has no notion of arrival or of being stuck, and it never touches
    `find_path` or the map — but a blocked direction is no longer walked into
    forever either:
    [`movement::Detour`](../../../crates/common/movement/src/detour.rs) answers with
    the nearest way still legal past it (an O(1) look at four tiles, not a
    search), so a runner slides past an obstacle instead of standing against
    it. What "legal" means is not symmetric: a wall dead ahead of a held
    *cardinal* has no diagonal past it at all — the server's own corner rule
    (`LiveTerrain::can_step`, and `movement::step_allowed`) requires both
    cardinal tiles flanking a diagonal step to be open, and the blocked
    direction is unconditionally one of those two flanks for either diagonal
    beside it, so neither ever passes; `detour` offers the cardinal along the
    wall's face instead, the same sidestep a body hugging a wall makes. A
    blocked *diagonal*, pinned by a corner rather than a wall, has the
    opposite shape: the two cardinals it splits into have no corner to cut,
    so those are what is tried. Offering the wrong one of the two — a
    diagonal past a wall, which an earlier version of this did — is not a
    cosmetic bug: the body is drawn slipping through the wall's corner for a
    round trip and rubber-banded back, worse than the stand-and-bump this
    replaced, on every retry for as long as the direction is held.

    And the corner-stuck report that outlived all of the above was the *other*
    half of that rule going missing: **`Terrain::can_step` answers for the
    destination tile alone, and the corner rule is not part of it.** The
    terrain the client plans against is `MapTerrain`, the static map, which
    has no such check — so a diagonal clipping the corner where a wall ends
    reads as perfectly open here, `detour` saw nothing to detour from, and the
    client asked for the same diagonal every hold while `LiveTerrain` — which
    *does* apply the rule, because it is the last word before a `0x21` —
    refused every one of them. A body pressed against a building corner
    therefore never tried the one thing that gets past it: a single step to
    the side. The fix is that the rule stops being something each caller
    remembers: `movement::step_allowed(terrain, from, direction)` is the whole
    question — tile steppable *and* no corner cut — and `find_path` and
    `steer`'s `open` both ask it. `LiveTerrain::can_step` still restates it
    inline, because it is inside the `can_step` `step_allowed` calls; that one
    is the intended duplicate, and the comment there says so. Pinned by
    `a_diagonal_that_cuts_a_wall_corner_sidesteps_instead_of_asking_for_it`,
    whose scene is the one the earlier detour tests could not see: the
    diagonal tile itself is open ground, and only the corner refuses it.

    The corner has a second half, and it is what "a heading never gives up"
    got wrong: **not giving up on the asking is not the same as sending a
    packet.** Wedged in the inside corner of a building and leaning on the key
    — the direction blocked, both flanks blocked, nothing for `detour` to
    answer with — the client used to ask for the blocked direction anyway,
    every hold. Every one of those is a step this end has already proven the
    shard refuses, and the answer is a `0x21`: the body snapped back and the
    walk sequence reset, a hold at a time, which reads as a character
    shuddering against the corner instead of standing in it. `detour` returns
    `Option` now and `take` sends nothing when it is `None` — except the
    *turn*, which the shard accepts (a mobile asked for a direction it is not
    facing turns and moves nowhere) and which is the feedback a player
    pressing into a wall expects. The clock is armed either way, and that is
    load-bearing: nothing here clears the asking, so an unarmed clock would
    have the wait loop wake on a deadline already passed and re-ask
    immediately, forever. Armed, the attempt repeats at the walking pace and
    the walk resumes on its own the moment the door opens
    (`a_heading_into_a_corner_turns_once_and_then_sends_nothing`,
    `a_heading_held_in_a_corner_walks_the_instant_the_way_opens`).

    Both halves then stopped being a special case buried in the input handling
    and became a rule with a name: `common/movement`'s
    [`detour`](../../../crates/common/movement/src/detour.rs) — a scene (`Around`),
    an intent, a three-state machine (`Detour::Clear` / `Sliding` / `Standing`)
    and an answer (`Step::Ahead` / `Aside` / `Stuck`). The third state is the
    one the first cut of this got wrong: it answered `Stuck` and then set
    itself back to `Clear`, which says *nothing was in the way* about a body
    wedged in the corner of a building. **Not moving is one of the things a
    body does**, it persists for as long as the player leans on the key, and a
    machine that throws it away can only answer "is this walk getting
    anywhere" by re-deriving the scene it just read.
    `the_state_left_behind_says_which_of_the_three_the_body_is_doing` pins the
    state to the answer at every scene, from every state.

    How far a body may be *turned* off the ask is the one thing here that is a
    preference rather than a rule, and the sizes are not a spectrum: the flanks
    are fixed, so there are exactly two. `Leeway::Eighth` — the default — is a
    45° turn, which is what a blocked diagonal splits onto: **a body rounding a
    corner, always allowed**, because refusing it is a character stopping dead
    at the edge of a house it was walking past. That was the first cut of this
    and it stopped far too hard. `Leeway::Quarter` adds the 90° turn, the only
    thing a blocked *cardinal* has, which puts the body travelling at right
    angles to what was asked — defensible, a surprise, and so the thing a
    player opts into. Walking straight into a wall therefore stops the body by
    default, which is what the classic client does.

    It is a parameter to `Detour::step` and not a field on the machine, because
    a state and a setting must stay two values — and being read where the
    decision is made is what lets it change mid-walk with no state to reset.
    There is no client config to read it from yet; `Steering::set_leeway` is
    the single line one will set, written out at `App`'s construction today.
    `a_heading_stops_at_an_obstacle_by_default_and_slides_only_when_asked` is
    what pins the default — deliberately the one test that sets nothing, since
    a default flipping by accident is every walk in the game changing character
    with nothing to catch it — and `the_leeways_differ_only_at_a_wall_dead_ahead`
    holds the two settings to differing in exactly one place, which is easy to
    widen by accident and hard to notice.

    ### The cursor says more than one of eight things

    With both ways round a corner open and nothing in the terrain to prefer
    either, the tie used to fall to the flank last taken and then to a fixed
    rotation. But the player has *already said* which way round they mean to
    go: the cursor sits a little to one side of the corner, and rounding to one
    of eight sectors threw that away before anything could read it.
    `movement::Lean` is that sub-sector detail put back — `Clockwise`,
    `Centred`, `Counter`, from the sign of a cross product, integer arithmetic
    so that "squarely on the bearing" is exact rather than a tolerance in
    degrees. `Heading` is a direction and its lean together, and it is what
    `Steering::steer` takes now. The tie-breaks run lean, then the remembered
    flank, then clockwise: the freshest and most specific thing wins, and the
    loop-breaking memory still covers the case where nothing was said (a held
    arrow key, a cursor squarely on the diagonal).

    **And the lean is measured on the screen, from where the body is drawn.**
    Not on the tile grid: a player pushes the mouse away from their character
    in the direction they want it to go, and that direction is a bearing on a
    flat picture. That the two agree for the projection drawn today is a
    coincidence of its numbers — `camera::project` is a rotation and a uniform
    scale, and rounding to a sector survives that — not a property of the idea;
    a 2:1 diamond, which is what most isometric art is, and the grid reading
    starts naming a direction the cursor is nowhere near. The origin is the
    body's own projected pixel rather than the middle of the viewport, which is
    what makes it survive **a camera that is not locked to the body**: with a
    free eye the character is off-centre, and "away from the middle of the
    screen" is then a different question from "away from the character". Both
    are defensible idioms and a shard may one day want the other; this is the
    one that keeps meaning what it means while the eye wanders.
    `App::heading_between` is the arithmetic, split out of the method so it can
    be checked against the drawn picture rather than against a running window,
    and `the_screen_bearings_are_the_grid_turned_an_eighth` pins the thing that
    catches a grid reading by mistake: straight down the screen is
    *south-east*, and the grid would have called it south.

    Its whole input is **four tiles**:
    where you stand, where you meant to go, and the two flanks that could take
    its place. Not eight, and that is the argument rather than a
    simplification — which two flanks are candidates is fixed by the intent
    (ninety degrees off a blocked cardinal, forty-five off a blocked diagonal),
    and no other neighbour can change the answer. That is what makes the rule
    *enumerable*: `Around::new` states a scene outright, so
    `no_scene_at_any_intent_is_ever_answered_with_a_shut_direction` runs all
    8 directions x 8 open/shut combinations x 3 machine states and checks the
    claim that matters on the wire, instead of drawing a wall and hoping the
    interesting case was the one drawn. `a_scene_read_from_the_world_is_a_scene_stated_outright`
    is what keeps `Around::read` and `Around::new` one rule, or the enumeration
    would be proving something about a fiction. `steer.rs` keeps only what is
    genuinely about input: that a *held* direction is answered this way and a
    planned route is not, that the first ask gets it too, and what `Stuck`
    means to something that has a facing and a clock. This
    applies from the very first ask, not just the steps `Steering::due`
    answers afterward: `Steering::steer`/`press` now take a `terrain` too
    (constructed on demand at their call sites in `App`, the same
    `MapTerrain` `due`/`go_to` already built) and route through the same
    `Steering::take` `due` does, rather than answering directly. That first
    ask is not the rare case for the mouse heading in particular — a player
    working a corner is actively moving the cursor, and every sector change
    is a fresh `steer()` call; answering those without the detour meant a
    player *trying* to route around a corner hit the undetoured path on
    almost every attempt, and only the occasional still-held, re-asked-at-
    the-next-hold heading ever saw the fix. Released, it stops at once;
    unlike a destination, there is nothing behind a heading once nobody is
    pointing it any more.
  - **`Steering::go_to(tile)`** — unchanged in mechanism, now reached only by
    holding Ctrl. The real move order: `find_path` plans a route, a refusal
    replans, and — the one behavioural change here — a destination `find_path`
    proves has no route at all no longer gives up outright. It falls back to
    the same straight-line heading `steer` would use, still under the
    `STUCK_STEPS` patience, because Ctrl+drag is an explicit "go to this exact
    spot" and walking up to an obstacle and stopping (classic UO's own answer
    to clicking on a wall) is the right honest answer for *that* idiom — the
    lurch was never the fallback itself, it was the fallback firing for input
    that never meant to ask a pathfinding question in the first place.

    That fallback is **gone** — see the entry below. It was the right *intent*
    said the wrong way: walking up to an obstacle and stopping is what a move
    order is owed, and a straight line at the obstacle is not that. It walked
    into whatever was between the body and the goal, one refused step a hold,
    for as long as the patience lasted; the way to walk up to something is to
    plan a route that stops there, which is now `find_path_toward`'s job.

  `keys` still outranks both, and `go_to`/`steer` clear each other, so exactly
  one of "arrows", "heading" or "destination" drives a step at a time — see
  `Steering::asking`. `a_heading_held_in_a_corner_walks_the_instant_the_way_opens`,
  `releasing_the_mouse_stops_the_heading_but_not_the_keyboard`,
  `the_keyboard_takes_over_from_a_heading` and
  `a_destination_with_no_route_falls_back_to_a_heading_then_gives_up`
  (`client/app/src/steer.rs`) are what pin the split.

- **A move order that shows its work, and stops at the door rather than at
  nothing.** Two complaints about the same idiom: a Ctrl-drag planned a route
  and drew none of it, so a body setting off round the far side of a building
  looked like a body that had misread the click; and a destination the shard's
  own furniture had sealed off — the usual case being a shut door, the only way
  into a room — was "no route", which degraded to walking at it in a straight
  line and giving up four beats later somewhere that was not the door.

  The plan now reads the ground twice. `steer::Readings` carries both halves: the
  map with everything the shard placed over it (`clutter.rs`'s `Cluttered`,
  which is what every step is decided against) and the bare map, which is the
  same world with nothing placed in it and therefore every door open.
  `steer::plan` asks them in that order — the world as it stands answers first,
  so a door with a way round is a longer walk and never a barred one — and only
  where there is no way through at all does it plan over the bare map and *cut
  that route at the first step the real ground refuses*. The body walks the open
  half and stands in front of whatever is in the way; the clock is armed but
  nothing is sent, for the reason a heading wedged in a corner sends nothing (a
  step this end has proven the shard refuses comes back as a `0x21` and a
  rollback), so a door opened within the `STUCK_STEPS` patience picks the walk
  straight back up with no fresh click.

  **And where neither half has a way through, the answer is still a walk, never
  a shove.** `movement::find_path_toward` is `find_path`'s other reading of the
  same A*: where one says "no route", the other says how far the route got —
  the reached tile closest to the goal, `None` only when nothing reachable is
  any closer than where the body already stands. It costs the search nothing
  (every candidate already carries its distance to the goal, which is what A*
  orders the open list by), and it comes out of *one* search over one terrain,
  so "there is no way" and "here is how far the way goes" cannot disagree about
  which tiles were reachable. A destination clicked on a wall, on the far bank
  or simply out of budget is now planned up to the last tile before it and
  stopped at. The straight-line fallback is gone entirely, and with it the whole
  class of client-side step the shard was only ever going to refuse: nothing a
  destination sends is a step this end can already see is blocked.

  The picture is the same call. `App::route_shown` runs `steer::plan` and draws
  the open half green and the barred half red (`shell::draw_route`, the
  `STANDABLE`/`BLOCKED` pair the walkability wash already speaks in), whether or
  not the terrain overlay is switched on — a move order is not a debugging
  question. It replans per frame rather than drawing `Steering`'s stored route,
  because the walk plans at most once a step by design and clears its route the
  moment a drag restates the destination, so a line drawn from it would blink
  out under the moving cursor. What it must never be is a *second* cut: where
  the red starts is where the body will stop, and both come off one function —
  `docs/render/design_frame_assembly.md`'s standing argument, applied to a route.

  **What the two readings differ by is a list, and the list is doors.** The first
  cut of this had the optimistic half be the *bare map* — the client's files with
  nothing the shard placed — on the standing belief that this end cannot tell a
  door from a barrel. It can: `client/render/src/doors.rs` already carries
  ServUO's door families for the occlusion pass, so `clutter.rs` marks each
  blocker `door` and keeps the tiles the shut ones stand on. That matters for
  what the picture *means*: with the bare map, a route "through" a stack of
  crates nobody will ever move came back red and the body walked up to the
  crates; with the list, only a door does that, and a crate is simply a thing to
  route around or stop short of. It also matters for what the halves *are* — the
  server has the same pair under the same name (`Obstacle::door`,
  `LiveTerrain::through_doors`), so both ends draw the line in one place.

  **And it turned up a live bug.** `clutter.rs` argued that no door state need be
  tracked, since a door's graphic changes when it swings and only the shut leaf
  is impassable. Measured against the real `tiledata.mul`: all 164 shut leaves in
  the table are impassable and **so are 132 of the open ones**, so this end was
  refusing to walk through open doors — steps the shard allows, the mirror-image
  of the bug `clutter.rs` was written to fix, and invisible because the shard
  simply rolled the body back. An open leaf is now left out of the index
  entirely. `an_open_door_s_own_art_is_impassable_just_like_the_shut_one` pins
  the measurement, `an_open_door_is_not_in_the_way` the fix, and
  `a_shut_door_with_a_crate_in_it_is_not_potentially_passable` the case a naive
  "is there a door on this tile" would get wrong.

  `a_shut_door_plans_up_to_it_and_names_the_rest_barred`,
  `a_thing_in_the_way_with_a_route_round_it_is_not_barred`,
  `a_destination_behind_a_shut_door_is_walked_up_to_and_no_further`,
  `the_walk_resumes_the_moment_the_door_opens`,
  `a_destination_that_cannot_be_stood_on_is_walked_up_to_and_stopped_at` and
  `a_destination_with_nowhere_closer_to_stand_sends_nothing_at_all`
  (`client/app/src/steer.rs`) pin the claims, and
  `an_unreachable_goal_is_walked_toward_until_the_ground_runs_out` /
  `nothing_closer_to_stand_is_nothing_to_walk` /
  `a_reachable_goal_is_the_same_route_either_way` (`common/movement/src/path.rs`)
  pin the search under them. Opening the door is still nobody's job but the
  player's — see the roadmap for why that stays a gameplay decision.

- ~~**The walk stuttered once a tile.**~~ Three causes, all of them in the same
  400ms:

  - **The glide's length was the nominal step and the steps do not arrive
    nominally.** Finish early and the body stands on its tile waiting for the
    next packet; finish late and that packet yanks it forward from wherever it
    had got to. So a walk already under way crosses each tile in the time the
    *last* crossing took (`crowd::glide_time`), believed only within half and
    double the wire's own claim — outside that band the gap is not a pace but a
    body that had stopped, or two steps in one burst. This is also the only thing
    that can glide an NPC correctly: nothing on the wire says what pace a
    creature walks at.
  - **The animation hold was that same one number**, so it expired in the gap
    between two steps — one frame of *standing*, which is a different group, so
    the walk's clock restarted at frame zero every tile. The crossing and the
    animation are two numbers now: `animation_hold` keeps the walk playing half a
    step past the landing, and a body that has genuinely stopped walks on the
    spot for 200ms that nobody notices.
  - **The animation clock was armed at the standing rate when the step
    arrived.** `FRAME_DELAY` is 80ms and a crowd where nobody is gliding waits
    it out, so the first 80ms of every glide was drawn frozen at its start.
    `App::user_event` pulls the tick forward to `GLIDE_INTERVAL` when the packet
    it just folded in started somebody moving.

- ~~**Our own body waited for the round trip.**~~ It is predicted now, which is
  the fourth and largest share of that same stutter: a step used to be drawn when
  the `0x22` acked it, so the body stood still for the latency, crossed its tile,
  and stood still again — and the *jitter* of that latency, not the latency
  itself, is what no interpolation can smooth out, because it moves the start of
  each crossing rather than its length.

  `Walk` already kept a `predicted` and deliberately refused to write it into the
  `WorldView`; that refusal is right and stands. What was missing was a second
  channel: `link::Update::World` now carries a `link::Body` — the prediction, and
  whether it got there by a correction — beside the view, and the app draws the
  body from *that* while everything else still reads the view. The ack becomes
  invisible: it confirms a position the screen already has. Only a `0x21` or a
  `0x20` changes anything, and it is a rollback.

  Two things the rollback turned up:

  - **A rollback must not be glided.** It is one tile, so the "more than one tile
    is a teleport" rule does not catch it — and glided, a body refused by a wall
    strolls *backwards* a tile every step. `Crowd::snap` puts it there instead,
    and deliberately leaves the animation alone: a walker whose third step is
    refused is still walking.
  - **A rollback is not a pace sample.** The gap between a step and its refusal
    is latency, and feeding it to `glide_time` would have the next crossing take
    a quarter of a step. `snap` drops the measurement with the step.

  What is deliberately *not* predicted: whether a step is allowed at all. That
  needs every rule about statics, doors and mounts to agree exactly with the
  server's, and being wrong about it costs a rollback where being wrong about a
  height costs a few pixels. The server is the authority and the `0x21` is how it
  says so.

- ~~**Every unit of the walk is tested and the walk itself is not.**~~ It is now:
  `crates/client/app/src/dst.rs` runs the whole path — `steer.rs`,
  `client/net`'s `Walk`, a real `openshard_movement::Walker` for the shard, and
  the `Crowd` — on a virtual clock, over a wire with latency, jitter and a wall
  in it, and holds the position of the *sprite* against an oracle.

  The oracle is the **intent** timeline: the body leaves the instant the key goes
  down and crosses one tile per hold, for ever. It is built from the script of
  inputs alone, it is constant velocity and nothing else — no turn tax, no ramp,
  no easing. Everything under test is the **event** timeline — when the loop
  woke, what the wire did — and the claim is that the second reproduces the
  first. Not a tautology: every walking bug this client has had
  is a divergence between those two sets of knots, and the harness found three
  more that four green unit suites did not.

  - **A turn stopped the pace measurement without being one.** `glide_time`
    measures the gap since the last *position* change, and a turn changes no
    position — so the step after a turn was measured across two holds, which the
    band was just wide enough to believe, and the tile after every turn was
    crossed at half speed. A turn records the pace sample now: it is a step in
    UO, it just covers no ground.
  - **The crowd's clock was a frame behind whenever a packet was folded into
    it.** A step is timestamped with `Crowd`'s own `now`, and `user_event` folded
    packets in between two `advance` calls — so every step was recorded at the
    *previous* frame's instant, up to a whole `FRAME_DELAY` for a body that had
    stopped. `glide_time` takes a difference of two of those, so the error landed
    on the crossing's length. `user_event` advances the crowd before it folds.
  - **The event loop's wake jitter accumulated into the walking speed.**
    `steer.rs` armed the next step at `now + interval`, and the loop is woken by
    the operating system whenever it gets round to it and never early. A few
    milliseconds a step is a body a fifth of a tile behind after ten and a whole
    tile behind after fifty, and nothing ever gives it back. The next step is
    armed from the deadline that has just passed; a wake later than a whole step
    is a stall rather than jitter and restarts the cadence, because those steps
    are deliberately not banked.

  One design decision came out of it: **the pace of our own body is not measured,
  it is commanded** (`Crowd::commanding`). `glide_time` exists because nothing on
  the wire says how fast a creature walks — but we send our own steps, so the
  nominal hold is not an estimate of that walk, it *is* the walk. Measuring it
  anyway feeds the loop's wake jitter into the crossing length, and consecutive
  gaps jitter in opposite directions, so the estimate was worse than the constant
  it replaced.

- ~~**A turn cost the player 400ms of standing still.**~~ It costs nothing.
  Turning is a whole step in UO — the mobile turns, moves nowhere, and gets its
  own `0x22` — and `steer.rs` used to charge it a whole hold, so pressing a new
  direction stood the character still for a step before it set off. Nothing asked
  for that: the shard answers a turn *before* it charges the pace budget
  (`Walker::request`, and the reference it is ported from does the same, because
  spinning on the spot is something clients do and throttling it would be
  absurd), so the step a turn precedes is legal in the same instant. `steer.rs`
  arms it at once and `App::about_to_wait` takes up to two steps in one wake, so
  the turn and the step it is for leave together. The oracle in `dst.rs` is what
  states the requirement: it charges nothing for a turn, and a walk that starts
  facing the wrong way tracks it from the first millisecond.

- ~~**The animation clock was read when the timer fired, not when the frame was
  built.**~~ A glide is a position read off a clock, so the moment that clock is
  read has to be the moment the picture is built. `App::about_to_wait` advanced
  the crowd and then asked for a redraw; between the two the loop laid out the
  UI, grew an atlas and waited on the swapchain, and however long that took was
  error in the body's position — error that varies frame to frame, which is what
  an eye reads as a stutter rather than as lag. `App::draw` advances the clock at
  the top of the frame now.

  With it, the other half of the same judder: the timer's 16ms beat against the
  display's 60Hz, so a frame landed on the wrong side of the beat about once a
  second. A body mid-step asks for its next frame at the end of `draw` instead,
  and the surface's FIFO presentation paces the walk at the display's own rate.
  The timer stays for everything else — a still world redraws on the animation
  clock and sleeps in between.

- **The camera is on whole world pixels.** `Camera::eye` is an integer
  `WorldPixel` and a step east crosses 22 of them in 400ms, so at 60Hz the ground
  moves 1, 1, 0, 1 pixels a frame rather than 0.92 — a half-pixel wobble at zoom
  1 and a whole one at zoom 2, which is the last quantisation left in a walk.

  **Worth knowing before changing it: ClassicUO does not solve this, it has the
  same quantisation.** `Mobile.Offset` is three `sbyte`s and
  `GameSceneDrawingSorting.UpdateDrawPosition` builds the scene's origin out of
  `int`s (`winGameCenterX -= (int) Player.Offset.X`), so the reference client
  everyone calls smooth is also stepping the ground a whole pixel at a time. Its
  `Renderer/Camera.cs` keeps floats and lerps — but only for the *peek* offset
  and the zoom, and `Camera.Transform` casts to `int` at the end. So a fractional
  world is ours to invent rather than ours to copy, and the reason to want it is
  zoom: at 2× a whole world pixel is two screen pixels, which is the case where
  the judder is actually visible. The cost is that a sprite quad on a fractional
  boundary samples its atlas unevenly and the art shimmers inside the sprite,
  which is why pixel-art engines that do this snap *sprites* to whole pixels and
  let only the ground carry the remainder.

- **The camera tracks `z` exactly, and stairs bob.** A step up is four world
  pixels of vertical (`camera::project`'s `Z_STEP`), and the eye follows every
  one of them. ClassicUO does the same thing —
  `winGameCenterY = ... + (Player.Z << 2)` plus the interpolated `Offset.Z`, no
  damping anywhere — so a bobbing camera on a staircase is what UO looks like,
  not a defect we introduced. Damping it is a deliberate improvement and it is
  cheap: the eye already takes a `WorldPixel`, so a critically-damped follow on
  the vertical axis alone (spring, or a first-order lag with a time constant
  around a step) leaves the horizontal walk exactly as it is. It has to be
  *bounded* — an eye that lags a recall or a teleport is worse than one that
  bobs — which is the same "more than one tile is not glided" rule the crowd
  already has. `Camera::eye` is an integer
  `WorldPixel` and a step east crosses 22 of them in 400ms, so at 60Hz the ground
  moves 1, 1, 0, 1 pixels a frame rather than 0.92 — a half-pixel wobble at zoom
  1 and a whole one at zoom 2, which is the last quantisation left in a walk.
  Fixing it means a fractional eye whose remainder is applied to the ground
  diamonds and the sprite quads (both are already `f32` at the GPU), and it is
  not free: a sprite quad on a fractional boundary samples its atlas unevenly, so
  the art shimmers inside the sprite. Worth measuring before doing — the two
  clock defects above were the visible share of this complaint.

- **The picture and the truth are the same number, and they should not be.**
  This is one mechanism from modern third-person and isometric action games —
  Diablo, Path of Exile and everything shaped like them — and it is the answer to
  three separate complaints here. Those games are continuous where UO is a grid,
  but the part worth taking is not the continuity, it is the *split*:

  - An entity has an **authoritative** position — what the server said, plus
    whatever the client has predicted on top — and a **drawn** position, which is
    the authoritative one plus an error that is decaying toward zero. Everything
    that is not the picture reads the authoritative one: the depth order, what
    the body can walk behind, what the camera says is on screen.
  - A correction never moves the picture. It moves the authoritative position at
    once and *puts the difference into the error*, which then shrinks with a
    half-life of something like 100–150ms. The body is where the server says it
    is immediately, and it arrives there smoothly. Unreal calls this
    `NetworkSmoothingMode`/`SmoothCorrection`, Source calls it `cl_smooth` with a
    `cl_smoothtime` of a tenth of a second, and every engine has one.
  - The error is **bounded**: past a threshold the correction is a teleport and is
    snapped, because sliding a body across half a facet is a stranger picture than
    the jump it hides. The same rule the crowd already applies to a move of more
    than one tile.
  - The decay is **frame-rate independent**: a half-life and `0.5^(dt/half)`, not
    a per-frame `lerp(a, b, 0.1)`, which is a different curve at 30fps and 144.

  For us that is a drift in world pixels on `Mobile`, set by `Crowd::snap` from
  where the body was actually drawn, decayed in `Crowd::advance`, and added in
  `mobiles::world_position` — where the camera reads it too, so both follow. It
  replaces "a rollback must not be glided" with something better than either
  answer: the tile the body was put back on is still never *walked* across, but
  the picture is not yanked either. It also absorbs whatever jitter is left in
  the arrival of a step, which is the same defect wearing a different hat.

- **The eye follows `z` exactly, and a staircase bobs.** The second thing those
  games all do: the camera is a *smoother over* the character, per axis, with
  different time constants — the horizontal tight or exact, the height loose,
  a few hundred milliseconds, because terrain height is the axis that steps. A
  camera locked to the character's exact height turns every stair into a jolt of
  the whole world, and no isometric game since about 2005 ships that.

  Cheap here, and independent of everything else: `Control` keeps the eye's
  height as an `f32`, pulls it toward the height the body is drawn at with a
  half-life around 300ms, and lifts the eye by the difference times
  `camera::Z_STEP`. The horizontal axis stays exact for now — a lagging eye on
  the walking axis is a separate decision with its own feel, and it is not what
  the stairs complaint is about. `Home` and `relock` stay instant: a rubber band
  on a deliberate jump is a bug, not a feel.

- **A fractional world, eventually.** With the two above done, what is left of the
  judder is the whole-pixel eye, and the fix is the one modern pixel-art engines
  use: keep the camera's position fractional, give the *ground* the remainder,
  and snap *sprites* to whole pixels so their art does not shimmer. Worth doing
  for zoom, where one world pixel is two screen pixels or more. Last, because it
  touches every quad and the two above touch a dozen lines each.

- **Nothing is predicted about whether a step is allowed, so a wall is a
  rollback.** This is the largest remaining gap against the reference, and it is
  what "the client does not respect being pushed back" and "obstacles are not
  walked around" both come down to. What ClassicUO does, from the clone:

  - **It never asks for a step it knows is illegal.** `PlayerMobile.Walk` calls
    `Pathfinder.CanWalk` before it queues anything and returns `false` if the
    ground refuses — so walking into a wall produces *no packet at all* and no
    rollback. The body walks on the spot, which is what a player expects, rather
    than lurching a tile and being pulled back.
  - **A refusal stops the walking.** `WalkerManager.DenyWalk` clears the step
    queue, resets the sequence, forces the position, and
    `SyncServerDirection()`s the facing; a `0x22` for a sequence it is not
    waiting on sets `WalkingFailed` and sends a resync — and `WalkingFailed` is
    the *first* condition in `Walk`, so nothing more is sent until the resync
    lands.
  - **The queue is capped.** `Constants.MAX_STEP_COUNT = 5` unconfirmed steps,
    and `Walk` refuses to add a sixth. Ours caps nothing on purpose (`Walk::in_flight`
    says so), which is fine while the shard is the only limit and wrong the
    moment the shard stops answering.
  - **Click-to-walk is A\*.** `Pathfinder.FindPath` with
    `PATHFINDER_MAX_NODES = 10000` and a Chebyshev heuristic, over the same
    `CanWalk`. Ours is greedy-with-a-stall-counter, which is the honest thing to
    do with no terrain and the wrong thing to keep once there is one.

  We can do better than the reference here rather than copy it: ClassicUO
  re-implements UO's walkability in the client and can therefore *disagree* with
  the shard, while `openshard_movement::Terrain` is one trait both ends already
  speak and `crates/server/world/src/terrain.rs`'s `MapTerrain` implements it out
  of `WorldMap` + `TileData` and nothing else — no world state, no server crate.
  Moving it below `server/` (it is `common/*` material sitting in the wrong
  group, and `crates/common/movement` already owns the trait) gives the client
  the shard's own rules, byte for byte. Then, in order:

  1. `MapTerrain` moves to `common`, with the server importing it from its new
     home. No behaviour changes; the test suite that pins its Sphere and RunUO
     arithmetic moves with it.
  2. `Walk::step` gains the terrain and refuses locally, exactly where its doc
     comment currently explains why it does not. That comment stops being true
     the moment the two ends share an implementation, which was its own premise.
  3. `steer.rs`'s greedy route becomes `openshard_movement::find_path` — already
     A\* with a Chebyshev heuristic over the same `Terrain` — and `STUCK_STEPS`
     stops being the only answer to a wall.
  4. ~~A rollback tells `Steering`, which it currently does not.~~ Done with the
     queue rule below: `Steering::corrected` takes the shard's facing and
     `App::entered` calls it whenever `link::Body::corrected` is set.
  5. ~~A cap on steps in flight, with the reference's five as the number.~~ Done:
     `walk::MAX_IN_FLIGHT`, checked first thing in `Walk::step` as the reference
     checks it first thing in `PlayerMobile.Walk`. It is not a second pace limit
     — the shard's budget is the only judge of how fast a body walks — it is the
     answer to a shard that has *stopped answering*, where every further step is
     another tile of correction when the link comes back. `Walk::step` now
     answers `NotSent`, which is that refusal and the world's edge in one type;
     `link.rs` logs either and sends nothing, so the body waits where it is.

- ~~**An input takes a step whenever it arrives, and the step under way is cut
  short.**~~ Fixed, and the rule it was fixed with is worth stating on its own
  because everything about walking now depends on it:

  > **An input joins the queue or rebuilds it. A step already begun ticks out.**

  Two complaints, one defect. Walking east and pressing west mid-stride jumped
  the camera; mashing the arrows sent the body flying off its own position and
  being dragged back. Both were `Steering` sending a step at the moment an input
  arrived rather than at the moment the walk was free for one — a turn costs the
  shard nothing, so the turn *and the step behind it* went out on every press,
  and a release disarmed the clock entirely, so press-release-press bought a step
  per tap.

  Three things go wrong at once when a step leaves early, which is why the rule is
  one rule and not three fixes:

  - **The picture.** `crowd.rs` starts each glide at the tile the *previous* step
    ended on, so a step issued half a hold early yanks the body forward to a tile
    it has not reached — half a tile in one frame — and the camera is locked to
    the drawn body, so the world jumps with it.
  - **The pace.** The shard's `WalkPace` refuses a body asking for steps faster
    than a body walks and answers `0x21`, which is the flying-off-and-being-
    dragged-back.
  - **The wire.** That rollback races the steps still in flight; their acks arrive
    for a sequence this end has forgotten, and `Walk::on_packet` calls that an
    `UnexpectedAck`. `link.rs` treats one as fatal, so a determined key-masher
    could *drop their own connection* — see the backlog below.

  The mechanism is small: `Steering::due` stops being "the walk is running" and
  becomes a floor that nothing clears, `Steering::free` is the one gate every ask
  goes through, and a turn no longer moves the deadline — it leaves in the same
  wake as the step it precedes and that step is what charges the clock, so the
  pair is one ask against the floor. What the queue *is* is `Steering::take`
  reading the keys at the moment the step leaves rather than when they were
  pressed: one step deep, rebuilt by every press for nothing.

  The oracle in `dst.rs` gained the same rule — a press while a step is under way
  moves no knot — and four scenarios hold the picture to it: the reversal, twenty
  reversals at 270ms so every phase of a step is interrupted, thirty presses a
  second through three directions, and one arrow tapped. Two assertions beyond
  the corridor, because a corridor is blind to a jump forwards and back inside
  it: `continuous` bounds how far the drawn body may move between two frames by
  what a walk covers in that time, and `paced` bounds how close together two
  crossings may be asked for. All four failed before the fix, by 0.5 tiles, 1.89
  tiles, a dropped connection and 0.74 tiles respectively.

  One trap found on the way, and it is the reason `Steering::walking` exists: the
  next step is measured from the *deadline* rather than from the wake, which is
  what stops a late loop accumulating drift — but a deadline that came and went
  with the arrows up is not a cadence. Measuring from it made the step after a
  fresh press due a fraction of a hold later, which cut the glide short and
  jumped the body exactly like the defect being fixed. A deadline is only a
  cadence if a step was taken at it.

- ~~**An ack that arrives after a rollback ends the session.**~~ Fixed, and it was
  two bugs wearing one hat. Found by the key-mashing scenario in `dst.rs` before
  the queue rule removed the flood that provoked it: a `0x21` voids everything in
  flight and resets both sequences, but the shard owes one answer per `0x02` and
  the steps already on the wire are still answered — so an answer lands for a
  sequence this end has forgotten.

  Both halves were wrong. `Walk` called those answers a desync, which they are
  not: the wire delivers in order, so while anything is owed from before the last
  correction the next answer is one of *those*. `Walk::draining` counts them and
  they are swallowed — including a stale `0x21`, which is the half that had no
  symptom anybody would have named: applying it rolls the body back a second
  time, onto a tile it has already walked away from, and clears the steps sent
  since. The DST scenario measures exactly that, and without the counter the
  drawn body ends four tiles behind the shard. An answer owed to *nobody* is
  still an `UnexpectedAck`, so a real desync is still reported.

  And `link.rs` turned any error out of `fold` into `Update::Lost`, so the window
  closed. What is left after the drain is a genuine disagreement, and that has an
  answer on the wire rather than a reason to hang up — which is the item below.

- ~~**Neither end speaks the resync request.**~~ Both do now, and it is what makes
  stopping the walk safe. The whole cycle, and it is a request/response rather
  than a hope — the argument, read out of both references, is in
  `docs/findings.md`:

  1. An answer this end cannot place sets `Walk::out_of_step`, and while it holds
     `Walk::step` sends nothing (`NotSent::OutOfStep`). It has to: `predicted` is
     a chain of asks the server has stopped agreeing with, so every step on top
     widens the disagreement, and a `0x22` ack carries no position to correct it
     with.
  2. `link.rs` sends one `ResyncRequest` — the client's `0x22`, three bytes —
     guarded on the flag not already being set, which is ClassicUO's
     `ResendPacketResync` in one line.
  3. The shard decodes it as `ClientPacket::ResyncRequest`, queues
     `Command::Resync` like every other packet, and `WorldState::resync` answers
     out of a tick: the walk sequence back to zero, this client's screen
     forgotten so that `refresh_around` sends it again, and a `0x20` with the
     real position. That is ServUO's `Resynchronize` list in our own terms.
  4. The `0x20` snaps the client, which clears the flag, and the walk is free —
     from a fresh sequence on both ends, which is why the step after a resync is
     not refused.

  `0x22` is **two different packets**, one per direction, three bytes each, with
  nothing in the body to tell them apart. It costs us nothing because
  `ClientPacket` and `ServerPacket` are separate tables, but it is exactly the
  sort of thing a single id-to-type map gets silently wrong, so both types name
  each other and sit together in `world.rs`. `crates/e2e/shard/tests/resync.rs`
  is what proves the packet one end sends is the packet the other end decodes;
  neither unit test can.

- **The walk has no home.** Two of those three defects lived *between*
  `App::user_event` and `App::about_to_wait` rather than in any of the four units
  the walk is made of, and the harness had to copy those handlers' ten lines to
  reach them — which is exactly the thing a test must not do, because a
  divergence introduced into the copy is invisible to it. What is wanted is a
  headless unit that owns the walk end to end: the steering clock, the
  prediction, the crowd's clock and the order the three are touched in, driven by
  `(now, input, update)` and answering `(steps to send, where the body is drawn,
  when to come back)`. `App` becomes the window's adapter to it, and the oracle
  drives the shipped code rather than a copy of it. Everything in this entry
  argues for it; nothing here is a reason to have waited for it.

- **The crowd cannot tell a mount from a body on foot, and a mount steps twice as
  fast.** `WALK_HOLD`/`RUN_HOLD` are the two on-foot rates; ServUO's other two,
  `WalkMount` (200ms) and `RunMount` (100ms), have nothing here to select them —
  the mount is not on `0x77` at all, it is an equipment layer on `0x78`. So a
  mounted mobile is held and glided at half the speed it is really moving, which
  looks exactly like the runner case above. This wants the same `MobileView`
  layering the "equipment, mounts and corpses" entry does, and should land with it.
- **`Home` still snaps.** `Control::relock` takes a tile and jumps the eye to it;
  the next frame's `follow_body` then puts it back on the glided pixel. Harmless
  and visible for one frame — the snap is deliberate, the *inconsistency* between
  the two doors is not.
- ~~**Ground items are decoded, held, and not drawn.**~~ Drawn.
  `crates/client/render/src/items.rs` is two of the existing collectors put
  together: an item's picture is a static's, and its source is a mobile's — a
  list somebody else built out of what arrived on the wire. The placement has
  one copy, `statics::stand_on`, and one atlas serves both, because a floor tile
  packed twice is a floor tile twice. What that made visible: "does the atlas
  cover this frame" has to be *one* question — asked twice with the item half
  forgotten in one of them, the atlas is rebuilt every frame an item is on
  screen and never holds it, which is a stutter rather than an error.
  The *amount* used to be deliberately left out of this, on the grounds that a
  pile of 500 gold is one sprite. Both halves of it are done now: the coin
  bands pick the sprite (`items::displayed_graphic`, from the shard's own
  graphic and the count), and the count itself is written over the pile —
  `items::stack_label` decides whether there is a number and what it reads,
  `items::labels` hangs it over the picture in the world, and
  `container::amount_label` puts it in the bottom-right corner of an icon in a
  bag and of the pack on the cursor. One rule for all three, because a pile
  counted in a bag and silent on the floor is the same pile telling two
  stories. **Not the reference client's picture**: no 2D client writes a count
  on a pile — the classic one puts the number in the name over the item and
  ClassicUO draws the art a second time five pixels up and left — so this is
  the client's own addition and the whole rule is stated at `stack_label`
  rather than cited to a reference that has none.
- **A pile's count has no switch.** Every counted pile on screen is written
  over, always — a bank floor with fifty piles on it is fifty numbers, and
  there is no knob in the dev window's Graphics tab to turn them off the way
  there is for the crowd or the statics. What *is* answered: the count has a
  size of its own now (`desk::FontSizes::stack_count`, 11 pixels by default,
  against speech's 16) whichever face is running — see `docs/render/design_text_sizes.md`,
  which this entry's second half asked for and which turned out to be one
  atlas keyed by `(char, size)` rather than the second atlas it predicted.
- **The facet is a startup constant and `0x1B` only carries a size.** The app
  loads Felucca and compares the shard's map size once, warning when they
  differ rather than following. Following means decoding `0xBF 0x08` and
  reloading the facet, and the reload is the interesting half:
  `WorldMap::load_facet` reads a few hundred megabytes. **M3b makes this
  blocking**: two sessions may stand on two facets, so the single shared
  `Arc<WorldMap>` has to become a cache keyed by facet.
- **A whole `WorldView` is cloned per changed packet.** Fine for the handful a
  standing character receives, and not fine beside a crowded bank: the thread
  clones the map of every mobile to say that one of them turned. The answer is
  probably not a delta protocol between the two threads but a shared snapshot
  the window reads — worth measuring before deciding, and **M3b multiplies it by
  the number of sessions**, so the measurement should happen before the count
  goes up rather than after.
- ~~**`z` still drifts on a hill, and now it is visible.**~~ Fixed by handing
  `Walk::step` the ground as a function of a tile; the window shares the facet
  it already loaded with the shard thread through an `Arc`, since it is plain
  data read by both and written by neither. Deliberately the *ground* and not
  `movement::Terrain`: this predicts a height and must not predict a refusal —
  whether a step is allowed is the server's answer, and deciding it here would
  need every rule about statics, doors and mounts to agree exactly. What is
  still flat: a step onto a *floor* — a building's second storey is a static,
  not land, and the height predicted for it is the ground underneath. A pier
  or a bridge is the same case with a visible symptom rather than a subtle
  one: reported by a player 2026-08-02 as falling underground specifically on
  piers and bridges, because the predicted Z sits at the water or ravine floor
  under the deck rather than the deck itself, and `ground.rs` draws no plane
  at the deck's height either — see the matching entry in "found while
  drawing the ground". `App::walk`'s offline path (`lib.rs:1167`) has the
  identical gap: `self.map.land(x, y)`, no static.

## Backlog, found while building M0, M1 and M1a

Each is a seam the work made visible. None blocks the next milestone.

- ~~**A walking client has no map, so `z` never changes under it.**~~ It has
  one now: the height is an argument to `Walk::step`, and `|_, _| None` is what
  a caller without a map passes — the e2e walk test, which is about the sequence
  seam. See the entry above.
- ~~**`enter_world` drops everything between `0x1B` and `0x55`.**~~ Applied.
  That window *is* the world being handed over — the player's own `0x20` and
  `0x78`, a `0x78` for everyone already on screen, the ground items — and none
  of it is sent again, so the loop that waited for permission to draw was
  discarding what it was going to draw. What it exposed is that two of those
  packets name the client's *own* serial and mean something different when they
  do: a `0x78` about ourselves is the one paperdoll a shard ever sends us (the
  reveal pass shows a mobile to everyone except itself), so it dresses
  `Player`, which now carries an equipment list and is no longer `Copy`; a
  `0x77` about ourselves is not a move at all and is dropped, because acting on
  it would fight `Walk`'s prediction. Both are routed by serial, which is what
  keeps `WorldView::mobiles` the *other* mobiles it claims to be. The e2e test
  asserts the backpack every character wears, since the only packet that
  mentions it lives inside that window.
- **Nothing on the client models a status bar, and two packets are waiting for
  one.** `MobileStatus` (`0x11`) decodes and is deliberately not folded into
  `WorldView`; `WalkAck`'s notoriety reaches the caller through
  `walk::Moved::Stepped` and has nowhere to go either. Both are the same
  missing thing — health-bar colour and paperdoll numbers are not positions —
  and both should land wherever M4's status bar does.

- **The shard sends the feature mask and the character list as one write.**
  Correct, and worth naming: it means "one compressed block" and "one packet"
  are different things on this wire, and any future reader — a proxy, a packet
  logger, a second client — has to keep the two layers apart.
- ~~**`ServerPacket::decode` covers the login set only.**~~ Fixed: `0x20`,
  `0x11`, `0x77`, `0x78`, `0x1A` and `0x1D` all decode now. `WorldView` folds
  five of them in — a client's own body, every other mobile, every ground
  item, and what `0x1D` takes back off screen. `MobileStatus` (`0x11`) decodes
  too but stays out of `WorldView`: it is paperdoll data, not a position, and
  belongs with whatever eventually models the status bar. Its `max_weight` is
  honestly lossy below status type 5 — the wire never carries it that old, so
  decoding gets `0` back rather than a guess at a real value.
- **`CharacterList` decodes only the post-7.0.13.0 form.** The older start list
  carries no coordinates, so there is no honest `StartLocation` to build; the
  decoder says so rather than inventing zeros. If this engine ever wants to be
  a client to an *old* shard, that is where to start.
- ~~**The client-to-server encoders are still labelled "test fixtures only".**~~
  Fixed: `AccountLogin::encode`, `SelectShard::encode`, `GameServerLogin::encode`
  and `CharacterPlay::encode` now say what `crates/client/net`'s login state
  machine (`session.rs`) actually calls them for. Only `ClientVersionReport::encode`
  is genuinely still test-fixtures only, since the client does not announce its
  version yet.
- **`Login` fixes the seed value at `0x0A000001`.** It is never read — see
  `RawSeedValue` — but a client that will one day face a shard implementing
  login encryption will need it to be the value that keys the cipher.
- **The `0x82` refusal loses why.** `DenyReason::from_wire_code` returns one
  reason per wire code because the wire has five, and the server collapsed
  fifteen into them. Nothing to fix on this side; worth remembering before
  anyone builds a UI that explains a failed login.

## Backlog, found while giving the client a speech line and the gump reader

- ~~**`0xAE` does not decode, so a client never sees its own words.**~~ Fixed.
  `UnicodeMessage` decodes, and the journal is now `VecDeque<Heard>`: `0x1C` and
  `0xAE` are one event in two encodings, so the journal holds a type that says
  so rather than one of the two packets standing in for both. Both fold in
  through the same cap. Nothing above `client/net` had to change for it — the
  overhead speech and the HUD's strip read the same fields — which is the sign
  the type was the right seam.
- ~~**The gump reader ignores hue.**~~ Fixed for text: a `{ text }` or
  `{ croppedtext }` hue is looked up in `hues.mul` and the label is drawn in it,
  which is what makes `.admin`'s "lay the world down" verbs read green and its
  "clear" verbs red. The column is ClassicUO's `HuesLoader.GetUnicodeFontColor`
  — `ColorTable[8]`, cited beside the constant, and pinned by a test on a ramp
  with a different colour in every column, so a wrong column reads as a wrong
  colour rather than a plausible one. `Hue(0)` stays "no colour", not row zero.
- ~~**The gump reader draws no art.**~~ Fixed for `gumppic`, `gumppictiled`,
  `resizepic`, buttons and switches: `client/render/src/gump.rs` is a pass of
  its own — no projection, no depth, no place attachment, because an interface
  is none of the three — and it tints per pixel through the same ramp the
  statics do, so a hued `{ gumppic }` is hued. `tilepic` is *not* done: that is
  static art, in `StaticAtlas` and not the gump atlas, and a window drawing it
  needs a second pass bound to a second texture. Below.
- ~~**A gump is drawn in points, not in the client's pixels.**~~ Withdrawn: this
  was written as a bug and is a decision, now argued in `client/app/src/gump.rs`
  and localized there. A layout's coordinates *are* the reference client's
  pixels, but converting them to physical pixels would be wrong on both counts —
  what is drawn here is egui widgets, whose text and padding are measured in
  points, so scaling the coordinates alone pulls the rows together underneath
  text that did not shrink with them; and the reference client predates display
  scaling entirely, so "what it does" is nothing and copying that gives postage
  stamps on a 4K screen. It stops being a decision the day gump *art* is drawn,
  because a bitmap cannot be reinterpreted — and then one scale has to apply to
  the coordinates **and** the font sizes together. Every layout number now
  passes through `gump::point` or `gump::size`, which is where that day's change
  goes.
- **A radio group is every radio in the layout, not every radio on the page.**
  `client/app/src/gump.rs` clears the other radios when one is set, across the
  whole window: no dialog this engine draws has two groups, so nothing shows the
  difference yet. The client's own rule is per page, and a pack's gump with two
  groups on one page would answer with both set.
- **`{ nodispose }` is not honoured.** The right button dismisses a dialog now —
  with an answer of button zero, which is what the reference's close box sends —
  and `{ noclose }` refuses it. `{ nodispose }` is a *server*-side "do not let
  this be closed even by that", and this client reads it and drops it: the two
  flags want telling apart before either is honoured properly.
- **The speech line has no history and no modes.** No up-arrow recall, and
  everything is said as `TalkMode::Regular`: emote, whisper and yell are the
  same packet with another mode byte, and there is nothing in the UI to pick one.
  Still true after the line moved off egui — `App::Chat` only holds what is
  typed, the caret and whether it is focused.
- ~~**The speech line and journal are egui's.**~~ Fixed: `App::chat`,
  `App::window_event`'s keyboard routing and `App::draw`'s use of
  `text::{GumpLabel, collect_gump}` replace `shell::speech_line` and
  `shell::Hud::said` — see the M4 section above. What did not move with it:
  no IME composition (a `KeyEvent`'s own `text` is enough for `fonts.mul`'s
  ASCII table, and a face with no Cyrillic or CJK glyphs has nothing an IME
  popup would help read anyway); no text selection or clipboard cut/copy/paste;
  no mouse hit test to focus it — Enter is the only way in, matching the
  reference client's own gesture.

### Found while drawing the art

- ~~**`tilepic` and `tilepichue` still draw a placeholder.**~~ Fixed twice
  over: the picture came first — `GumpArt::Item` puts static art in the *gump*
  atlas beside the gump art, keyed so the two overlapping index spaces cannot
  answer for each other — and the placeholder that used to be drawn on top of it
  went with the egui half. A `{ tilepic }` is one picture in the window's list
  now, like every other.
- ~~**Gump text is still egui's font, not the client's.**~~ Fixed. A `Caption`
  is resolved against the gump's own text table by `Dialogs::lines` and drawn
  through `text::collect_gump` on the pass bound to `App::font_atlas`, tinted by
  the same ramp the pictures are. Two things came out of it that the egui path
  had wrong: the layout's text hue is one *less* than the wire hue it means, and
  `{ croppedtext }` crops rather than wraps. What is left is the face — see the
  `unifont.mul` entry below.
- **`{ checkertrans }` is not drawn at all.** It is a translucent darkening and
  the pass discards rather than blends, deliberately (`gump.wgsl`). Drawing it
  wants either a blend state on a second pipeline or the client's own
  checkerboard, which is what the reference actually uses — a 50% dither, not an
  alpha.
- ~~**The window is still egui's, and that is the next decision, not a bug.**~~
  Taken, and it went the way this entry guessed: `WindowSubject::Dialog` is the
  third kind of subject in `App::own_windows`, the layout is built where egui's
  rectangle used to be, and dragging, z-order, the close gesture, `{ nomove }`
  and `{ noclose }` are all ours. See decision 7 in M4 for what the egui window
  was actually costing — three rectangles for one button.
- ~~**A button's click target is `BUTTON_SIZE`, not its art.**~~ Fixed, and for
  free as predicted: a button is a picture in the window's list, and a click on
  one is an opaque texel of that picture (`gump::pick`). There is no rectangle
  left to be wrong.
- **Nothing bounds the gump atlas.** It grows as windows open and never shrinks,
  and `AtlasError::Full` is reported per window and then drawn without whatever
  is missing. A session that opens hundreds of distinct dialogs is not a case
  anyone has yet, but the eviction question is the same one `StaticAtlas` has
  and neither has an answer.
- ~~**`content_size` is estimated, so art can overflow its window.**~~ Gone with
  the thing that needed it: a window has no size at all now, only the list of
  pictures it drew — the same answer `container::size` losing its caller gave in
  decision 5.

### Found while giving the windows their layout

- **Gump text is `fonts.mul`'s face 1, and the reference's is `unifont.mul`.**
  `Label`'s constructor for a `{ text }` passes `isUnicode = true`, so the real
  face is a Unicode one this engine has no reader for. `gump::CAPTION_FONT` is
  the nearest thing shipped — the same face `PaperDollGump` names for its own
  title — and the cost is the character set: a shard writing a dialog in
  anything past Latin-1 gets those glyphs skipped rather than drawn. A
  `unifont.mul` reader is the fix, beside `font.rs`, and the same one the
  journal will want the day a shard says something in Cyrillic.
- ~~**A paperdoll's buttons press and do nothing.**~~ Seven of the eleven send a
  packet now, and the double click the three scrolls wanted is drawn — decision
  8 in M4, and `scroll_pairs` is the pair. What is left is not a gesture but
  four missing packets and two missing windows: **Help (`0x9B`), Profile
  (`0xB8`) and the party manifest (`0xBF 0x06`) have no packet in
  `openshard_protocol`**, and Options is a client window of our own that does
  not exist. Beside them, **Status and Skills send a request nothing draws the
  answer to** — the `0x11` and the `0x3A` arrive and are dropped by
  `WorldView::apply`, which is where a status bar and a skill list would start.
  Status on a *stranger's* doll sends nothing at all, deliberately: the shard
  answers a `0x34` about the connection and not about the serial in it, so it is
  a health-bar window that is missing rather than a packet.
- **Nothing remembers where a window was.** The reference keeps a per-container
  and per-paperdoll position across sessions (`UIManager.SavePosition`); this
  cascades containers from a constant and puts a dialog where the shard asked,
  every time. The `Desk` file is where such a thing would live.
- **`{ html }` and `{ htmlgump }` draw nothing.** They parse and are dropped:
  the tags want a parser, a scrollbar and a background flag, which is three
  features and not one. A shard's book and its quest text are the callers.
- **A gump's own scale is egui's `pixels_per_point`.** With the dialogs off egui
  that is no longer a shared space that must agree — it is only what makes the
  interface the same size as the panels around it. The day the panels go, this
  becomes a setting of the client's own, and `App::gump_scale` is the one place
  it is read.

## Backlog, found while chasing a slow debug build

- ~~**A frame walks the visible rectangle four times.**~~ Twice now, and the two
  that went were the expensive ones: `ground::visible_graphics` and
  `statics::visible_graphics` walked ~9,800 cells at 1080p on every frame purely
  to answer "is the atlas stale", against a camera that had moved one tile.
  `TileBounds::difference` subtracts the rectangle the atlases were last grown
  for from the one the camera wants and hands back the two or three thin bands
  between them — a step of one tile is one row — and `ground::graphics_in` /
  `statics::graphics_in` walk those. `App::covered` is the rectangle, and the
  invariant it carries is positional: every cell inside it has been offered to
  the atlases, so a graphic can only be new outside it. Which is why anything
  that makes an atlas *forget* has to clear it in the same breath, and why the
  rebuild path does.

  The other two walks are `ground::collect` and `statics::collect`, which build
  the quads and therefore have to see every visible cell. They are what the
  entry below is about.
- ~~**One new graphic at the edge of the view repacks every atlas.**~~ The
  atlases grow instead. Each one keeps its allocator — the land grid's next
  slot, the texture grid's cells, the shelf the sprites and the animation frames
  are packed on — plus the set of keys it has been *offered*, and `add` reads
  only what is genuinely new. What was written is recorded as a band of rows, so
  the upload is `write_texture` over that band into the texture already bound
  rather than three new `SpriteRenderer`s and 48MB: `Atlases::grow` then
  `Atlases::upload`, and a frame where the camera stood still touches no file
  and no GPU at all.

  Two things that had to come with it. The packers lose their global sort — a
  shelf is tallest-first *within one growth* now — which costs waste and not
  correctness, and a single `pack` still lays out exactly as it did, which is
  what keeps the frame tests exact. And growing needed an eviction, because
  rebuilding on every miss *was* one: see the entry on a failed repack above.
- **A growing shelf wastes more than a packed one, and nothing measures it.**
  `StaticAtlas` and `AnimAtlas` sort tallest-first, which is what makes a shelf
  worth using — and a growth can only sort *within itself*, so a frame that adds
  one 12-pixel sprite starts a row that no 200-pixel tree can share. The waste
  is bounded by the number of growths rather than by the art, and what it
  decides is how soon the eviction fires. Nothing reports how full an atlas is,
  so the first sign of this is a rebuild, which is invisible. A `used`/`capacity`
  line in the camera panel would cost nothing and is the honest place to start.
- **The dirty band is a bounding box, and one atlas can defeat it.**
  `TexmapAtlas` allocates first-fit over a cell grid, so a growth that lands in
  the first free cell near the top and another near the bottom widens the band
  to almost the whole texture — a 16MB upload for two textures. The land grid
  and the two shelves fill downwards and cannot do this. Worth a list of bands
  rather than one, if a profile ever shows the texmap upload at all.
- **`ground::visible_graphics` and `statics::visible_graphics` have no callers
  outside their own tests.** They are the whole-viewport form of
  `graphics_in`, which is what the app uses now. Either they are the public
  spelling and `graphics_in` is the private one, or they should go — a `pub fn`
  that only tests call is a decision nobody has taken.
- **`visible_tiles` widens by the whole `z` range on both axes and then takes an
  axis-aligned box.** `MAX_Z_LIFT` either way is 512 pixels of margin for a
  mountain that is rarely there, and the bounding box of a rotated rectangle is
  about twice its area — so most of the ~9,800 cells walked at 1080p are not on
  screen. Correct and generous; worth measuring against a `u`/`v` walk if the
  per-frame cost ever matters in release.

## Backlog, found while porting the client's cutaway and its culling

`crates/client/render/src/cutaway.rs` is `GameScene.UpdateMaxDrawZ`,
`Map.CalculateNearZ` and `CalculateObjectHeight`; the tie-break inside a tile is
`LessEqual` in `renderer::depth_state` plus the pass order. What was found on
the way and not done:

- **The client fades where this cuts.** Promoted out of this backlog and into
  the plan — see *What is still M3: a pass that blends*, which is where
  `ProcessAlpha`, `IsTranslucent`, foliage and `HasSurfaceOverhead` all land
  together, because they are one pass and not four features. What stays here is
  the pair that is neither a fade nor blocked on one: the season test
  (`IsFoliageVisibleAtSeason`) and the `TreeToStumps`/`HideVegetation` profile
  settings, both of which decide whether a graphic is drawn at all and neither
  of which has a profile to read yet.
- **The ground is not screen-culled and the statics now are.**
  `statics::on_screen` rejects a sprite whose rectangle misses the image, which
  is where most of the ±512-pixel `MAX_Z_LIFT` band goes. A land quad's screen
  extent is its four corner heights rather than a sprite's size, so the same
  test needs the stretched diamond's bounds — worth doing, and it is the same
  band being walked.
- **The atlas is still built from what the cutaway would hide.** `collect` drops
  a roof; `visible_graphics` still packs its art. Deliberate for now — the
  cutaway changes as the player walks and an atlas that shrank with it would
  repack every time somebody stepped through a door — but it means the atlas is
  sized for the widest case, which is worth remembering if packing ever fails.
- **`Cutaway::at` is recomputed every frame.** The client caches it against the
  player's `x`/`y`/`z` and recomputes on change. Two tiles and a flood fill is
  cheap, but `near_roof_z` allocates a 4,096-entry visited grid per call, which
  is a `Vec` per frame for nothing.
- **`Chunk.AddGameObject`'s `state == 1` arm is multis, and there are none.**
  When multis land, a multi at an equal `PriorityZ` sorts after the land and
  before everything else — which the current scheme (pass order plus
  `LessEqual`) cannot express, because it has no pass of its own between ground
  and statics. Either multis draw in the statics pass with an explicit sub-key
  in `depth::Order`, or they get their own pass.
- **`depth::mobile_priority_z` has no corpse or effect arm.** The client's
  `AddGameObject` gives a corpse `z + 1` like a mobile and a `GameEffect`
  `z + 2`. Both belong with whatever draws them.
- ~~**`Cutaway::at` was fed the unconfirmed prediction, not a trusted
  position.**~~ `App::draw` (`client/app/src/lib.rs`) read `self.player.at` —
  `link::Body`'s own optimistic guess, published the instant a step is sent
  and corrected only a round trip later — straight into `Cutaway::at` every
  frame. Deliberate for the body's own drawn position (`docs/camera.md`'s
  "follow the prediction"), but roof visibility flipping on an unconfirmed
  guess was never weighed as its own question; a held direction retried
  against a wall (`Steering::detour`, above) made it visible — a building's
  roof popped for one frame on every retry, for as long as the direction was
  held. Fixed with a second field, `App::cutaway_at`, which follows a
  prediction only after the client's static map and live item layer agree it
  is a legal step, and is snapped outright on a correction. The same guard is
  applied when the prediction reaches the window, rather than waiting for its
  ACK: a real step round a building therefore cannot spend a round trip under
  the previous tile's cutaway, while a known-doomed step still cannot pop a
  roof.

## Backlog, found while giving the client the shard's obstacles

- ~~**The client's terrain saw walls and nothing else.**~~ `MapTerrain` reads the
  map and `tiledata.mul`, so a wall stopped a step here and a *barrel* did not:
  a placed item is an entity the shard described in a `0x1A`, and nothing on
  this end laid it over the map. `Steering::detour` therefore offered no way
  round one, the `0x02` went out, the shard refused it with a `0x21`, and a held
  direction shuddered against the crate — the corner-rule bug one layer down.
  Fixed with `crates/client/app/src/clutter.rs`: `Clutter`, a third projection
  of the `WorldView` beside `App::items` and `App::others`, and `Cluttered`, the
  client's twin of `openshard-state`'s `LiveTerrain`. Same predicate
  (`Terrain::item_blocks`) and same z-span (base z, tiledata height) as
  `decor::place_decoration` uses server-side, so the two ends agree by
  construction. Every step decision on this end goes through it — the held
  direction, the mouse heading, `find_path`, and the `cutaway_at` guard.
- **The z-span overlap test now exists twice**, in `Obstructions::blocker_at_z`
  and in `Clutter::blocked_at`, with `MOBILE_HEIGHT` written out on both ends.
  It is the same arithmetic on the same units and it decides whether two ends of
  a wire agree; it belongs in `common/movement` as one function the server's
  index and the client's both call.
- **`Cluttered::sight_clear` still delegates to the map alone**, where the server
  treats a shut door as opaque. The premise this was filed under — "a client
  cannot tell a shut door from a barrel", the wire carrying a graphic and not a
  kind — no longer holds: `clutter.rs` marks its blockers `door` off
  `client/render`'s table, so the missing half of the server's rule is now one
  `is_some_and` away. Still deliberately not drawn: nothing here computes line of
  sight for gameplay, and a rule with no reader is one nobody notices going
  wrong. It stops being harmless the moment something does.
- **Neither end blocks a step on a mobile.** Nothing registers a body in
  `Obstructions`, so `Clutter` deliberately holds none either — two ends wrong
  the same way, which walks, rather than a client refusing steps the shard
  allows. Whether a mobile should block at all is a gameplay decision (ServUO's
  `checkMobiles`) and belongs in the movement rules, not in either index.
- **A placed item that is `Surface` but not `Impassable` blocks nothing here,
  and ServUO weighs it.** Both ends filter placements through
  `Terrain::item_blocks`, which is `Impassable` alone; the reference's movement
  gathers items on `Impassable | Surface` (`Scripts/Services/Pathing/Movement.cs`)
  because a placed floor or table is a surface a body stands on *and* a body in
  the way of one standing lower. Nothing has needed it yet — decoration is
  overwhelmingly walls and furniture that carry `Impassable` — but a shard that
  places a raised platform as an item will find it walkable from underneath.
  When it is fixed it has to be fixed on both ends in one commit, or the two
  disagree and the walk rubber-bands.
- **Multis are not items and are not here.** A house is not a `0x1A`, so the
  moment multis land this end will walk into their walls exactly the way it
  walked into barrels. Whatever indexes them for drawing should feed `Clutter`
  in the same pass.

## Backlog, found while separating the world's markers from the UI

- ~~**The tile markers were drawn over egui.**~~ The hover, the selection and
  the walk goal were painted into an `Order::Foreground` layer with no clip, so
  a diamond under the cursor was drawn *on top of* the panel the cursor was
  hovering — the world leaking onto the UI. Fixed with one
  `shell::world_painter`: `Order::Background`, which puts them under the
  windows where a thing lying on the ground belongs, and clipped to the world's
  own viewport rect, which is what keeps them off the docked panels (layers
  inside one order are painted in creation order, and the panels' background
  layer already exists, so the order alone does not do it).
- ~~**The overlay's rect was read in the middle of the layout.**~~ It was taken
  after the status panel and *before* the speech strip claimed its edge, so it
  named a rectangle the world is not drawn in and the markers were clipped to
  the strip's rows as well. Read at the foot of `shell::layout` now, which is
  the same rect `Shell::run` hands the camera a moment later.
- ~~**The hover was inferred from an absence of events.**~~ A cursor over a
  panel has its `CursorMoved` consumed by egui, so the world's idea of where it
  is simply stopped updating and the highlight froze at the panel's edge instead
  of going out — behaviour that looked deliberate and was not. The positive
  question is asked once a frame now (`Shell::holds_pointer`, plus
  `App::pointer_inside` for the cursor leaving the window entirely, which no
  egui state can answer) and `App::hud` picks no tile unless the world owns the
  pointer.
- **The markers are still egui shapes, not world geometry.** They are drawn
  after the world pass and therefore over everything in it: a highlight on a
  tile a mobile is standing on is drawn over the mobile, where the ground it is
  lying on is behind them. Correct is a quad in the world pass with the ground's
  own depth. It has not mattered yet because a one-tile diamond under a body is
  mostly hidden anyway, and the terrain overlay below makes it matter more.
- **The terrain overlay is thousands of egui polygons a frame.** `App::terrain_overlay`
  asks `spawn_z`/`can_fit` of every tile in view and `shell::draw_terrain` emits
  a filled diamond for each — 7,600 of them at 1600x1000, about 2ms of frame
  build. Fine for a debug toggle that is off by default, and the wrong shape the
  moment anything wants it on permanently: it is one instanced quad draw with a
  per-tile colour, in the same pass as the markers above.
- **`Cluttered` does not clutter `stand_z` or `spawn_z`.** Both delegate
  straight to the map; only `can_step` and `can_fit` consult the index. That is
  why `terrain_overlay` has to ask the two of them together to get an answer the
  walk agrees with, and a caller that asks `stand_z` alone gets a surface with a
  barrel standing on it. Either the two should consult the index or the trait
  should say plainly that they answer for the map only.

## Backlog, found while giving the client the double-click

- **Only ground items are pickable.** `items::pick` walks `App::items` and
  nothing else, so the map's own statics and every mobile are invisible to a
  click. A static has no serial and never will — it is not an entity — but a
  mobile does, and double-clicking one is the *paperdoll* arm of `0x06`
  (`DoubleClick::interpret`), waiting on a paperdoll to show. The picking rule
  itself is the same one: the topmost opaque texel, over one more list.
- **The click rebuilds the cutaway; the highlight does not.** Inside `draw` the
  pick uses the frame's own `Cutaway`, camera and cursor. `use_under_cursor`,
  reached from the event loop, has none of those to hand and calls `Cutaway::at`
  again against a camera it reads back from `self.control`. Cheap at click rates
  and wrong in shape — the frame's state is one thing and the click asks three
  parts of it separately — and it is one camera-move away from the click using a
  picture the player never saw. The fix is the frame keeping what it picked
  against, which is also what a "what am I pointing at" line in the HUD wants.
- **The double-click knows nothing about what it clicked, and that is still
  right.** It goes out for whatever was under the cursor and the shard decides.
  What it costs is feedback — no cursor change over something usable, and a click
  on scenery is silence rather than "you cannot reach that"; the `0x1A` that
  comes back is the only signal, and only when the shard did something. One
  ingredient has since arrived: a door *is* now nameable on this end
  (`clutter.rs`, off `client/render`'s table), so "the cursor says door" is
  available whenever somebody wants to spend it. Everything else usable — a
  container, a corpse, a lever — still is not.
- **The pairing is time alone, with no gesture layer.** `App::last_click` is two
  fields and a comparison in the `MouseInput` arm. It matches the reference and
  it is enough for one button, but single click (`0x09`) is the same event
  *without* a pair — it has to wait out the window before it can fire, which is
  a timer this end does not have. Whoever adds `0x09` writes the small state
  machine both go through rather than a second timestamp beside this one.
- **There is no end-to-end test that a door opens.** The chain is covered in
  three places — encode, decode, pick — and joined nowhere: a shard spawned by
  `crates/e2e/shard` has no doors in it, because doors come from the community
  pack's `op_generate_doors` and the engine holds no decoration data of its own.
  The test that would be worth having places one item the client can name and
  then double-clicks it, which needs the e2e shard to be able to put something
  in the world without a pack.

## Backlog, found while chasing a player sprite that disappears

- **`Cutaway::at`'s second loop read the wrong mask, and it is fixed.** The
  client's `0x204` is Surface and *Transparent*; the port had Surface and
  *Climbable*. A roof a player can walk onto carries Climbable, and reading
  that bit into the mask stops such a roof from ever being cut — the roof stays
  drawn over whoever just climbed onto it. No roof static in the Britain block
  carries Climbable, so nothing observable changed there, but the bit is the
  kind that is only ever wrong on the one map somebody plays. Both masks are
  now pinned by `the_two_cutaway_masks_are_the_flags_they_are_named_after`.
- **Nothing ties the tile the cutaway was taken from to the body it hides.**
  `Cutaway::at(self.cutaway_at, ..)` and `shows_mobile(mobile.at.z)` still
  receive separate values. `cutaway_at` now follows each locally legal player
  prediction, which removes the ordinary one-step drift around a building;
  it intentionally stays put for a move the local terrain refuses. The hard
  guarantee is still absent: the type should carry the point it was computed
  from, so "hide the body this was taken for" is not expressible rather than
  merely covered by `a_cutaway_never_hides_the_player_it_was_taken_from`.
- **The cut is hard where the client fades.** Deliberate (see the module's own
  "What is deliberately absent"), and the cost is worth naming beside it: the
  client walks alpha down 25 a frame, so a `_maxZ` that is wrong for one frame
  is invisible there and is a whole-body blink here. Every transient this
  module can have is maximally visible until something blends.
- **`AnimAtlas` counts frames one way and looks them up another.**
  `AnimAtlas::grow` skips a blank frame but keeps the file's frame index, so a
  blank in the middle leaves a hole; `frame_count` counts what is *packed*, and
  the caller picks `ticks % count` — an index that can land in the hole and
  addresses nothing past it. A body would vanish for one frame delay every
  cycle. Not reachable in this install (350 animations of bodies 400, 401, 605
  and 606 hold no blank frame), and cheap to make unrepresentable: pack a blank
  as a zero-area region, or count by the file's frame count rather than by the
  map's length.
- **The atlas is grown for one animation group and drawn from another.**
  `wanted_in` reads `self.player.group` — the group as of the last packet —
  while the loop below the pack overwrites `mobile.group` from
  `Crowd::group_for`, which changes on the crowd's own clock. A live group the
  pack never asked for has `frame_count == 0`, and `mobiles::collect` then
  drops the body *and its equipment* with no diagnostic at all. Today every
  group a body reaches has also been the snapshot group at some packet, so the
  hole does not open; the ordering is an accident and not an invariant, and the
  silent drop is what makes it undiagnosable when it does.

- **And the sprite really did disappear, for none of the reasons above: the
  player had died.** A dead character relogs a ghost, so the shard names body
  `0x0192`, and `anim.mul` has no index block for `0x0192` or `0x0193` at all —
  the client's `Mobile.GetGraphicForAnimation` reads the living body two below
  it, and this end had no port of that function. `anim::animation_body` is that
  port now, applied where the atlas is packed and where the frame is looked up,
  and `the_ghost_bodies_are_in_no_index_block_and_the_bodies_they_map_to_are`
  asserts against the shipped files that the remap is necessary rather than
  decorative. What is *not* done: a ghost is drawn as a solid living body. The
  client draws it translucent, and until it does here a ghost and a living
  player are the same picture.
- **A standing body is one frame, and nothing plays the fidgets.** Group 4
  (`PeopleAnimationGroup.Stand`) holds exactly one frame for bodies 400 and 401
  in this install — a body that is standing is *supposed* to be still — and what
  makes the client look alive is the fidget groups beside it (5 and 6, five
  frames each), played now and then off a timer. `Crowd` has no notion of them:
  it holds standing, walking and running, so a character that is not moving is a
  frozen picture and reads as "the animation is broken" whether or not it is.
- **A layer with no `AnimID` used to draw a monster, and now draws nothing.**
  Zero is the absence of an id — a backpack or a ring shows on a paperdoll and
  never on a walking body, which is why `MobileView.Draw` guards every layer
  with `if (item.ItemData.AnimID != 0)`. Unguarded the zero is not inert: body 0
  is the first monster in `anim.mul`, so the layer packed and drew *its* frame
  under the wearer and followed them around. `worn_graphic` is the one place
  that answers "what does this layer draw with", for the atlas and for the quad
  alike, and it answers `None` here.
- ~~**The shard's ghost is dressed correctly, and the layer this end cannot tell
  apart is the hair.**~~ Fixed: `EquipmentLayer` carries the wire's `Layer`, and
  `worn_graphic` answers `None` for hair and a beard on a ghost body — one place,
  so the atlas and the quad cannot disagree about it. What was found:
  checked rather than assumed, against the save, a relogged
  ghost wears exactly three things — a death shroud (`0x204E`), its backpack
  (which is where the zero `AnimID` came from; a backpack stays on the dead in
  UO too) and its hair. The reference draws the first two and skips the last,
  `IsDead && (layer == Layer.Hair || layer == Layer.Beard)`, so a ghost is
  bald under its hood. This end could not make that decision at all:
  [`EquipmentLayer`](../../../crates/client/render/src/mobiles.rs) carried a graphic
  and a hue and *not* the layer it was worn on, so "hair" was not a question the
  renderer could ask — and it is the same field a real paperdoll ordering needs,
  which is why the fix landed with the paperdoll's wire half.
- **The silent drop cost a third hunt, so it should stop being silent.** Twice
  above this is named as a hazard and once below as an accident; this time it
  was the whole defect, and from outside it is indistinguishable from the
  cutaway hiding the body, from the atlas missing a group, and from the mobile
  never arriving. `mobiles::collect` should count what it dropped and why, and
  the shell's World panel should show it — a body the shard sent and this end
  did not draw is a fact the client already knows and refuses to say.

## Backlog, found while giving the client firelight

The pass itself: `client/render/src/light.rs` collects the flames a frame can
see, `blit.wgsl` applies them on the way to the surface, and F10 toggles night.
The shape is one point light per burning graphic, multiplied over the finished
world image — the argument for that arrangement is `light.rs`'s own header. What
was found and not done:

- **`light.mul` and `lightidx.mul` are not read at all.** They are what the
  client draws light *from*: a per-source sprite, keyed by an id in the tiledata
  entry, rather than a radius and a colour somebody chose. `light::flame` is the
  stand-in — one warm default and a wider one for a campfire, matched on the
  graphic — and it is the only invention in the pass. Reading the two files
  replaces that function and nothing above it.
- **Fixed: a pool used to pop out at the frame's edge.** `light::collect` walked
  `Camera::visible_tiles` — the tiles whose *sprites* can land in the image —
  and a pool reaches nine tiles past the thing making it, so a lamp's light
  vanished the instant the lamp left the screen. Measured on Britain at the
  widest zoom: 88 light sources stood in the band being skipped. `lit_tiles`
  now grows the rectangle by the widest flame's reach, and
  `every_flame_that_can_reach_the_frame_is_walked` states the implication rather
  than the margin, so a wider flame added later cannot reintroduce it.
- **Done: walls stop light, and the whole pass moved into world coordinates to
  let them.** [`lighting.md`](../../archive/render/lighting.md) is the plan and the argument; the
  short of it is that the screen-space shadow sketched here cannot work. A
  wall's sprite stands 44 pixels above the tile it occludes from, so any mask
  drawn over the ground behind it also covers the wall's own lit face — the two
  are the same pixels of the same sprite. The three world passes now write which
  tile each pixel came from (`client/render/src/place.rs`), the flames are tiles
  with a reach in tiles, and the blit walks a grid of occluders
  (`client/render/src/occlusion.rs`) between a fragment and each flame. What
  occludes is `WINDOW | NO_SHOOT` — ServUO's own line-of-sight rule — and never
  `BLOCK`, or every crate on the street would cast a shadow.
- **Done with the same attachment: a flame no longer lights through a floor.**
  The channel this asked for is `place.rs`'s, the decision is
  [`lighting.md`](../../archive/render/lighting.md)'s, and the distance is now three-dimensional with
  eleven `z` units to the tile — so a cellar's brazier is as far from the street
  above it as it would be eleven tiles away.
- **Nothing a mobile carries burns.** A player holding a torch is the commonest
  light in the game and this pass cannot see one: `light::collect` walks the map's
  statics and the ground items, and equipment is neither. It needs the layer the
  torch is worn on and the graphic under it, both of which `mobiles` already has.
- **The ambient is a key, not a clock.** There is no time of day on the wire, so
  night is `App::night` and F10. When the shard grows one, it writes to that
  field and the rest is already a colour per frame.
- **The falloff eats the outer half of the radius.** `(1 - d)²` is a quarter of
  its peak at half the radius and under a byte's worth by 0.9, so a pool set to
  six tiles reads as about three. That is the look the reference isometrics have
  and it is why the numbers in `light.rs` are as large as they are — worth
  remembering before anyone "fixes" a radius that looks too big in the source.
- **A light is placed by its tile, not by its sprite.** `FLAME_LIFT` is a flat
  half-tile of `z` above the tile it stands on, because the sprite's height is
  in the atlas and the atlas is not a parameter here. A wall sconce's flame is
  higher than that and a candle's is lower; both are a fraction of a pool six
  tiles across, which is why it is a constant and not a lookup — but it is a
  constant standing in for a measurement.

## Backlog, found while giving the mouse's heading a dead zone

The fix itself is `DEAD_ZONE` in `client/app/src/lib.rs`: a cursor within 10
world pixels of the body's drawn pixel names no heading, so a right button held
with the mouse sitting over the character stands still instead of walking off in
whichever of the eight sectors the last pixel of hand tremor landed in.

Ten is a *play* number and not the geometry's. The geometry asks for
`22 / cos 22.5° ≈ 23.8` — below that a step can carry the body past the cursor
and leave it asking for the way back — and
`a_step_stops_overshooting_further_out_than_the_dead_zone` derives that bound
from the projection and asserts the constant is inside it, so the trade stays
visible rather than becoming a number nobody remembers choosing. The trade: a
zone half a tile wide is a hole a player can feel, the character stopping well
before the cursor looks like it is off him; the overshoot is bounded at one tile
and the next ask corrects it, the jitter is unbounded. So the radius kills the
jitter and no more. What was found on the way and left undone:

- **The heading is only recomputed when the *mouse* moves, never when the body
  does.** `App::walk_toward_cursor` runs from `CursorMoved` and from the press;
  after that `Steering::due` repeats the stored `Heading` every step's length
  with nothing re-reading where the body now is. With the camera locked to the
  body it happens not to matter — the cursor's world pixel travels with the eye,
  so the bearing is genuinely unchanged — but with the eye unlocked (`Home`) the
  body walks toward a cursor standing still in the world, arrives, walks past it,
  and keeps going: the dead zone it is now inside is never asked about. The
  heading is a function of two positions and one of them is moving, so it wants
  re-deriving in `App::about_to_wait` next to the `due` loop, not only in the
  input event.
- **The dead zone is in world pixels, so it is a fixed fraction of a tile and a
  varying number of screen pixels.** That is the defensible choice for the
  overshoot argument (the projection is what makes a step 44 pixels long), but
  the argument the radius is actually *set* by is the jitter one, and that is
  about the hand and the screen: at a far zoom 10 world pixels is a patch of
  screen a tremor crosses easily. If that shows up in play the two halves want
  separating — a world-pixel floor for the overshoot and a screen-pixel floor
  for the noise, whichever is larger.
- **The overshoot has never been seen, because the recompute above is missing.**
  The two findings are one: with the heading only re-derived on a mouse event, a
  step that carries the body past the cursor is not answered by a step back, and
  the small radius costs nothing today. Closing the recompute is what makes the
  10-against-24 gap live, and is where the constant gets re-argued rather than
  assumed to have survived. **Closed by `TURN_ZONE`** (see the turn-ring section
  below): the whole 10-to-24 band answers a cursor with a facing and no ground,
  so there is no step to overshoot with. The recompute is still missing, and is
  still worth doing for the unlocked camera.
- **The reference has no dead zone here at all.** ClassicUO's
  `MoveCharacterByMouseInput` (`Game/Scenes/GameSceneInputHandler.cs`) measures
  from the viewport's centre and walks on any non-zero offset; `mouseRange >= 190`
  is the only radius in it, and it chooses *run* rather than whether to move.
  What saves it is that its origin is the centre of the screen and the body is
  drawn there, so "the cursor is on the character" is where the offset is
  genuinely zero — our origin is the body's own projected pixel, which is the
  right origin for a free camera and is a pixel or two away from where the
  sprite's feet look like they are. The reference is worth re-reading if the
  radius ever needs defending, not copying.

## Backlog, found while chasing a creature drawn as a pair of feet

A player who had died reported an NPC following him that was drawn as **walking
feet and nothing else**, and that would not answer the mouse. It behaved like a
creature — it hunted, it pathed, it was not repeating the player's steps — so it
read as one broken sprite. It was three separate things, and only one of them
was a defect in this repository:

- The body was `480`. That is not a creature at all: it is the `AnimID` of
  `0x170F "shoes"` in `tiledata.mul`, an *equipment* animation, and `anim.mul`
  holds a real block for it. The client drew exactly what it was told to.
- The id came from the Community Pack's ServUO converter, whose `resolveBody`
  read the first `new int[]` in a class as a body table. In every
  `BaseCollectionMobile` the first array is `int[] hues`, so `Enrico the thief`
  was converted to body `0x1E0`. Fixed there, with the rule gated on `BaseMount`
  and a test pinning each class shape.
- **It could not be clicked because no mobile can be** — see the M5 section
  above. That is the finding worth keeping: an entity that is drawn wrong and an
  entity that cannot be picked are indistinguishable from the player's chair,
  and the second one was invisible for as long as it was because it never had a
  symptom of its own.

What was found on the way and left undone:

- **This client reads `anim.mul` and nothing else.** `Body.def` redirects are
  now applied to the render snapshot before it asks the atlas for frames (for
  example, a grey wolf's body 25 becomes body 225 under hue 946). `Bodyconv.def`
  is still missing: everything drawn since LBR lives in `anim2.mul` through
  `anim5.mul`, and which file holds a body is a lookup there (`752 29 -1 -1 -1`
  — body 752 is index 29 of `anim2`). So **every body that lives only in a
  later file still draws nothing at all** — on the Felucca spawn set that is
  bodies 752 and 764–794 among others, tens of spawn points, and each one is a
  creature that hits a player from an empty tile. This is a file-reader gap,
  not a renderer change.
- **A modern install ships `AnimationFrame*.uop` too**, keyed by a hash rather
  than by the index arithmetic, and ClassicUO prefers it to the `.mul`. Reading
  the def redirects closes most of the gap on a legacy install; the uop is what
  closes it on the install people actually have.
- **A mobile whose body frame is missing still draws its equipment.**
  `mobiles::collect` drops the body quad when the atlas has no frame — the right
  call on its own — and then walks the layers, which are packed under their own
  graphics and draw fine. The result is a floating hat, or a pair of boots,
  walking about with no wearer: the same picture as the bug above, from a
  different cause, and there is no test that says which of the two a person is
  looking at. A body that draws nothing should take its layers with it, and say
  so once in the log.
- **Nothing on the shard ever asks whether a body id is a body.** The pack's
  converter now does, at generation time, which is the right place for data that
  ships. It is not the only door: `op_spawn_mobile` takes a number from a script,
  and a typo there arrives in the world exactly as this one did.

## Backlog, found while giving the dev HUD a memory

What was found on the way and left undone:

- **The gump windows are not remembered.** `gump::Windows` places a shard's
  dialogs at egui defaults every run, and the same file could hold them — but a
  gump is the *server's* window and keyed by a serial that does not survive a
  logout, so what a saved position is keyed by is a real question and not a
  field. It waits for M4, which decides whether gumps are egui at all.
- **Nothing is written until the client exits cleanly.** A crash or a kill loses
  the layout. A debounced save on change is the fix and costs a timer; a file
  written every frame is not.
- **The world's TTF text does not follow the HUD's zoom, by design** —
  `TtfAtlas` bakes one pixel size at startup from the monitor's `scale_factor`.
  That is the right split today (the HUD's scale is not the world's), but if the
  world ever wants a zoom of its own it is an atlas rebuild and not a parameter.
- **`shell.rs` is 1.8k lines** and the panels are now cleanly separable — one
  function per tab, plus `overlays`. A `shell/` module with a file per tab is the
  obvious next split, and the file is close enough to the ~2k line rule that it
  should happen before the next panel is added.

## Backlog, found while giving the turn its own delay

Turning is a whole step in UO, and this client used to charge nothing for one:
`Steering::charge` treated a direction change as free and `App::about_to_wait`
looped twice on purpose, so the turn and the step it preceded left in the same
wake and a player never saw the body square up. The reference does the opposite
— `PlayerMobile.Walk` leaves `x`/`y`/`z` alone when the direction asked for is
not the one the body faces and charges `MovementSpeed.TurnDelay`
(`Constants.TURN_DELAY = 80`, `TURN_DELAY_FAST = 45`) — so a click sideways
turns first and covers ground a beat later. `steer::Turning` is that setting,
defaulting to the reference's, with `Turning::Immediate` keeping the old
behaviour. What was found on the way and left undone:

- **The walk oracle in `dst.rs` does not model the turn.** The harness pins
  `Turning::Immediate` on its `Steering` so its constant-velocity oracle stays
  the truth, which means the *shipped* default's timing is only covered by unit
  tests in `steer.rs`. That is a real hole if the turn ever interacts with
  latency or wake jitter — a turn charged 80ms while a `0x21` is in flight is
  exactly the sort of thing the DST exists to find. Closing it is a turn tax
  term in `Oracle::build`, one knot of standing still per direction change, and
  a scenario that turns under latency.
- **`OPENSHARD_TURN` is a stopgap, like `set_leeway` before it.** Two settings
  now exist that are a player's taste rather than a rule, both set in one place
  in `lib.rs`, and neither has a config file to come from. That is the shape of
  a client config: the walk's preferences want to arrive as a struct read once
  at startup, not as one environment variable per question.
- **The reference's adaptive coalescing is not ported.** ClassicUO suppresses
  the direction-only packet entirely once `Walker.UnacceptedPacketsCount >= 3`
  — it turns the body locally and sends nothing, so a congested link stops
  spending its budget on turns. Ours always sends the turn. Worth having when
  there is a real link to congest; our own `Walk` already counts what would be
  needed.

## The turn ring, and where it is not the reference's

`TURN_ZONE` in `client/app/src/lib.rs`: between `DEAD_ZONE` and it, a held right
button asks the body to *face* the cursor and covers no ground — `steer::Ask`
carries which of the two the cursor is asking for, and `Steering::take` answers
`Ask::Turn` with one `0x02` while the body is not facing that way and with
nothing at all once it is.

It is the stock 2D client's ring and **not ClassicUO's**, which is worth being
explicit about since everything else about the walk here is argued from CU:
`MoveCharacterByMouseInput` walks on any non-zero offset from the viewport's
centre, and its one radius (`mouseRange >= 190`) chooses running rather than
whether to move. So a mouse in CU cannot turn a character on the spot at all.
The reason to have it anyway is that a player needs to face a door, or face
whoever they are talking to, and every other ask a cursor makes also sets the
body walking.

The radius is the overshoot bound and not a taste — `22 / cos 22.5° ≈ 23.8`
world pixels, where a step stops ending past the cursor that asked for it — so
the band where walking is the wrong answer is exactly the band where the body
turns instead. `a_step_stops_overshooting_further_out_than_the_dead_zone` pins
that the ring reaches it. What was found on the way and left undone:

- **The ring has no cursor of its own.** The classic client tells the player
  which zone they are in by changing the cursor graphic; ours looks identical
  either side of the ring, so "why is my character not moving" is answered only
  by moving the mouse further. The art is in the client's files
  (`Art`/`Gumps` cursors) and the zone is already computed in one place.
- **`Ask` is only the mouse's.** A held arrow key cannot ask for a facing at
  all — the keyboard's own idiom for that in the stock client is a modifier,
  and `keys.rs` has no notion of one. It wants the same enum, not a second
  mechanism.
- **The turn ring and `Turning` are two answers to one player question.** One
  says what a turn costs, the other where a turn is all that is asked for; both
  are set from `lib.rs` and neither has a config file. When the client config
  lands they belong in the same struct as `Leeway`.

## Backlog, found while drawing the paperdoll

- **`MobileView.IsCovered` needs the item's wire graphic**, and nothing carries
  it this far. `crowd::worn` resolves each worn item to its `AnimID` out of
  tiledata and throws the wire graphic away; every arm of `IsCovered` keys on
  that graphic (`0x3DC0` sticking out below robe `0x3CAC`, tunic `0x1541`,
  pants `0xAEB1`) and only some arms on the `AnimID`. So the fix is one more
  field on `EquipmentLayer` and then the port — not a smaller table. Until then
  a garment that should be hidden under a closed robe is drawn poking out.
- **The in-world order is still the wire's**, and the same table would serve it:
  `PaperdollOrder.BuildInWorld` is `Build` plus `ApplyDirectionCloak`, one rule
  that puts the cloak on top when a body faces away and behind when it faces the
  viewer. `mobiles::push_quads` pushes layers in wire order today. Cheap now
  that `paperdoll::order` exists and `EquipmentLayer` carries its layer.
- **No tooltips, no lifting.** The buttons are done — decision 8 in M4: they
  press, they release over the same picture, and seven of them send a packet.
  What the `0x88`'s flag byte still buys nothing is the *lifting*:
  `view::Paperdoll::can_lift` is read off `PaperdollFlags::CAN_LIFT` and
  nothing asks it, because dragging a worn item off the doll is a `0x07`/`0x13`
  pair this client does not send from any window yet. That flag is the shard's
  permission and the first thing that arm should consult.
- **A window has no memory.** Both kinds cascade from a fixed corner and are
  forgotten when they close; the reference client remembers a per-container and
  per-paperdoll position across sessions. `desk.rs` already persists the
  window's own frame and is where this belongs.
- **`is_partial_hue` is not modelled.** The reference draws the body and most
  equipment with `IsPartialHue`, which retints only the art's grey pixels;
  `gump::Picture` has one hue and applies it whole. Nothing looks wrong on the
  bodies this shard sends because they arrive with `Hue::NONE`, and the first
  dyed robe will show it.
- **A paperdoll of a mobile the client has never seen draws an empty frame.**
  The window is in the list, the `0x88` named the mobile, and `WorldView::mobiles`
  has no entry — which happens when a shard opens a paperdoll for a body it has
  not revealed. The frame is drawn and the doll is not, so the window can be
  moved and closed while it waits; what is still not decided is whether it
  should *ask* for the body.
- ~~**The frame carries nothing a frame is for.**~~ It carries the name — the
  `0x88`'s 60-byte string on the plate at `(39, 262)`, decision 6 — and eleven
  working pictures, decision 8. What is still missing is the **minimise**
  (`0x07EE`, the reference's collapsed frame, and a second window state nothing
  here models) and the **equipment slots down the side**, which are a modern
  client's and want the same lifting the entry above is about.

## Backlog, found while wiring the paperdoll's buttons

- **A packet the shard sends and the client has no arm for is invisible.**
  `ServerPacket::decode`'s list is shorter than the list of things
  `ServerPacket` can *encode*, and the gap answers `Ok(None)` — framed, stepped
  over, and silent. That is the right behaviour for a decoder that is still
  growing, and it is also how the `0x72` answering the war toggle went missing
  for as long as the toggle existed: every unit test on either side passed. The
  two lists are in one file and nothing compares them. A test that walks the
  server's send-side length table and names the ids with no decoder would not
  fail — it would *report*, which is what makes a growing gap visible instead of
  quiet.
- **`StatusFlags`' bits are still unmodelled, and a body's stance now has a
  reader.** `WorldView::player.war` is our own character's, off the `0x72`.
  Everybody *else*'s war stance is in the flag byte of their `0x77`/`0x78`
  (`Mobile::flags`) and nothing reads it — see the type's own docs, which say
  outright that the bits are a guess nobody has made yet. It costs nothing
  today, because no stranger's frame draws a toggle; it starts costing the day
  M5 draws a body standing in a war stance, which is the same fact for our own
  body and for theirs.
- **Three gestures now say "press, hold, release over the same thing".**
  `Dialogs::press`/`release` for a dialog's buttons and switches, the doll's
  `held_doll`, and the world's own double-click pair. They are not the same code
  and two of them are not the same *shape* — a dialog's hold is keyed by gump
  id, a doll's by window and button — so this is an observation and not a
  design: a fourth would make the pattern worth extracting, and until then a
  shared "gesture" type would be an abstraction over two examples.
- ~~**The status bar and the skill list are the next two windows.**~~ Both are
  built. The Status button sends `0x34`; `WorldView::apply` folds its `0x11`
  into `view::Status` on the player, while the one `Player::hits` field remains
  the shared answer for the status window and the overhead health line. A local
  `WindowSubject::Status` opens only on the button press — never merely because
  the login status reply arrived — and draws the classic `0x0802` frame with
  the values the shard stated.

## Backlog, found while drawing the skill window

- ~~**A stat change moves every skill's value and no `0x3A` follows.**~~ Fixed,
  and it was the server's half exactly as this said. `apply_stats` now takes all
  fifty-eight *drawn* values before the stats move and announces every one that
  differs afterwards — the difference itself, rather than a rule about which
  skills the changed stat lends to, because that rule is the same table read
  plus something to get wrong. `set_skill` was silent for the same reason and
  emits too. See `SkillChanged`'s own docs for why those events carry `previous`
  equal to `value`: the trained number did not move, and saying so is what keeps
  "your skill has increased" quiet for a change that is not a gain.
- ~~**The lock arrows are drawn and answer nothing, and no skill can be used
  from the window.**~~ Fixed. Both packets already existed and the shard
  already decoded both — what was missing was entirely client-side:
  `SkillLockRequest::encode`/`UseSkillRequest::encode`, a `client/net::skill`
  to hold them, and two more `skills::Hit` variants on the same single-click
  furniture pipeline the heading arrow and the scrollbar already use. The lock
  arrow is the one deliberate exception to "wait for the shard's answer" — see
  `skills::Tree::lock_of`'s doc — because ServUO's own client redraws it on the
  click and `World::set_skill_lock` sends nothing back by design. The use
  button is drawn only for a skill the files mark `has_action`, `SkillItemControl`'s
  own gate, ported as-is. `crates/e2e/shard/tests/skill_window.rs` gates both:
  a lock click reflected in the next full list, and a passive skill's use
  press answered with cliloc 500014.
- **Found while gating the use button: `0xC1` had no client-side decoder at
  all.** `LocalizedMessage` — the packet `use_skill_button`'s "cannot be used
  directly" line travels on, and every other gate through
  `WorldState::localized_message` — had `EncodePacket` and no `DecodePacket`,
  and `ServerPacket::decode` had no arm for it: a real client asking for one
  read it as `Unknown` and dropped it silently, forever, since nothing had
  ever sent one over a real socket in a test to catch the gap. Fixed in
  `crates/common/protocol/src/speech.rs`, the same shape `SpokenMessage`'s
  decoder already has — sentinel-folded `serial`/`graphic`, little-endian
  `arguments`. **Still open:** nothing in `WorldView::apply` does anything
  with a decoded one yet, so it is readable now and still invisible to a
  player — the e2e test above only proves the packet arrives, not that this
  client's chat line or journal draws it.
- **`Standing::cap` is read off the wire and drawn nowhere**, which is
  `Paperdoll::can_lift`'s trap again: the reference has a checkbox that swaps
  the value column for the base or the cap, and until something like it exists
  the field has no reader.
- **A `0x3A` of type `0x01` or `0x03` means "open the window" as well**, and
  this client does not take it: its window opens when the player presses the
  button. Nothing sends one today — this engine's own shard sends `0x00`/`0x02`
  — so it costs nothing until a shard that does is on the other end.
- **The shard's own skill-name table (`0x3A` type `0xFE`) is refused by name.**
  It is how an emulator ships skills the client's `skills.mul` has never heard
  of, and reading it would mean the name table is a *view* rather than a file
  read once at startup. Refused rather than skipped, so the day one arrives it
  says so instead of drawing the wrong names.
- ~~**`SkillUpdate` has no gate on a live wire.**~~ Gated. The blocker was that
  making a skill move meant training one and a gain is a dice roll; `.skill
  <name> <value>` is the staff command this asked for, and
  `a_skill_the_shard_moves_arrives_as_one_line` drives the whole path — say it,
  read the delta, find the new value in the table a window draws from. Worth its
  own case rather than a line in the full-list one: the two `0x3A` bodies share
  an id, and the delta is the only one of the pair whose skill ids ride **raw**
  rather than one-based, so a client applying the full list's numbering to a
  delta would move the wrong bar and look correct from either side alone.
- **Nothing stops a second `GumpRenderer::render` in a frame.** The rule is
  written on the function now, and the app obeys it by collecting every line of
  gump-space text into one list. It is still a rule and not a type: a third
  caller would silently take the second's quads, which is exactly the shape the
  bug had. The fix, if it is worth one, is a renderer that appends into its own
  buffer and draws once at the end of the frame.
- **The window has no memory and no minimise**, which the paperdoll's backlog
  already says for both kinds: the tree — which headings are shut, where the
  list is scrolled to — is thrown away when the window closes, and the reference
  remembers both across sessions. `desk.rs` is where that belongs.
- **The scrollbar is the client's first and it belongs to one window.** A
  container with more in it than fits has the same problem and no bar; so does
  the journal. What is reusable is already separate — `Scissor` cuts anything,
  and the thumb's arithmetic is four constants — but "a list that scrolls" is
  not a type yet, and it should not become one until the second caller says what
  shape it wants.
- **The names are `skills.mul`'s English, not the localized ones.** `SKILL30.ENU`
  and its `.KOR`/`.JPN`/`.CHT` kin are a different file this crate does not read,
  and a client installed in another language will draw English rows.

## Backlog, found while giving Escape something to close

- ~~**Escape quit the client.**~~ It closes the topmost of this client's own
  windows now — `App::close_top_window`, the same door the right button goes
  through (`App::close_window`) — and quitting is `CloseRequested`, the window
  manager's close box, like every other application. No reference client has
  ever quit on Escape; ours did, and that turned the defect below from an
  annoyance into a window that could not be closed at all.
- 🚩 **egui is painted over this client's gump windows and takes their mouse
  with it.** The gump pass draws into the surface and `Shell::paint` loads over
  it, so a floating egui window stands on top of a paperdoll, a container or the
  skill scroll — and `Shell::on_window_event` claims the click first, so
  `close_window_under_pointer` never hears the right button that would have
  closed it. This is not hypothetical: the dev window opens at `(16, 48)` and is
  360x420, `CONTAINER_ORIGIN` is `(120, 80)` and the skill scroll is 345 wide by
  ~350 tall — the window a player is most likely to open lands *entirely* inside
  the panel that eats its clicks. Escape is a way out, not a fix. The fix is a
  decision nobody has taken: either the gump pass draws after egui and the
  pointer is offered to our windows first (the reference's own order — the
  game's interface is on top and the dev shell is a tool), or the windows
  cascade into the world's viewport, which is a constant answering a layering
  question and would break again the moment a panel moves.
- **A window still has no memory, and now it has one more reason to want one.**
  Both the paperdoll's backlog and the skill window's already ask for this. The
  point Escape adds: with no memory, the *only* place a window can open is the
  cascade — so a bad cascade is not something a player can work around by
  putting the window somewhere sensible once.

## Backlog, found while giving the shard its guilds

The server side of guilds landed with the window it opens (`openshard-guilds`),
and the audit of what this client does with it turned up one defect it had
already had and one whole feature it does not have.

- ~~**Only the last line over a head was drawn.**~~ The crowd held one `Speech`
  per mobile and a second line overwrote the first, so two lines in a row from
  one NPC lost the first — silently, and only when the pair landed inside the
  five-second hold. A single click sends *two* `0x1C`s for one mobile (the guild
  line `[Warlord, OSS]`, then the name — `Mobile.OnSingleClick`'s order), which
  made it every time. Lines stack now, each with its own clock, newest nearest
  the head, bounded at `SPEECH_STACK`.
- ~~**This client reads no `0xD6` at all.**~~ **Built, 2026-08-15** — see
  "Tooltips, and the half that was never written" below.
- **The Guild button was already right.** `DollButton::Guild` sends its `0xD7`
  subcommand `0x28`, and it had been sending it into a server that named the
  subcommand and routed it nowhere. Worth remembering as the shape to keep: the
  client sends what a reference client sends, and the server's stub is the
  server's problem — a packet that never leaves is a defect nobody would look
  for the day the stub is filled.

## Backlog, found while giving the shard guild chat

- ~~**This client cannot send a guild or party line.**~~ **Built** — see
  "The channel selector, and the whole of `0xBF`" below.

