# Plan: many shards, many characters

**Nothing of this is built.** It is written down before the window layer grew
any further because it decides what "the client's state" means: everything
downstream of a socket is per session, and a status bar built against `App`'s
fields rather than against a session is a status bar that has to be written
twice.

One section below describes work that *is* built — the radar reader, its
reduction and its pass — because it was argued here. The raster itself is
[`docs/world/design_radar.md`](../../../docs/world/design_radar.md); what is
still missing is the window's input, named at the end of that section.

## M3b — many shards, many characters

This client holds **sessions, plural**, and a session is a triple: a shard, an
account on it, and a character. The natural shape is therefore `[shard →
{characters}]` and not a flat list — several characters logged in to one shard
is the common case, several shards at once is the same machinery with one more
level, and neither is a special case of the other. A character select to say
which session is on screen, a map of the facet to say where the bodies are, and
the keyboard driving one of them or all of them at once, the way a strategy game
drives a group.

It is written down here, before M4, because it decides what "the client's state"
means. Everything the gump layer builds — a journal, a paperdoll, a container —
belongs to *a* character, and a status bar built against `App`'s fields rather
than against a session is a status bar that has to be written twice.

**A serial is unique on a shard and nowhere else.** `0x2A` on one shard and
`0x2A` on another are two creatures, so every map keyed by `Serial` — `Crowd`'s
clocks, `WorldView`'s mobiles and items — is inside a session or it is wrong.
This is the one mistake here that would not look like a bug: two characters on
two shards standing near each other's serials would simply animate each other.
The atlases are the other way round and are fine, because a graphic id belongs
to the *files*, not to the shard.

### One copy of the files per install, N of everything downstream of a socket

The whole design is that split, and it is not a preference: `App` holds a facet
of a few hundred megabytes plus `Art`, `TexMaps`, `TileData`, `HueRamp` and
`Anim` — 200MB of pictures read before the first frame. Ten sessions holding ten
of those is not a client. So the client's own data is loaded once and shared, and
everything that comes off a socket is per session and shared with nobody:

- **Shared, immutable, `Arc`, and keyed by install.** `WorldMap`, `Art`,
  `TexMaps`, `TileData`, `HueRamp`. The precedent exists — the facet is already
  an `Arc<WorldMap>` handed to the shard thread so `Walk::step` can predict a
  height. `Anim` is the exception and the awkward one: `Anim::frames` takes
  `&mut self` because reading a frame seeks the file, so it is shared behind a
  lock or it is read behind the atlas that already caches what it produced.

  *Keyed by install* is what multi-shard adds, and it is the reason
  `OPENSHARD_CLIENT` cannot stay a single environment variable. A 5.x shard and a
  7.x shard are not read from the same files, and a custom shard ships its own
  map and its own art — `docs/client_versions.md` is the standing rule that
  server and client must read the same `.mul`, and it applies once per shard
  rather than once per process. So the cache key is `(install, facet)`, two
  shards on the same install share everything, and two shards on different
  installs share nothing but the process.
- **Per shard.** The login server address, the relay it hands back, the feature
  mask, the version this client claims, and the `.def`/cliloc set the install
  supplies. **The version is the one that will bite**: it is a startup constant
  today, and every `Feature` gate on both ends follows from it — see "Which
  version we claim to be" below, which stops being one decision and becomes one
  per shard.
- **Per session, and never merged.** The connection, the `WorldView`, the
  `Walk`, the `Crowd`, the eye. `Walk` in particular: the step sequence and the
  fastwalk key are properties of *one* connection, and a shared one would ack the
  wrong session's step and desynchronise every character but one. This is the
  same rule the server lives by from the other side.

### What assumes one today

All of it, and none of it deeply. `App` (`crates/client/app/src/main.rs`) holds
one `link: Option<Link>`, one `crowd: Crowd`, one `control: Control`, one
`player`, one `others`, one `items`, one `view`, one `connection` string. The
window is woken with an `Update` that names no session, because there is only
one to name. Outside the struct the same assumption is in the environment:
`OPENSHARD_CLIENT`, `OPENSHARD_ACCOUNT` and `OPENSHARD_PASSWORD` are one install
and one account, and the shard address and the claimed version are constants in
`main`.

The shape that replaces it:

- A `Shard` — an address, an install, the version claimed to it, and the
  accounts on it. This is the level `[shard → {characters}]` names, and the
  level the file cache is keyed by.
- A `Session` — the link, the last `WorldView`, the crowd, the projections the
  renderer reads, and the account it logged in as. `App` holds a list of them and
  which one is drawn.
- `Update` becomes `(SessionId, Update)`, and `EventLoopProxy` carries the pair.
  A `Lost` is then one session ending, not the client ending — which is already
  what `link.rs` promises ("the window stays open on one of these") and cannot
  currently deliver, because there is nothing left to look at.
- **One runtime, N tasks — not N threads.** `link.rs` argues for a thread
  because the event loop blocks on the compositor and the runtime blocks on the
  socket. That argument buys *one* thread, not one per character: ten idle
  sockets do not want ten current-thread runtimes. The seam stays exactly where
  it is — a thread that is not the event loop — and the connections become tasks
  on it.

### Only what is drawn needs a renderer

The saving that makes the whole thing cheap. A session nobody is looking at
needs its `WorldView` and its `Crowd` — both plain data, both advanced by
packets and a clock — and no GPU state at all. The atlases, the world texture,
the depth buffer and the three passes belong to the *view*, of which there is
one, or two if a split screen ever happens.

This matters because the atlas is the tightest resource here already: it is
rebuilt whole whenever the camera walks off it, WebGL2 guarantees only 2048, and
zooming out was already making that fire more often. N simultaneous worlds would
turn an open question into a wall. N connections against one atlas does not.

### The radar, and the three layers under it

Built to the pixels, and not yet to a window. The reader, the reduction and the
pass are in the tree; `WindowSubject::Minimap` and its input are not, and what
is missing is named at the end of this section.

**`radarcol.mul` is a flat table of `Color16` with no header and no index** —
the offset *is* the key, the first `LAND_TILE_COUNT` are land and everything
after is statics at `LAND_TILE_COUNT + graphic`. Neither reference server reads
it, because both are shards and this is a render file, so the split was
**confirmed against a real install** rather than ported: land `0x03` comes out
green, land `0xA8` blue, static `0x0006` brown and static `0x0751` grey stone,
and no other split produces four colours that are each the colour of the thing.

**The file is not a fixed size.** The canonical table is 163,840 bytes; the
install this was written against is **163,768** — thirty-six entries short, with
no padding and no trailing zeroes. It simply stops. So the reader reads the
length rather than demanding one, the split is a *position* rather than a size,
and an id past the end is absent the same way an id inside it with no colour is.
A reader that insisted on the canonical size would have refused the operator's
own client and been wrong to.

**One pixel per tile** (`render/src/radar.rs`) is a pure function of the map and
that table, which is what lets the player's radar and the facet map below share
it. Three details are each a bug: `WorldMap::statics_at` is keyed by `(y, x)`
and **not** sorted by z, so the highest is compared for rather than taken; the
comparison against the land is `>=` and not `>`, because a floor lies at the
ground's own height and `>` draws grass through marble; and a tile with no
colour is `UNKNOWN` rather than transparent, because zero spells *absent* in
these files and a transparent pixel is a hole in the window. The walk is
block-major so `statics_in_block` is asked once per 8×8 rather than once per
tile — the same cost `map.rs` records as the largest single phase of the
lighting pass when it was asked per tile.

**The radar gets its own pages and its own draws, outside `GumpAtlas`.** The
deciding property is *mutability*, not identity: `GumpArt`'s two arms both name
a picture in a client file and the atlas is a shelf packer for art that never
changes, while a radar is generated. It is also the cheaper way round — the gump
atlas is 2048 square, so a corner of it for a radar would carry sixteen megabytes
to draw a fraction of it. What it has instead is a bounded texture array of
64-tile chunk pages, each uploaded once and kept until it is evicted; a step
selects different pages rather than rewriting a texture.

`radar_chunk.wgsl` has no hue — there is no ramp for a colour that was never in
a client file. **It is validated at pipeline creation and nowhere else**, being
`include_str!`d — so `tests/radar_pass.rs` builds every radar pipeline on a real
device, and that earned itself immediately: the first draft named a uniform field
`target`, which WGSL reserves.

**Nothing about a chunk is in the uniform**, and that earned itself too. The
first version wrote each chunk's placement, source rectangle and page into one
uniform buffer between recorded draws, which cannot work: `Queue::write_buffer`
is ordered against the *submission* rather than against the commands inside it,
so every draw of the frame read the last chunk's values and the window showed one
chunk's slice several times over. Per-chunk data travels in an instance buffer,
the way the gump pass does it, and the whole window is one draw.

The player's marker is an overlay drawn after the terrain, not a pixel stamped
into a cached chunk: a chunk is immutable and a marker moves every step, so
stamping it would make walking invalidate terrain — the one thing
`docs/world/design_minimap_lod.md` exists to prevent. A cross and not a dot, in both
drawings (`radar::MARKER_ARMS`): at one pixel a tile a dot is indistinguishable
from a lamp post.

Under both there is a backdrop of `radar::UNKNOWN`, because terrain arrives a
chunk at a time and a window with no chunk yet would otherwise show the world
through it. Where a chunk is missing but a coarser ancestor exists, the ancestor
is drawn at its own LOD instead — see the plan's phase 2.4.

**The window it is drawn in.** `WindowSubject::Minimap` sits beside `Skills` and
`Status` — the three whose *existence* is local UI state rather than something
the shard opened. Opening it is `M`, which is provisional: the affordance is a
product decision the plan holds open.

**And the decision it needed, which is taken.** Every other window here is
hit-tested against the `Vec<Picture>` its layout produced: `Drawn` holds gump
art, and dragging, raising and closing all ask which *picture* the pointer is on.
The radar has no gump art, so an empty list would be a window the pointer can
never find. `Drawn::Minimap` carries **a rectangle that is not a picture** — the
second of the three ways out below — and `panes::minimap::EXTENT` is the one
place its size is decided, so the drag, the pointer and the terrain region cannot
each invent their own.

- **Give it a gump frame**, the way `status::window` opens with one
  `Picture::plain(GumpArt::Gump(FRAME), at)`. Still open as *decoration* — it
  wants a real gump id out of a client file, chosen the way `FRAME` was rather
  than guessed — but it is no longer what holds the hit test up.
- **Give `Drawn` a rectangle that is not a picture.** Taken. Honest about what
  the window is, and the hit test asks the rectangle before it asks the art.
- **Draw the map through the gump pass after all**, which is the decision
  `radar_pass` already refused, and for a reason that has not changed.

### Where everybody is: the facet map

"Control several at once" is unusable without one picture that shows all of
them, and it is nearly free: one pixel per tile from `radarcol.mul` — **read
now**, and reduced by the layer above — plus a marker per session. It is an egui
image and shares nothing with the isometric renderer, which is what makes it
cheap. It also answers the standing backlog item that a free camera can lose the
character entirely, for every character at once.

One map per `(install, facet)` and not one per client: two shards are two
worlds even where the files agree, and a marker is placed on the map its session
is standing in. Which is also the honest answer to what the character select
shows — a tree of shards, each with the characters logged in to it, because that
is the shape the state already has.

### The keyboard, and who hears it

Three modes, and they are the same question the camera lock already asks:

- the drawn session only, which is what happens today;
- a selected group;
- everyone.

Broadcasting a step is N independent `0x02`s with N sequences, whose acks come
back interleaved and are folded per session. Nothing is shared and nothing is
synchronised — the client does not decide that two characters stepped together,
it decides that one key sent two packets. Anything cleverer (formation, waiting
for the slowest) is a layer above this and must not be built into the fan-out.

### Two things that stop being backlog and become blocking

- **The facet is a startup constant.** Two sessions may stand on different
  facets, so the single `Arc<WorldMap>` becomes a cache keyed by facet, loaded
  on demand and shared by whoever is on it. `0xBF 0x08` is what says a session
  moved between them.
- **A whole `WorldView` is cloned per changed packet.** One standing character
  makes this invisible; ten characters beside a bank multiply it by ten, and the
  clone is of the map of every mobile each of them can see. Worth measuring
  before the count goes up, not after.

### What the shard permits is the shard's business

Each session is its own account and its own pair of sockets. A shard may refuse
several connections from one account or one address, and whether it should is
the operator's rule, not this client's — what this client owes is to report the
refusal *per session*, so one login failing is one row in the character select
and not the client giving up. Across shards the question does not even arise:
they have never heard of each other.

### The list has to live somewhere, and that is a decision

Three environment variables are a single session's worth of configuration. A
list of shards, each with an install path, a claimed version and its accounts,
is a file — and the moment it holds accounts it holds credentials, which is not
a thing to arrive at by accident:

- a password in a plaintext config is what every UO launcher has always done,
  and it is still the thing that leaks;
- the platform keyring is right and is a dependency and a headless problem;
- asking at connect time is free, correct, and unusable for the ten-character
  case this milestone exists to serve.

Deliberately unresolved here. What is decided is that the file names shards and
installs and *may* name accounts, and that whatever holds the password is behind
one seam rather than read wherever a login is built — because there will be a
lot of logins.

### Done when

Two accounts log in from one process, the character select switches which one is
drawn, the arrows drive the drawn one or all of them, and the facet map shows
every body. Two shards are configured and at least one test drives both.
`cargo test --workspace` is green, including a test with neither a window nor a
GPU that two sessions on one shard share one facet and two sessions on different
installs do not — `Arc::ptr_eq` both ways, because "the files are loaded once per
install" is the property the whole milestone rests on and it regresses silently
in either direction: a second copy is invisible until the memory runs out, and a
wrongly shared one draws a 5.x shard's world out of a 7.x shard's art.

