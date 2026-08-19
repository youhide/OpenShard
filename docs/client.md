# The client

The shard has always been server-only: the client is ClassicUO, and
`crates/common/protocol` is the contract between them. This document is the plan
for a second client — ours — and it starts where every client starts, by
connecting and walking into the world.

The order below is not a wish list. Each milestone is the smallest thing that
can be run and disbelieved, and each one is a prerequisite of the next: nothing
renders before the bytes arrive, and nothing arrives before the protocol can be
read in the direction a client reads it.

## What is already here, and what is missing

The protocol crate is complete in *one direction*. The server decodes what a
client sends and encodes what a server sends, so every packet type implements
exactly the half the server needed:

- `ClientPacket::decode` exists; `ClientPacket::encode` does not.
- `ServerPacket::encode` exists; `ServerPacket::decode` does not.
- `client_packet_length` and `frame_client_packet` exist. There is no
  server-to-client length table at all — the server has never needed one,
  because it knows the length of what it writes.
- `huffman::compress` and `huffman::decompress` both exist, but `decompress`
  takes a whole buffer and expects it to end at a terminator. A client reads a
  socket, not a buffer.

The readers for the client's own data files — `map`, `statics`, `uop`,
`tiledata`, `terrain` — used to live in `crates/server/world`, where a client
crate may not depend on them: `server/*` and `client/*` never depend on each
other. They now live in `crates/common/uofiles`, which both sides may use, and
`crates/server/world` keeps the gameplay built on top of them.

None of this is a defect. It is what "sans-io, server-side" honestly produces,
and the missing halves are mostly mechanical. What matters is that they are
*missing*, and that they are the first milestone.

## M0 — the protocol, in the other direction

Nothing else can start. Four pieces:

1. **`server_packet_length` and `frame_server_packet`.** The mirror of the
   client table. The numbers are not re-derived from a reference: every
   server-to-client payload already declares `EncodePacket::LENGTH`, so the
   table reads those constants and holds no second copy to fall out of step —
   the same argument as `ServerPacket::id`.

   An id our server never sends is `None`, i.e. fatal for the connection. That
   is deliberate: guessing a length for a packet nobody here writes would put a
   made-up number in the one table whose whole purpose is to be right.

2. **The decode side of `ServerPacket`, and the encode side of `ClientPacket`.**
   Not all at once — the login set first (`0x82 0xA8 0x8C 0xA9 0x1B 0x55 0x11
   0x78 0x1A 0x20 0x22 0x21 0xBF`), the rest as a milestone needs them.

3. **Incremental Huffman.** A game connection compresses *every write
   independently, terminator and all* (see `Session::send_packet`), so a client
   needs a decoder that can say "this many input bytes produced this block, and
   the rest is the next one". That is a `decompress_prefix` returning the
   payload and the bytes consumed; `decompress` becomes the one-shot case of it.

   **A block is not a packet, and assuming it is passes every unit test.** The
   login server answers a `0x91` with the feature mask and the character list in
   one buffer, which is compressed as one 1,115-byte block carrying two packets.
   Decompression fills a byte stream and framing splits *that* — two layers,
   kept apart, the way ClassicUO does it. This cost nothing to fix and would
   have cost a day to find without the end-to-end test below, which is what
   found it.

4. **Round-trip tests.** Until now an encoder could only be checked against
   hand-written bytes or against itself. With both halves present, every packet
   gets `encode(decode(x)) == x` — which tests the server's encoders with a real
   inverse for the first time.

## M1 — `crates/client/net`: connect, and enter the world

The milestone the whole plan hangs on, and the one that can be finished and
believed on its own.

- A sans-io `Connection`, the mirror image of `gateway::Connection`:
  `receive(bytes)` in, `poll() -> Event` out, an outgoing queue, and no socket
  anywhere near it. Byte boundaries are what is hard here, and a real socket
  will not reproduce them on demand.
- The login state machine: seed → `0x80` → `0xA8` → `0xA0` → `0x8C` → **a second
  socket** → seed + `0x91` → `0xA9` → `0x5D` → `0x1B` / `0x55` → in the world.
  The auth key from the relay is the only thing linking the two sockets, and the
  version travels in the seed — both are findings the server already paid for,
  and the client has to honour the same two.
- A Tokio driver in its own file that decides nothing.
- `WorldView`: what the server has shown us — our own serial, position and
  direction, the mobiles and ground items we have been sent, light, season, map.
  It is the client's side of `World::seen`, and it is a record of what arrived,
  never a guess about what is there.
- Walking: `0x02` with its sequence and fastwalk key, `0x22` to confirm, `0x21`
  to roll back to what the server says. This is the first place the client must
  be *right* rather than merely plausible — a mishandled reject desynchronises
  the position and everything drawn after it.

Done when an integration test drives the real server through the whole
conversation with this crate, and when the binary walks a character around a
live `cargo run -p openshard-server`.

That test lives in **`crates/e2e`**, a group of its own beside `common`,
`server` and `client`. It needs both ends in one process, and putting it on
either side would make that side depend on the other — the rule those two live
by. So it sits outside both, ships no code of its own, and nothing depends on
it. It is also what turned `crates/server/server` into a library with a
four-line binary: a test that wants a shard should call one, not build one.

Only what cannot be tested on one side belongs there. Framing, the login machine
and the tick all have better tests of their own; what is left for `e2e` is that
two correct ends actually agree — which is exactly what caught the compression
mistake above, on the first run.

**And one command runs both ends, with no network under them.**
`crates/e2e/playground` is that same arrangement with a window instead of
assertions:

```sh
cargo run -p openshard-playground -- --client "/path/to/Ultima Online Classic"
```

Every option is also the `OPENSHARD_*` variable it used to be, and a `.env` at
the workspace root is read before the command line, so in practice the install
is named once in `.env` (copied from `.env.example`, and never committed) and
the command is `cargo run -p openshard-playground`. `--help` is where both
spellings are written down; `--account` and `--character` pick which of the
stock development accounts to play.

A shard in a thread of its own, the window logged in to it, both ending
together — and **no port bound and no socket opened**. The two are joined by
`tokio::io::duplex`, a pair of in-memory pipes.

### The transport is a parameter at both ends

This is the part worth writing down, because it is not a shortcut for a
playground: it is the seam a world driven by something other than a person needs.
A virtual player walking, talking and being fuzzed at wants a connection per
player and no file descriptors, no ports and no kernel timing — and it must
exercise *this* login machine and *this* framing, not a second implementation
that agrees with them.

So each end names what it needs and nothing more:

- **The client asks a `Dial`** (`crates/client/net/src/transport.rs`) for its two
  connections. `Tcp` is a real client on a real network; `e2e`'s `InProcess`
  hands back a pipe. Two methods rather than one, because the two connections are
  not the same question: the first goes where the player said, the second goes
  where the *server* said in its `0x8C` relay — and an in-process shard
  advertises an address it never listens on, so it must be free to ignore the
  second without guessing which call it was looking at. `Socket` became
  `Socket<S>`; nothing above it changed.
- **The gateway serves a stream** (`gateway::Gate`). `ClientGatewayServer` is now
  that plus a listener, and `client_session_serve` is generic — so an in-process
  client goes *through* the gateway rather than around it.

One thing had to become explicit in the move. The write task used to close the
connection by dropping an `OwnedWriteHalf`, whose `Drop` shuts the socket's write
direction; `tokio::io::split`'s half does not, so the client's zero read — which
the whole teardown chain hangs on — would have arrived for a socket and never for
a pipe. The hang-up is now a `shutdown()` that is written down, and it means the
same thing for every stream. Two tests in `gateway::server` pin it.

What the pipes do *not* reproduce: segment boundaries, resets, Nagle, and a slow
reader filling a kernel queue rather than blocking a writer. The socket tests in
`crates/e2e/shard` cover those and stay exactly where they are —
`tests/in_process.rs` is the same login again with the transport swapped, and
its deadlines are there because a broken pipe arrangement hangs rather than
refusing.

**A third `Dial` is expected, and it is why the trait is worth its keep.** A
browser cannot open a TCP socket, so a client compiled to WebAssembly reaches a
shard through a WebSocket and something on the far side that speaks TCP to the
gateway. That is not scheduled here and it is not a milestone; what it does is
fix the rule the two existing implementations already follow — **nothing above
`Dial` may name TCP**, not in a type, not in an error, and not in a timeout that
only makes sense for a kernel queue. `crates/client/render` and
`crates/client/app` already carry `cfg(target_arch = "wasm32")` dependency
sections for the same eventual reason. The cost of the relay when it is written
is one `impl Dial` and a small binary; the cost of letting TCP leak upward
first is every caller.

### Stopping is one word, and everything hears it

A shard has three loops that never end on their own — the accept loop, a
read/write pair per connection, and the tick — and it used to have no way to end
any of them. `run_shard` left its loop on Ctrl-C, which the tick listened for
itself, and everything else ran until the process did.

So there is now a `gateway::Shutdown`: a value that is cloned and carried down
the call tree, not a signal handler and not a flag some module owns.
`ClientGatewayServer::bind` takes one, `Gate::new` takes one, every connection
task holds one, and `run_shard` takes the same one. Ctrl-C in the binary and a
handle in a test produce the same stop, on the same paths.

It is **level-triggered**, and that is the design rather than an implementation
detail: `requested()` resolves the moment the stop *has been asked for*, not the
moment it is asked for. A connection accepted one instant before the stop would
otherwise be served forever by a shard that had already saved and gone. It is
also what makes it safe in a `select!` loop — cancelling a waiter loses nothing,
because the thing waited for is a state and not an event.

What a stop does, in order: the listener stops accepting and is dropped, so the
port is free and a late client is refused rather than let into a world that is
saving; every connection task hangs up, which the client sees as the zero read
it would get from a process that had exited; and the tick leaves its loop, ends
every trade, takes one last full snapshot and **awaits** the save task. So
`run_shard` returns only once the world is on disk, which is what makes it
something a caller may wait for.

`crates/e2e` is where that last part matters. `spawn` hands back a `Running`
beside the address — stop it, or drop it, and the shard stops and its thread is
joined. Fifty worlds started and dropped is now fifty threads that end, which is
what a fuzzing run needs and what the old arrangement could not do at all.

What this does *not* yet do — `SIGTERM`, the bytes still in the outbox, and
telling the player why the world went away — is [`shutdown.md`](shutdown.md), a
plan of its own.

Two smaller decisions. The shard is given the same install the window reads
(`world.client_files`), because the client predicts each step's `z` from its own
copy of the facet and two ends reading different ground is a stream of `0x21`
rollbacks that looks like a client bug. And none of this lives in `client/app`,
which is why that crate is now a library with a thin binary — the move
`crates/server/server` already made, and for the same reason: something that
wants a client should call one rather than build one.

Backlog it leaves behind:

- **The facet is read twice in one process**, once by the shard and once by the
  window, because `Map` is loaded from a path by each end and neither knows the
  other is in the room. A few hundred megabytes, paid twice, and the same
  question the "a container is read whole into memory" item below is about. The
  honest fix is a `Map` that can be handed over rather than opened again, and
  M3b's `Arc<Map>` cache is where that belongs.
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

## M2 — `crates/common/uofiles`: the data files

The move first: `map`, `uop`, `tiledata`, and the format-reading half of
`terrain` leave `crates/server/world` for a crate both sides may depend on. The
world crate keeps the gameplay built on top of them.

Then the readers a renderer needs and a server never did: `hues`, `art` (land
and static), `texmaps`, `gumpart`, `anim` with `animdata`, `unifont`, `cliloc`,
`multi`, `light`, `radarcol`, `sound`, `verdata`. In that order the first picture
needs hues, land art, and the tiledata and map readers that already exist; the
first *hillside* needs `texmaps` as well.

No client files enter this repository, now or ever. Tests read
`OPENSHARD_CLIENT` and skip when it is unset.

## M3 — the first picture

Isometric 44×44 diamonds, ground only to begin with: no statics, no mobiles.
A flat green field in the right place proves the block loading, the coordinate
system and the hue table, and proves them separately from the sorting problem.

Then statics, then mobiles, then the labels — and with them UO's draw order,
which is the part of a UO renderer that is actually hard. The camera follows
`0x20`, and blocks load around the player.

**Statics and mobiles are drawn.** `crates/client/render` has three passes now:
the ground, and two sprite passes that differ only in where the quad goes —
`statics::collect` from the map's own `staidx`, `mobiles::collect` from a list
somebody else built. `crates/common/uofiles/src/anim.rs` reads the frames the
second one draws.

**What decides overlap is a depth buffer, not a draw order.** This is the part
worth writing down. Ground is drawn whole before any static, so painter's order
*within* a pass says nothing about the pass next door: without a shared depth,
every wall would be in front of every hill. So all three passes compute one
ordering on the CPU — `crates/client/render/src/depth.rs` — and all three test
it, which makes the pass order decide nothing but who clears.

The ordering is ClassicUO's, taken apart rather than copied.
`Chunk.AddGameObject` gives ground its average height less two, a static its
own height with one down for a background tile and one up for anything with a
height, and a mobile one above everything; `View.CalculateDepthZ` folds that
together with `x + y` into `(x + y) + (127 + z) * 0.01f`. That float form
overflows into the next tile at large `z`, so what is kept here is the integer
pair — sorted tile first — normalised around the camera, which puts the visible
frame where a 24-bit buffer has resolution to spare. A step of one priority is
1e-6 apart where the buffer resolves 6e-8, and `depth.rs` asserts the margin
rather than trusting it.

**A static sprite's zero pixel is absent; a land sprite's is black.** Opposite
rules in two files, and both are the client's: `ArtLoader.ReadStaticArt` writes
a run's pixel only `if (val != 0)`. Getting it backwards on statics draws every
sprite's bounding box as a rectangle.

**A mobile is placed from its frame, not from its tile.** Five directions are
stored and three are mirrors of them, so half of every creature is drawn
backwards — and flipping a picture moves its anchor to the other edge:
`MobileView.Draw` is `x -= flipped ? width - center_x : center_x`, `y -= height
+ center_y`. Using `center_x` for both makes every west-facing creature stand a
body's width from where it is. The flip itself costs nothing: a region with a
negative width samples its own texels backwards, which is asserted on a real
GPU rather than argued about.

`anim.mul` is 195MB and is the first reader here that does **not** read its
container into memory — the index is held and frames are read on demand, which
is why `Anim::frames` takes `&mut self`. The browser is the reason the rest of
`uofiles` will follow.

**The ground half is done.** `crates/client/render` draws it and
`crates/client/app` puts it in a window: `cargo run -p openshard-client-app --
--client …` opens on Britain and the arrow keys walk the camera. The
crate is `wgpu`, and it is browser-shaped on purpose — WebGL2's ceiling, no
compute, no storage buffers, instancing through vertex buffers, every device
request `async`, and a 2048 atlas because that is the only texture size WebGL2
guarantees. It compiles for `wasm32-unknown-unknown`; nothing runs there yet,
because a browser has no filesystem and every reader in `uofiles` opens a path.

What made it worth doing rather than looking at: the renderer draws into an
offscreen texture just as readily as into a surface, so `tests/frame.rs` reads
frames back and asserts on the bytes. A lone sprite is compared to the art it
came from texel for texel, and level ground is asserted to cover **every** pixel
of the viewport — 393,216 of 393,216. Both found real defects on their first
run: `visible_tiles` was widened for `z` in only one direction, which loses a
band of ground wherever the terrain goes negative, and the atlas treated a black
pixel as a transparent one. Neither would have been visible on a screenshot.

**A slope is textured from `texmaps.mul`**, which is the difference between
terrain and a 44×44 diamond pulled out of shape. The art tile is drawn for level
ground; on a steep quad it smears, and the client does not use it there at all —
it takes a square 64×64 or 128×128 picture from `texmaps.mul` and maps it corner
to corner onto whatever the four heights make. Which corner is which is
ClassicUO's `DrawStretchedLand`, and it is the identity: the quad's top vertex is
the texture's top-left. `crates/common/uofiles/src/texmaps.rs` reads the pair,
and `tiledata`'s land entry — whose texture id this reader had been skipping past
for its whole life — is what says which texture belongs to which tile.

Nothing in either file relates a land graphic to a texture id, so reading that
field two bytes out gives *a* texture for every tile and the ground comes out
textured with somebody else's terrain: a picture, and one that reads as a
seasonal variant rather than as a bug. The test is a comparison rather than a
threshold — a tile's own texture and its own art have close average colours, and
the same measurement against a shifted pairing is three times worse (5.5 against
18.4 across 3,806 tiles). A file decides, not a number somebody chose.

Two things the half-texel inset is not: a nicety, and ours. A quad's corner
texture coordinates *are* the region's edges, and an edge is the boundary between
two texels — so at `u + du` the sample lands on the first texel of whatever was
packed next door, a one-texel fringe of the wrong terrain along two edges of
every sloped tile. The frame test caught it on its first run. ClassicUO insets
by half a texel in `CalculateHalfPixelUVs` for the same reason.

Where we knowingly differ from the client: a tile whose graphic has **no**
texture. The client refuses to stretch such a tile at all and draws it flat,
seams and all; we stretch it and texture it with the art, because the geometry
here is watertight by construction and giving that up would put the holes back.
`Land.ApplyStretch` is the reference, and the backlog has the rest of what it
does that we do not.

**Ground is stretched over its four corner heights**, which is the difference
between terrain and a mosaic of flat diamonds. A land cell stores one height and
it belongs to the diamond's *top* corner; the other three are the neighbours'.
Two tiles therefore do not merely abut, they are built from the same vertices,
and a seam between them is not expressible — which is why hilly Britain now
covers every one of its 393,216 pixels too, where flat diamonds left 2.3% of the
viewport in gaps. A tile whose four corners agree keeps the old path exactly: it
is drawn as the art's own square with the diamond cut out by alpha, so the
texel-for-texel comparison still holds where it can. The choice between the two
shapes is made in the shader from the heights themselves, so it cannot disagree
with them.

Deliberately not done here: **hue**. Ground carries no hue — `LandCell` has a
graphic and a height and nothing else — so the hue table has no consumer until
statics arrive, and building the plumbing for it now would be building it
untested.

**The window is joined to the wire.** `crates/client/app` logs in when it is
given an account and draws what the server has shown it — the character, and
everyone else on screen — with the arrow keys sending a `0x02` each and the
camera following the body the server confirms:

```sh
cargo run -p openshard-client-app -- --client … --account admin --password …
```

Without an account it stays the offline map viewer it was, which is the only
thing that runs against a facet nobody is serving.

The socket gets a thread and a current-thread runtime of its own
(`crates/client/app/src/link.rs`), because the event loop blocks on the
compositor and the runtime blocks on the socket, and neither can poll the
other. They exchange values: a `Facing` down, a whole `WorldView` back through
`EventLoopProxy`. Nothing about the protocol is decided there — `client/net`
owns the login, the walk and the view.

**Neither `0x22` nor `0x21` moves the body in `WorldView::apply`, and both move
it on screen.** The ack carries a sequence and no position, so the tile is the
one `Walk` asked for; the rejection is a rollback the view has no arm for. That
join is the one rule in `link.rs` and it is `fold`, tested without a socket or
a window: fold only one of the two and the client's own body stands still while
everyone else walks around it.

### What is still M3: a pass that blends

Everything drawn so far is opaque. Every pass writes depth and tests it, which
is what makes one ordering span three passes — and it is also the reason
`crates/client/render/src/cutaway.rs` *cuts* where the client *fades*. The port
of `UpdateMaxDrawZ` landed as booleans: a roof over the player is gone this
frame, where the client walks its alpha down 25 a frame and drops it at zero
about a fifth of a second later.

The blended pass is one step and it unlocks five things at once, which is why it
is written here rather than left as five backlog lines:

1. **`ProcessAlpha` whole.** `Cutaway::shows_static` and `shows_land` become the
   two ends of a ramp instead of a predicate, and `CalculateAlpha` is the ramp.
2. **`IsTranslucent`** — a window pane, a force field: alpha 178, and nothing to
   do with the cutaway.
3. **Foliage.** `CheckIfBehindATree`, `IsFoliageUnion` and `FOLIAGE_ALPHA`: a
   tree fades where a body walks behind it, and a tree is a *union* of graphics
   that has to fade as one or it fades in stripes.
4. **`HasSurfaceOverhead`.** A mobile under a `NoShoot` or `Window` static is
   drawn differently in the client (`AllowedToDraw`), which is what stops a body
   standing in a doorway from showing through the arch. It needs a 4x4 scan per
   mobile, cached on the mobile against `max_z`, and there is nothing to see it
   with until this pass exists.
5. **The circle of transparency.** A radius around the body inside which statics
   go translucent, so a player behind a wall can see themselves — the client's own
   feature and a player-facing setting, not a debug aid. It is a radial alpha and
   nothing else, so it is the cheapest of the five once there is a pass that
   blends, and it is worthless before there is one.

The order matters: a blended quad must not write depth, or it blocks whatever is
underneath it — which is exactly the note ClassicUO leaves on its own mesh path,
where a fading static is pulled out of the GPU buffer and drawn through the CPU
transparent list *after* the mobiles. So the pass is a fourth one, after the
mobiles, reading the depth the other three wrote and writing none of its own.

## M3a — the camera, and a shell to look through

**Built.** `cargo run -p openshard-client-app -- --client …` opens on
Britain, the wheel zooms about the cursor, a middle-drag pans, `Home` re-locks
the camera to the body, and the three panels are on screen. What follows is the
design as it was argued, with the places the code went another way marked — each
of them found by writing it.

**How the eye follows the body has a plan of its own from here on:**
[`docs/camera.md`](camera.md). What is below is the projection, the zoom and the
shell — the geometry the camera is made of, which is settled. Which camera runs
on top of it is not, and the answer is a bench rather than an argument.

This client is deliberately not a copy of the client. The camera zooms, pans
freely, and can be unlocked from the body; the interface is egui windows and
panels rather than a wall of gumps. Those are decided together rather than one
at a time, because all three want the same thing the code does not have yet: an
honest, invertible map between where something is in the world and where it is
on the screen.

Today there is half of one. `project` turns a tile into pixels and
`Camera::to_screen` turns those into an offset inside the viewport, and there is
no way back — nothing here can answer *which tile is under the cursor*, which is
what a zoom about the cursor, a drag, and eventually M5's clicking all reduce
to. Both halves also return the same `ScreenPoint`, so world pixels and viewport
pixels are one type today and mixing them is not a compile error.

### Two spaces, and a type for each

Three coordinate spaces exist and only the first is named:

- **Tile space** — `Point { x: u16, y: u16, z: i8 }`, the server's, and the only
  one that ever goes on the wire.
- **World pixels** — what `project` returns. Origin at tile `(0, 0, 0)`,
  unbounded in both directions, `y` down, no camera in it at all.
- **Viewport pixels** — where a thing lands in the rectangle the world is drawn
  into: origin at its top-left, and *after* the zoom.

So `ScreenPoint` splits into `WorldPixel` and `ViewPixel`, which are the same two
fields and two different meanings — the newtype rule, applied to the one place in
this crate where a raw pair of `i32`s currently serves two masters. Neither gets
`From` or `Into`: the only thing allowed to move between them is a camera, and
a conversion that needs a camera is a method, not a coercion.

**Built, and the third space is real but has no type.** `ViewPixel` is a pixel of
the *image the world is drawn into* — the offscreen target, which is the viewport
only at zoom 1. A viewport pixel is therefore a third thing, and it exists: the
cursor arrives in one. It gets no newtype because it never travels —
`Camera::pick(x, y) -> WorldPixel` takes one and the zoom is undone inside that
call, so there is nothing to carry and nothing to confuse. A type for a value
that is born and consumed in one expression buys nothing.

The camera is then four things and one rule:

```rust
pub struct Camera {
    /// Where the middle of the viewport looks. Pixels and not a tile: a tile is
    /// 44 pixels across and a drag is one pixel at a time.
    eye: WorldPixel,
    zoom: Zoom,
    /// The viewport, in *physical* pixels — the rect the UI leaves free, not
    /// the window.
    width: u32,
    height: u32,
}
```

and the rule is that the two conversions are exact inverses:

- `to_view(WorldPixel) -> ViewPixel` — `(w - eye) * zoom + half viewport`
- `to_world(ViewPixel) -> WorldPixel` — `(v - half viewport) / zoom + eye`
- `to_screen(Point) -> ViewPixel` keeps its meaning as `to_view(project(p))`, so
  every existing caller in `ground`, `statics` and `mobiles` is untouched.
- `unproject(WorldPixel, z) -> (u16, u16)` is the named inverse of `project`,
  which exists implicitly inside `visible_tiles` today and is written out here
  because picking needs it and because a formula with no name gets a second copy.

**Two of those came out differently, and both because the zoom is in the blit.**

`to_view` and `to_world` have *no zoom in them at all*: they are
`w - eye + half(image)` and its exact integer inverse. The formula above —
`(w - eye) * zoom + half viewport` — would scale the geometry as well as the
blit, drawing the world twice as large into a target that is already scaled. It
also cannot be an exact inverse at `2/3`, so the round-trip property the "done
when" asks for would have had to become a tolerance. The zoom enters exactly
twice: in the size of the image (`Camera::render_width`) and in `Camera::pick`.

`unproject` returns `(i32, i32)`, for the same reason `TileBounds` holds `i32`:
world pixel space is unbounded, a pixel north of the map's corner *is* a negative
tile, and a `u16` would have to clamp — which invents a tile rather than
reporting one. The caller knows its map; this knows arithmetic.

`eye` is private with `Camera::look_at`/`Camera::eye` either side of it, because
"where the camera looks" is the one piece of state two writers already fight
over: `App::entered` pins it to the player and `App::step` moves it offline.
One method, and the lock below decides which of them may call it.

**The eye is whole world pixels, and the remainder lives in the input handler.**
At zoom 2 a one-pixel drag is half a world pixel, and an eye that carried the
fraction would put every sprite on a half-texel boundary for half of all camera
positions — the same class of defect as the half-texel inset two sections up,
except spread across the whole frame instead of one edge. So the drag
accumulates its remainder where the mouse deltas are summed and commits whole
world pixels to the camera.

### Zoom is a ratio, and the scale is applied once

`Zoom` is a fraction from a fixed ladder — `1/2, 2/3, 3/4, 1, 4/3, 3/2, 2, 3, 4`
— and not an `f32`. Three reasons, and the third is the one that decides it:
`Camera` is `Copy + Eq` and several tests compare cameras, which an `f32` field
takes away; the offscreen target's size has to come out the same integer every
frame or the world is reallocated on rounding noise; and a ladder is what a wheel
notch wants anyway. The type keeps its numerator and denominator private and
hands out `Zoom::scale_up`/`scale_down`, so a zoom off the end of the ladder is
not expressible.

The scale itself is applied in exactly one place: the world is drawn at 1:1 into
an offscreen `Rgba8Unorm` texture of `ceil(viewport * den / num)`, and that
texture is blitted into the viewport rect. Every quad, every atlas region and
every pixel-exact assertion in `tests/frame.rs` therefore keeps meaning what it
meant, because nothing in the three world passes learns what a zoom is; what is
new is one fullscreen blit and one uniform. It is also what ClassicUO does in
substance, and it is the only arrangement where the UI stays crisp at 1:1 while
the world is magnified — scaling the geometry instead would resample five-bit art
through a filter at every fractional step.

**Nearest above 1, linear below.** Magnifying pixel art, a texel has to stay a
square; minifying it, nearest samples one texel in four and the ground shimmers
as the camera walks. Two rules rather than one filter, and the reason is written
next to them.

**The zoom-out limit is the GPU's, and it is small.** WebGL2 guarantees only
2048 in each dimension, so a 1024×768 viewport at `1/2` already wants
2048×1536 and a 1080p window wants more than the floor allows. The ladder is
therefore clamped at runtime against `limits.max_texture_dimension_2d` and the
clamp is *reported*, because a silently truncated target draws a smaller world
into a larger rect, which looks like a bug in the projection. If that limit turns
out to bite on real hardware, the fallback is scaling the geometry after all —
recorded here so the choice is a measurement rather than a rediscovery.

**Zooming is about the cursor, not the centre**, which is the first thing the
invertible pair buys: hold `to_world(cursor)` fixed across the change and solve
for the new eye. One line, and it is the difference between a camera that feels
placed and one that feels shoved.

**Except while locked, where it is about the centre.** An eye pinned to the
cursor would be moved by the zoom and moved straight back by the next
`WorldView`, which is a fight rather than a camera. Locked zooms about the
middle, which is where the body is; unlocked zooms about the cursor.

**And the device can refuse.** The clamp is not only on the way down the ladder:
the image is `viewport / zoom`, so *growing the window* at a zoom that fitted
asks for a texture that does not, and nobody zoomed. So the fit is checked where
the size is used — `App::fit_zoom_to_device`, once per frame — and it steps the
zoom back in rather than letting `world_texture` fail validation. Checking it
only in the wheel handler passes every test anybody would write and breaks when
a window is dragged wider.

### The lock

```rust
enum Follow {
    /// The eye is the body's, and the server moves it.
    Body,
    /// The eye is the mouse's, and the body may walk off screen.
    Free,
}
```

It lives in `App` and not in `Camera`: the camera does not know what a player is,
and giving it one would put `client/net` inside `client/render`.

- `Body` is what happens today — every `WorldView` update calls `look_at`.
- `Free` means the view no longer moves the eye at all. Drag pans; the arrows
  still walk the character, because walking and looking are different questions.
- Re-locking snaps rather than eases. Easing wants a per-frame clock over a
  mobile that survives between frames, which is exactly what the "everything
  stands" backlog item below is waiting for, and both should be built once.
- Middle-drag pans, the wheel zooms, `Home` re-locks. The camera panel shows
  which mode is on and can toggle it, so the state is never invisible.

### The shell: egui, and the wgpu version it does not have

`egui-winit 0.35` works with our `winit 0.30` untouched. `egui-wgpu 0.35` is on
**wgpu 29** and this client is on **wgpu 30**, and the two do not mix: a resolve
puts both in the graph and a `Device` from one is not a `Device` for the other.
Downgrading is not free either — `Instance::new`, `CurrentSurfaceTexture` and
`queue.present` are all wgpu 30 shapes here.

**The port turned out to be four lines.** `RequestAdapterOptions` gained
`apply_limit_buckets`, `VertexState::buffers` became a slice of `Option`,
`AdapterInfo` gained `limit_bucket` and its `transient_saves_memory` became an
`Option<bool>`. `renderer.rs` — the part that actually draws — needed one of
them. Each is marked `wgpu 30:` in the vendored source and listed in
`vendor/README.md`, which is also where the exit condition lives.

So: **vendor `egui-wgpu`, port it to wgpu 30, and send the port upstream.** The
vendored copy lives in a top-level `vendor/` directory rather than in
`crates/*/*`, because the group is part of the path here and a third-party crate
belongs to none of the three groups; it keeps its own MIT/Apache-2.0 licence
files. `[patch.crates-io]` points at it. The exit condition is written into the
crate's own README: when upstream releases wgpu 30 support, the directory and the
patch are deleted in one commit. The fallback, if the PR stalls and the vendored
copy starts to rot, is a paint pass of our own — egui's output is clipped
triangle meshes and texture deltas, which is `SpriteRenderer` with a scissor
rect and no depth attachment.

Four things the integration has to get right, each of which is silent when wrong:

1. **Colour.** The surface is deliberately non-sRGB, and egui's shader assumes
   an sRGB target unless told otherwise — the usual symptom is a UI that is
   merely *slightly* too bright, which nobody reports as a bug. A flat panel of a
   known colour, read back and compared to the byte egui asked for, is the test;
   this is the "colour is never converted" rule meeting somebody else's renderer.
2. **Depth.** The UI pass takes no depth attachment. The world's depth buffer
   orders the world.
3. **Input.** `egui_winit::State::on_window_event` answers whether it consumed
   the event, and a consumed event must reach neither the camera nor the walk
   keys. One `if`, in one place — otherwise a drag inside a panel pans the world
   underneath it.
4. **Points against pixels.** egui lays out in logical points and the world is
   drawn in physical pixels, so the rect egui leaves free is multiplied by
   `pixels_per_point` before it becomes the camera's viewport. Getting this
   wrong is invisible at scale factor 1 and wrong on every HiDPI screen.

Layout: `CentralPanel`'s available rect *is* the world's viewport, so docked
panels shrink the world and floating windows sit over it. The camera's `width`
and `height` therefore stop meaning "the window" — which is already how resize
works, so it is one more caller of a path that exists.

**In egui 0.35 that rect comes from the root `Ui`.** The frame is
`Context::run_ui(input, |ui| …)`, panels are `egui::Panel::top(id).show(ui, …)`
inside it, and what is left is `ui.available_rect_before_wrap()` — there is no
`CentralPanel` to ask. Windows still take the `Context`. The consequence is the
same and the call is not, which is worth writing down once rather than
rediscovering at the next version bump.

The wait loop grows one term: `about_to_wait` currently re-arms from the
animation clock, and egui asks for repaints of its own, so the deadline becomes
the earlier of the two.

### What the first panels are, and what they are not

egui is a **dev-HUD** for now. Whether the client's real interface is egui or the
`0xB0` gump layer is M4's decision and this milestone must not take it by
accident, so nothing built here may reach into `client/net` or `WorldView` beyond
reading them:

- a status strip — connection state, our serial and position, frame time;
- a camera window — the zoom, the lock, the eye, the viewport size;
- a world window — the mobiles and ground items `WorldView` is holding, which are
  decoded and completely invisible today.

Deliberately absent: the journal, the paperdoll, containers. Those are M4, and
building them in egui now would decide M4 without arguing it.

A **Light** tab joined them, and it is the same kind of thing: every number the
lighting is turned by — the flame's own size (which is how hard a shadow is), how
many rays a fragment casts at it, the brightness and reach of every flame, the
two halves of the ambient, and where the sun stands. `light::Tuning` is the type,
`desk::Light` is the file's copy of it, and `light::Tuning::clamped` is the one
place a number's domain lives, so a slider and a hand-edited `client_ui.toml`
cannot disagree about what is allowed.

Two things about it are not taste. The knobs are **read where they are applied**
rather than scaled onto a finished frame: the reach is what the occlusion grid's
own rectangle is grown by (`light::lit_tiles`), so a pool widened afterwards
would light tiles out of a grid that holds no walls for them. And the ray count
now travels **on the wire**, in a word of the blit's own header that was padding
— `the_shader_casts_as_many_rays_as_the_frame_asks_for` is what says it arrives,
by drawing one ray against eight and requiring the two frames to differ.

What is deliberately *not* there: night, the sun's own switch, the lantern, the
sky field and the debug views. Those are F10, F8, F7, F6 and F11 and have been
since before the tab, and a second way to spell one state is how the two come to
disagree.

### Done when

`cargo run -p openshard-client-app -- --client …` opens on Britain, the
wheel zooms about the cursor, a middle-drag pans, `Home` re-locks the camera to
the body, and the three panels are on screen. `cargo test --workspace` is green,
including: `to_world(to_view(w)) == w` over the whole ladder,
`unproject(project(p), p.z) == (p.x, p.y)`, the existing "every tile that lands
on screen is inside the bounds" property re-run at each zoom, and a frame test
that the blit at zoom 1 is texel-for-texel what the world pass drew.

**All of the tests are.** The blit test is the load-bearing one: with the world
drawn offscreen, every other pixel-exact assertion in `tests/frame.rs` is about
an image the screen never shows unless the blit is the identity at 1:1 — a
half-texel of sampling error, a flipped vertical axis or a filter left on all
read as "slightly soft" on a screenshot and are exact there. It needs no client
files, because a scene made of two gradient diamonds has the edges a flat wash
would not.

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

- **Shared, immutable, `Arc`, and keyed by install.** `Map`, `Art`, `TexMaps`,
  `TileData`, `HueRamp`. The precedent exists — the facet is already an
  `Arc<Map>` handed to the shard thread so `Walk::step` can predict a height.
  `Anim` is the exception and the awkward one: `Anim::frames` takes `&mut self`
  because reading a frame seeks the file, so it is shared behind a lock or it is
  read behind the atlas that already caches what it produced.

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
it. Three details are each a bug: `Map::statics_at` is keyed by `(y, x)` and
**not** sorted by z, so the highest is compared for rather than taken; the
comparison against the land is `>=` and not `>`, because a floor lies at the
ground's own height and `>` draws grass through marble; and a tile with no
colour is `UNKNOWN` rather than transparent, because zero spells *absent* in
these files and a transparent pixel is a hole in the window. The walk is
block-major so `statics_in_block` is asked once per 8×8 rather than once per
tile — the same cost `map.rs` records as the largest single phase of the
lighting pass when it was asked per tile.

**The radar gets its own texture and its own draw, outside `GumpAtlas`.** The
deciding property is *mutability*, not identity: `GumpArt`'s two arms both name
a picture in a client file and the atlas is a shelf packer for art that never
changes, while a radar is generated and rewritten every step. It is also the
cheaper way round — the gump atlas is 2048 square, so a corner of it for a
256-tile radar would carry sixteen megabytes to draw a sixty-fourth of it.

`radar.wgsl` has no hue (there is no ramp for a colour that was never in a
client file) and no instances (one quad, so its place is in the uniform). **It is
validated at pipeline creation and nowhere else**, being `include_str!`d — so
`tests/radar_pass.rs` builds the pipeline on a real device, and earned itself
immediately: the first draft named a uniform field `target`, which WGSL reserves.

The player's marker is stamped into the bitmap rather than drawn as a second
quad, so it rides the upload the map already costs. A cross and not a dot: at one
pixel a tile a dot is indistinguishable from a lamp post.

**What is left, and it is the window.** `WindowSubject::Minimap` beside `Skills`
and `Status` — the two whose *existence* is local UI state rather than something
the shard opened — its input in `own_windows/minimap.rs`, `Resources` holding the
colour table, `Screen` holding the pass beside `gump_pass`, and the per-frame
fill from wherever the player is standing.

**And one decision it turns out to need, which is not taken.** Every other window
here is hit-tested against the `Vec<Picture>` its layout produced: `Drawn` holds
gump art, and dragging, raising and closing all ask which *picture* the pointer
is on. The radar has no gump art. Its map is a texture of this client's own, so
its `Drawn` would be an empty list — and an empty list is a window the pointer can
never find, cannot drag and cannot close.

Three ways out, and they are not equivalent:

- **Give it a gump frame**, the way `status::window` opens with one
  `Picture::plain(GumpArt::Gump(FRAME), at)`. Then the frame is the hit
  rectangle and nothing else changes. It wants a real gump id out of a client
  file, chosen the way `FRAME` was rather than guessed.
- **Give `Drawn` a rectangle that is not a picture.** Honest about what the
  window is, and it changes the shape of every hit test that currently assumes
  art.
- **Draw the map through the gump pass after all**, which is the decision
  `radar_pass` already refused, and for a reason that has not changed.

A first attempt at the wiring was reverted rather than landed half-built: the
enum variant and its five match arms are mechanical, and stopping at the one
question they lead to is cheaper than answering it in passing.

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
  facets, so the single `Arc<Map>` becomes a cache keyed by facet, loaded on
  demand and shared by whoever is on it. `0xBF 0x08` is what says a session
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

## M4 — the gump layer

The journal and the speech line, the status bar, the paperdoll, containers, and
generic gumps.

**Partly built, ahead of the rest of M4**, because the shard's staff commands
are unreachable without it: they are `.`-prefixed *speech*, and `.admin` is
answered with a gump. What landed is the whole path and none of the art —

- `0xAD` encodes (`UnicodeTalkRequest::encode`) and `0xB0` decodes; `0xB1`
  encodes. Each is the direction the engine had never needed until it grew a
  client of its own.
- `protocol::gump::layout::parse` reads the layout language the builder next
  door writes, and is tested against it element by element. Total: an unknown
  keyword is an `Element::Unknown`, never a lost window.
- `WorldView::gumps` holds what is open, and `gump_closed` is the one thing the
  wire never says — a reply button closes a window client-side.
- `client/app/src/gump.rs` draws a layout with egui's own widgets — still the
  *dev HUD's* rendering, in the same spirit as the rest of that file.
- **The speech line and journal are no longer egui's.** `App::chat` holds what
  is typed and whether the keyboard is listening for it; `App::window_event`
  gives it the keyboard ahead of every hotkey and walk key once focused
  (opened by Enter, the reference client's own gesture); and `App::draw` reads
  `WorldView::journal` directly and lays both out through
  `openshard_client_render::text::{GumpLabel, collect_gump}` — a top-left
  anchored, screen-space glyph layout bound to a second `GumpRenderer`
  (`Screen::gump_text_pass`), over the picture and under egui, the same corner
  the old `egui::Panel::bottom` claimed. `collect_gump` is written to be
  reusable rather than single-purpose: it is where a gump dialog's own
  `{ text }` / `{ croppedtext }` captions are meant to draw through too, once
  that lands — see the next bullet.

What is still M4 proper: the gump art (`gumpart.mul`), which is why a
`{ gumppic }` is drawn as a placeholder naming its graphic; hue lookup for gump
text; and the dialogs themselves, still egui's widgets over the client's own
gump art.

### Containers — the wire and the memory, not yet the window

A chest was **write-only**: the shard encodes `0x24`, `0x25` and `0x3C` and
nothing in the tree could read one back, so a client double-clicking a container
heard three `Undecoded` ids and drew nothing. Two of the three pieces are now in.

- **The protocol reads back.** `OpenContainer` (`0x24`), `AddToContainer`
  (`0x25`) and `ContainerContents` (`0x3C`) decode, and the first two became
  `ServerPacket` variants on the way. They had been kept out because
  `EncodePacket::LENGTH` is a `const` that cannot ask its payload's version and
  both change size across a `Feature` seam — an excuse the *enum* never had, so
  `ServerPacket::length` takes the version now and the two hand-written encoders
  stay as thin wrappers for the shard, which sends them as bytes with the value
  already in hand.
- **`ContainerContents::container` is an `Option`, and that is the wire's shape.**
  A `0x3C` has no header field naming the container — every item record names its
  own — so a listing with no items has named nothing at all, and opening an empty
  chest sends exactly that. The client learns which container it was from the
  `0x24` before it.
- **`WorldView` holds two tables, not one.** `containers` is which art each open
  window uses; `contents` is what each container holds. A **vendor** is what
  separates them: the shop window is a `0x24` naming the *vendor* and the goods
  are a `0x3C` naming the crate worn on its shop layer, so a listing whose
  container has no window is a shape the shard sends on purpose.
- **Three things the wire never says**, each one a line in `view.rs`: taking an
  item out of a bag is a plain `0x1D` and nothing else, so a `Remove` that does
  not reach the contents leaves the icon in the window forever; closing a window
  is a click, so `container_closed` is `gump_closed`'s twin and drops the
  contents with it; and a `0x25` for an item already listed is a stack that grew,
  not a second item.

**The window draws.** It stopped at the same place `{ tilepic }` did — the
icons inside a container come from the *world's* art and the gump pass bound one
atlas, which held `gumpart` — and the fix was the key rather than a second pass:
`GumpArt::Gump` versus `GumpArt::Item`. Not cosmetic. Gump ids and art ids are
separate 16-bit index spaces and they overlap, `0x003C` being a chest's gump
*and* an item's graphic, so a shared `Graphic` key answers one with the other and
draws the wrong picture with no error anywhere to notice it by. The packer under
the wrapper stays keyed by a `Graphic` and what reaches it is a slot the atlas
hands out, private to two fields. Growing it now needs both readers, so it takes
an `ArtFiles` and not a `Gumps`.

`{ tilepic }` closed in the same move, which is what it was always going to be: a
container is the question a layout had been asking since the gump reader was
written.

`crates/client/render/container.rs` is the layout, and it is deliberately *not* a
layout at all — a `0xB0` is a program with pages and buttons; a container is one
picture with statics laid on it at the coordinates the `0x3C` carried. Its size
is its background's size, because there is no rectangle on the wire to
nine-slice. Its order is the shard's, because icons in a bag overlap and
re-sorting them would put a different one on top than the reference shows.
Picking is against the picture and not the box, for `items::pick`'s reason.

**And the window is not an egui window**, unlike the dialogs beside it: a
container has no widget in it — no button, no field, nothing that needs egui's
hit test — so its position, its drag and its z-order are the client's, in gump
pixels. Where it goes is the one thing a `0x24` never says, so the client
cascades them and the player's drag wins after that. Left raises and holds;
right closes, which is the reference's own gesture and does not fight the
right-hold that steers, because a press over a window is not a press into the
world.

Still open, in rough order:

- **Dragging an item out** — `0x07`/`0x08`/`0x09` and the drop back. Picking
  inside a window is built (`container::pick`); nothing sends yet.
- **A double-click inside a window.** `use_under_cursor` asks the world's
  `items::pick` and knows nothing about windows, so a bag inside a bag cannot be
  opened from the one that holds it.
- **Window positions are not remembered**, per container or at all: the cascade
  is recomputed from scratch every session.
- **The gump pass has no blending**, so a `{ checkertrans }` is skipped and a
  container's own art is drawn opaque. Fine for a bag; not for a paperdoll.

### The paperdoll — drawn

A `0x88` is the shard's answer to double-clicking a body, and it *was* the one
packet this engine sends that its own client could not read: `OpenPaperdoll`
encoded and was a `ServerPacket` variant, with no `DecodePacket` behind it, so a
client was handed `Ok(None)` and read on. It decodes, `WorldView` holds what is
open, and the window draws: a body picture with its equipment over it, in the
reference's order, in a window that drags, raises and closes like a bag's.

All five decisions are taken. What is *not* built is listed under decision 3 and
in the backlog below — chiefly `IsCovered`, which needs a graphic the client
does not carry yet.

1. **A `0x88` is not a listing, and `WorldView::paperdolls` is built on that.**
   It carries a serial, a 60-byte name and a flag byte (`PaperdollFlags`:
   warmode, can-lift) and nothing about equipment, because the client already
   has it: a `0x78` carries the wearer's layers, and `WorldView` folds them into
   the mobile (and into `Player` for our own, which is the only `0x78` a shard
   ever sends us about ourselves). So the table holds what is *open* — name and
   flags, keyed by the mobile — and reads the equipment out of the mobile it
   names. The same shape `containers` has beside `contents`, and for the same
   reason: the window and what is in it arrive apart, and one can change without
   the other. It also means taking a hat off reaches an open paperdoll with no
   packet of its own.

   Two things the wire does not say, each a line in `view.rs`: closing the
   window is a click, so `paperdoll_closed` is `container_closed`'s twin — and
   it drops nothing else, because the equipment belongs to the body; and a `0x1D`
   for the mobile closes it, which is `Mobile.Destroy` in the reference and the
   only thing that ever would.
2. **A paperdoll's art is gump art, keyed by the item's `anim_id`, and this is
   a third index space.** The body is a gump — `0x000C` male, `0x000D` female,
   `0x03DB` drawn as `0x000C` in hue `0x03EA` — and each worn layer's picture is
   `StaticTile::anim_id + 50000` for a male body and `+ 60000` for a female one,
   falling back to the male gump when the female one is missing
   (ClassicUO's `IsAnimExistsInGump`; `PaperDollInteractable.GetAnimID` is the
   whole function). `Equipconv.def`'s fourth column is read now
   (`EquipConvEntry::gump`) and overrides the `anim_id` before the offset for
   bodies that need it — a *different* override from the third column, which is
   the animation's; a row may send the two apart, and the client-file test found
   one that does. The column's two shorthands (`0` is the item's own `AnimID`,
   `-1`/`0xFFFF` the animation override) are resolved by the reader, where
   `ProcessEquipConvDef` resolves them, so nothing downstream has to guess.
   `Gumps::has` answers "does the client ship this picture" without decoding one,
   which is what the female fallback asks twice per layer.
   **`worn_graphic` cannot be reused**: it answers the same
   question into `anim.mul`'s body-animation space, and `+ 50000` into
   `gumpart` is a different picture of the same shirt. Two resolvers, one table
   each, and the `anim_id == 0` guard is the only line they share — with
   opposite meaning, since a backpack draws here and never on a walking body.
3. **The draw order is not the layer numbering.** ClassicUO builds the order per
   body (`PaperdollOrder.Build`) — arms and torso swap for a female or gargoyle
   body — draws the backpack last and outside that pass, and skips hair and
   beard on the dead. Every one of those is a question about which *layer* a
   piece was worn on, and
   [`EquipmentLayer`](../crates/client/render/src/mobiles.rs) carried a graphic
   and a hue and not the layer. It carries `Layer` now, through `crowd::worn`
   from the wire's own byte — one field, and the ordering table has something to
   be written against. It paid for itself immediately: **a ghost is bald**,
   which is the backlog entry below, and `worn_graphic` is the one place that
   decides it, for the atlas and the quad alike.

   The table is written: `paperdoll::order`, the whole of `PaperdollOrder.Build`
   — three base tables chosen by the arms and torso *graphics* (not by the body:
   an arms match locks the arms-late table and the torso is never asked), then
   the per-graphic exceptions, which are a list of garments whose art was drawn
   expecting a different neighbour. The layer names it is written against live on
   `wire::Layer` now, because a table of hex bytes cannot be checked against the
   reference it came from.

   The walking body reads the same table now — `paperdoll::world_order`, which
   is `PaperdollOrder.BuildInWorld`: `Build` plus the one cloak rule keyed on
   facing. "As the shard listed them" was *not* tolerable after all, and the
   case that showed it is the commonest one on screen: a cloak listed after a
   tunic, on a character facing the camera, was painted over the front of the
   tunic when it belongs behind the whole body. The rule has three arms and they
   are `LayerOrder.UsedLayers`'s three distinct rows — away, the cloak is
   painted last; facing the camera, first; edge-on, immediately under the
   helmet. `mobiles::drawn_layers` is the one list the three passes that walk a
   mobile's equipment share, so what is packed, what is drawn and what is
   pickable cannot disagree; it also drops the backpack from a walking body, as
   the reference does (`includeBackpack: false`).

   Not built: `MobileView.IsCovered`, which *hides* a layer an outer garment
   fully occludes — shoes under plate legs, arms under a closed robe. Every arm
   of it keys on the item's **wire graphic**, and that is the one graphic
   `EquipmentLayer` does not carry: it holds the `AnimID` `crowd::worn` resolved
   out of tiledata. Its absence draws a garment poking out from under a robe,
   which is visible and is not a hole.
4. **Female is the body graphic, not a flag on the wire.** Nothing in `0x78` or
   `0x88` says it; `mobile.IsFemale` is a fact about `0x0191`/`0x025E` and their
   kin. So the offset in decision 2 is chosen from the body the client is
   already drawing, and a mobile whose body is unknown is drawn male the way the
   reference does rather than not at all.
5. **The window is the container's, not egui's, and its background is the
   frame.** Position, drag, z-order, right-click close and picking against the
   picture were already the client's (`crates/client/render/container.rs`,
   `client/app`), in gump pixels, and a paperdoll is that machinery's second
   caller — which is the point of having written it there.

   What that machinery asks each window for is the one picture it *is*, and for
   a while a paperdoll answered with the body: there was no frame at all, and a
   doll floated on the world. The frame is `0x07D0` for our own character and
   `0x07D1` for anyone else's — `PaperDollGump.BuildGump`'s `0x07d0 +
   (LocalSerial == World.Player ? 0 : 1)`, written out as two named pictures
   because the arithmetic is how the file happens to be laid out and not a rule
   — and the difference between them is room down the right-hand side for the
   buttons a player gets over their own doll and nobody gets over a stranger's.
   The doll sits at `(8, 19)` inside it (`new PaperDollInteractable(8, 19,
   ...)`), one offset applied to the whole stack rather than per garment, since
   every layer already shares one origin.

   Drawing the frame first is also what lets a window outlive not knowing what
   is in it: `paperdoll::window` takes the wearer as an `Option`, the frame is
   drawn either way, and a doll the client has not been told the body of is a
   window that still picks up the pointer and still closes — the backlog entry
   that used to say it drew nothing.

   The machinery is shared by having the *subject* of a window say what kind it
   is: `App::own_windows` is one list of `WindowSubject::Container` and
   `WindowSubject::Paperdoll`, so a bag dragged over a paperdoll stays over it,
   and the two kinds differ in exactly two `match`es — what is laid out for it,
   and what closing one means to the `WorldView`.

   **A window has no size, and is picked by every picture it drew.** That is the
   third `match` gone, and the backlog entry that said a paperdoll was picked by
   its frame alone. `gump::pick` is one walk over a laid-out window — last
   picture first, because last is what was drawn on top, each against its own
   opaque texels — and it answers an *index*, because what a hit means differs
   per window kind and the walk does not. `container::pick` is that walk with
   the background dropped from the answer, and `App::window_under_pointer` is
   the same walk keeping only whether there was one. Nothing asks how big a
   window is any more, which is why `container::size` has no caller and
   `paperdoll` never grew one: a hat drawn past the edge of the frame belongs to
   the window and a click through the frame's transparent corner falls to the
   world, and neither of those is expressible as a rectangle.

   What the walk is picked over is the list the *last frame drew*
   (`App::drawn_windows`), not one laid out again at the press. A paperdoll's
   layout is not a function of the window alone — it reads the view, the
   tiledata and `gumpart` to decide which picture a worn item is — so a second
   walk asking those questions again is a second answer waiting to disagree with
   what is on the screen; it is `items::place`'s rule one layer up. The cost is
   that a window just opened is not pickable until it has been drawn once, which
   is the same frame its art is packed on and so the first frame it has any
   pixels to be picked by.

   What a paperdoll adds is *buttons* over its own art, which a container has
   none of. The hit test they want is now written — a button is a picture in the
   list and `gump::pick`'s index names it — and the layout is written with it.

6. **The frame's furniture is a table, and the name is a line of `fonts.mul`.**
   `PaperDollGump.BuildGump` puts every one of its buttons at `x = 185`, `y = 44
   + 27 * n`, and which button is on which row is the whole of the layout:
   help, options, log out, quests, skills, guild and the peace/war toggle down
   our own frame, then the status button on row seven — which is the only one a
   *stranger's* frame carries, because `0x07D1` has no column to put the rest
   in. Three pictures are not buttons at all: the profile scroll at `(25, 196)`,
   the party manifest fourteen pixels along it, and the virtue menu at `(80,
   4)`; the reference answers a **double** click on those and a single one on
   the buttons, which is why `paperdoll::DollButton` names windows rather than
   actions and leaves that difference to the caller.

   The peace/war toggle is the one picture the frame is drawn differently for,
   so `Whose::Own { war }` carries it: the flag comes off the `0x88`'s own
   `PaperdollFlags`, and the toggle only exists on our own doll, which is why
   the field is on that variant rather than beside it.

   The name is `new Label("", false, 0x0386, 185, font: 1)` at `(39, 262)` —
   and the `false` is `isUnicode`, which makes it the one string in this
   client's interface whose face the reference states outright: `fonts.mul`'s
   face 1, cropped to 185 pixels. It is drawn through `text::collect_gump` like
   every other line of interface text, so the plate is a `GumpLabel` and not a
   window kind of its own.

7. **A `0xB0` dialog is a window of ours now, not an egui one.** This is the
   layout fix, and it is worth writing down what was wrong: a dialog used to be
   an egui window with the shard's own background art drawn underneath it. That
   is two frames — egui's title bar and close box around the picture of a frame
   the shard sent — and, worse, two opinions about where everything in it is.
   egui needs a widget's size *before* the art is packed, so `client/app`'s
   gump module had invented one: a 26 by 20 button, a 220-point label, and a
   content rectangle measured from those. The clickable rectangle, the picture
   under it and the window's own extent were three different rectangles.

   All three are one list now. `gump::window` answers a `Window` — pictures,
   captions, `hits` and `fields` — and every question is asked of it:
   `gump::pick` for what was clicked, `Window::hits` for what that means,
   `gump::field` for the one thing in a window that is a box rather than a
   picture. Position, drag, z-order and closing are `App::own_windows`', the
   same machinery a container and a paperdoll have used since decision 5, so
   `WindowSubject` grew a third variant and nothing else about it changed.

   Four things fall out of it, and each was a defect before:

   - **A dialog opens where the shard put it.** A `0xB0` carries a coordinate,
     unlike a `0x24`, so dialogs are not cascaded — the client no longer
     second-guesses a layout it was handed.
   - **A button presses on the way down and answers on the way up**, with the
     pointer still on it. The layout has carried two pictures per button all
     along and nothing said when to draw the second; it is the mouse, and
     `Dialogs::held` is where the mouse lives between the two events. It is a
     `Hit` rather than a button id so that what is drawn pressed and what the
     release acts on cannot be two values.
   - **Text is the client's own.** A caption is drawn from `fonts.mul` through
     the gump pass, tinted by the same hue ramp every picture is, at the
     coordinate the layout named — and the layout's text hue is **one less**
     than the wire hue it means (`Label`'s and `CroppedText`'s constructors both
     add one), which the egui path had never done. `{ croppedtext }` crops
     character by character, which is what the reference's `FontStyle.Cropped`
     does and what a wrapping label did not.
   - **`{ textentry }` works without a widget.** A field is a box with an id;
     clicking one takes the keyboard, `winit`'s own `KeyEvent::text` fills it,
     and a caret is drawn at `text::gump_width` past the last character. The
     keyboard is given back on Escape, on Enter and on a press outside the
     window, so a letter is a letter typed only while a field is asking for one.

   What is still egui's is the dev HUD and the panels around the world. Nothing
   the shard sends is drawn by it any more.

8. **A paperdoll's buttons are requests, and the frame is drawn from the
   answer.** The layout landed in decision 6 with nothing behind it. What is
   behind it now is one rule stated three ways: *nothing is done locally on the
   way out.* The toggle does not flip its own picture, the Log Out button does
   not close the connection, and no window is opened by pressing anything — each
   button sends a packet and what is drawn follows the shard's answer to it.
   It is `App::use_under_cursor`'s rule for the interface, and it is why the
   client's own war stance is not a field the button writes.

   **The gesture is a dialog button's, with no dialog.** `App::held_doll` is
   `Dialogs::holding`'s counterpart: the press takes hold of a button, the frame
   draws its pressed face while the finger is down, and the release acts *only*
   if the pointer is still on the same button. It is keyed by
   `(WindowSubject, DollButton)` and not by picture index, because the doll is
   laid out afresh every frame and a hat coming off renumbers the list. Taking
   the press away from the drag is the other half: the column runs down the
   middle of the frame, and without this, pressing a button picked the doll up.

   **The three scrolls want a pair.** `scroll_pairs` is `App::last_click`'s rule
   applied to a picture — ClassicUO's 350ms, no distance test — and it compares
   the window *and* the picture, because the profile scroll and the party
   manifest sit fourteen pixels apart and a rule that only looked at the clock
   would open one because the hand slipped.

   **War mode has one home, and it is the player.** It arrives two ways — the
   `0x88`'s `PaperdollFlags::WARMODE` when a doll opens on us, and every `0x72`
   after that — and both fold to `WorldView::player.war`. The flag byte is *not*
   kept beside the window: `view::Paperdoll` carries `can_lift` and nothing
   else, because a stance stored on the packet that opened the window is a
   stance that goes stale the moment the next `0x72` lands, and the toggle would
   draw the older of the two answers. That the `0x72` was not being read at all
   is the defect this found: `ServerPacket::decode` had no arm for it, so the
   shard's answer was framed, dropped, and the toggle could never move.

   **What each button sends, and what four of them do not.** The wire half is
   `openshard_client_net::doll`, one function per button, each tested against
   this engine's own `ClientPacket::decode` — a button whose packet decoded as
   `Unknown` would look, from the window, exactly like one that works:

   | Button | Packet | What answers it |
   |---|---|---|
   | Peace/War | `0x72` | the shard's own `0x72`, which is what the toggle is drawn from |
   | Log Out | `0xD1` | the shard's `0xD1`, and the client closes the socket on it |
   | Quests | `0xD7` `0x32` | a `0xB0` — the quest log opens as a dialog, and that path was already built |
   | Guild | `0xD7` `0x28` | nothing yet: `guilds` is a stub, and the dispatch says so where it names the subcommand |
   | Status | `0x34` `0x04` | a `0x11` nothing draws yet |
   | Skills | `0x34` `0x05` | a `0x3A` nothing draws yet |
   | Profile scroll (double) | — | `0xB8` is not in `openshard_protocol` |
   | Party manifest (double) | — | `0xBF 0x06` is not either |
   | Virtue menu (double) | `0xB1` | a `GumpAnswered` the script pack can act on |
   | Help | — | `0x9B` is not in `openshard_protocol` |
   | Options | — | a window of the client's own that does not exist |

   Two of those deserve their reasons written down. **Status and Skills send
   only from our own doll**, because that is all this shard answers: the request
   is keyed on the connection and the serial in the packet is ignored
   (`StatusQuery::serial`), so pressing Status on a stranger's frame would fetch
   *our* status and open nothing about them. A health bar over somebody else is
   a window of its own. And the **virtue menu is a `0xB1` for a dialog nobody
   opened** — `ReplyGump(player, 0x1CD, 1, [subject])` is the reference's own
   convention, ServUO registers the same id — which reaches the script pack as
   an ordinary `GumpAnswered`, so a Community Pack can draw a virtue menu
   without an engine change.

   The four that send nothing press and come back up, and they are in the
   backlog rather than papered over: a packet invented here so that a button
   "did something" would be a shard logging an unknown id for a window that is
   never going to open.

   **The gate is both ends on one wire**:
   `crates/e2e/shard/tests/paperdoll_buttons.rs` logs a client into a real shard
   and presses three of them — the toggle in *both* directions (a client that
   folded "at war" from the mere arrival of a `0x72` passes half of that and
   fails the other), the Quest button until a `0xB0` opens, and Log Out until
   the grant comes back. Nothing else could have caught the missing decode arm:
   every unit test on either side of it passed while the toggle was dead.

9. **The skill window is a tree, and both of its tables are the client's own
   files.** The second window here that is nobody's layout: the shard sends
   numbers and nothing else. What a skill is *called* is `skills.mul` and which
   heading it is filed under is `skillgrp.mul`, and neither had a reader —
   `openshard_uofiles::skills` and `openshard_uofiles::skillgrp` are new. Two id
   spaces, two newtypes: `SkillId` indexes the names, `GroupId` the headings,
   and a window that mixed them would draw the right names under the wrong
   headings while never being out of range.

   `skillgrp.mul` is a file nothing but UOFiddler has ever read, and its shape
   is worth writing down: a count, then `count - 1` fixed-width names, then one
   `int32` per skill numbering **from one** into those names — zero means the
   group the file never names, which the reference tooling calls *Misc* and
   which holds eight of the fifty-eight. Read as zero-based, every skill comes
   out one heading early and nothing looks broken.

   **The `0x3A` is two packets sharing an id**, told apart by the byte after the
   length: `0x00`–`0x03` is the whole list (ids one-based, zero-terminated),
   `0xFF`/`0xDF` is one line (zero-based), `0xFE` is the shard sending its own
   skill *names*. That byte is also what says whether the rows carry a cap, and
   **it is the byte that is believed and not the version** — the version says
   what a shard should send, and a decoder that asked it would read every field
   of every row two bytes out of place the day the two disagreed. Two forms are
   refused by name rather than read as the form we know: the `0xFE` table, and
   the capless rows of a pre-4.0.0a client, which the reference hands a cap of
   1000 that nobody sent.

   **The window opens on the button and the packet only fills it.** The shard
   sends the whole list at world entry too, so a window that opened when a
   `0x3A` arrived would open itself at every login. `App::skills` is `Some` when
   the window is up *and* holds the tree — one field on purpose, since a
   `skills_open: bool` beside it could say the window was shut while its scroll
   position stood. It is the one window kind whose existence `WorldView` cannot
   answer for.

   **A list that scrolls needed a scissor, and there was none.** `Scissor` is a
   box applied on the processor: a quad straddling its edge is cut, rectangle
   shortened and region moved by the same fraction, which is exact for a sprite
   that is never rotated and needs no second pass. It rides on the `Picture`,
   because a picture is *picked* as well as drawn and a hit test reading a
   different box answers for a row scrolled out of the window. It is **not**
   `GumpLabel::clip`, which is `{ croppedtext }` — that one drops whole
   characters off the end of a line, ellipsis and all, and was checked against
   `FontsLoader.GetTextByWidthASCII` rather than assumed.

   **What the picture said that no test did.** Drawn and looked at, three
   things were wrong at once. Every window's text was being overwritten by the
   chat — one `GumpRenderer`, one instance buffer, and a queued write that lands
   before the encoder is submitted, so two `render` calls a frame draw the
   second call's quads twice and lose the first's; a defect the paperdoll's name
   plate has had for as long as there has been a journal line to overwrite it.
   The viewport was forty pixels too tall, so the list ran under the frame's own
   rule. And cutting *every* line to the list's box took the total with them,
   which is written on the frame below where the list stops — a line carries its
   own box now, exactly as a picture does.

   `crates/e2e/shard/tests/skill_window.rs` is the gate, and it is on the whole
   path rather than the decoder: the list a character is sent for entering the
   world, and the button asking for it again with the table emptied first —
   because with the login's own list still standing, every assertion in it
   passes on a shard that ignores the request. Checked by mutation: with the
   `0x3A` arm out of `ServerPacket::decode` both tests fail, and every unit test
   on either side stays green. The picture is `gumpshot`'s `skills` scene.

Done: double-clicking a mobile — or ourselves — opens a framed window that draws
its body and its equipment in the reference's order and hues, with the backpack
last; the window drags, raises and closes like a container's; a unit test says a
female body's order differs from a male one where the reference says it does;
and the client-file tests say a layer with `anim_id == 0` draws nothing, that
every gump a dressed body asks for is one the client ships, and that the female
fallback is exercised rather than merely available. The frame carries its own
buttons and its name plate, and the client-file tests say every one of those
pictures — up and pressed, on both frames — is one the client ships. Seven of
those eleven pictures now send something: the toggle asks for a stance and is
drawn from the answer, Log Out leaves the world, Quests opens the shard's own
dialog, and the rest are in decision 8's table.

Done for dialogs: a `0xB0` opens where the shard asked, drags by its own
background, closes on the right button with an answer of button zero (unless
`{ noclose }`), presses its buttons in their pressed art, flips its pages, keeps
its switches, takes typing into its fields, and draws its own text — with no
egui window anywhere in it.

**Seeing a window's layout without running the client**:
`crates/client/render/tests/gumpshot.rs` composites a laid-out window out of the
same quads the GPU pass draws and writes it to `target/gumps/`. It is
`artshot.rs` one layer up — that one answers what the artist drew, this one
answers where we put it — and it is how the paperdoll's buttons and the
nine-slice's seams were argued rather than guessed:

```sh
OPENSHARD_CLIENT=… cargo test -p openshard-client-render --test gumpshot \
    -- --ignored --nocapture
```

## M5 — interaction

Single and double click (`0x09`, `0x06`), drag and drop (`0x07`, `0x08`),
targeting (`0x6C`, `0x6B`), war mode. Speech (`0xAD`) landed early — see M4, and
so did **war mode**: the paperdoll's toggle asks for a stance and the client
folds the shard's answer (decision 8 in M4). What M5 still owes it is the
*picture* — a body drawn in its war stance, and an attack that follows from
standing in one.

**Double-click landed early too, because a door needs it.** A door is an entity
the shard placed, and the only thing that opens one is a `0x06` naming its
serial — there is no open-door packet, and what "use" means is decided entirely
server-side (`crates/server/items/src/doors.rs`). So the client's half is three
pieces and nothing about doors in any of them:

- `openshard_client_net::interact::use_object` — the `0x06`, with
  `DoubleClick::encode` in `common/protocol` as its other half. A test in each
  crate says what this client writes is what this server's own dispatch reads,
  and reads as a *use* rather than as the paperdoll request sharing the id.
- `openshard_client_render::items::pick` — **which item the cursor is over,
  picked against the picture**. Not against the tile: a door's leaf is drawn two
  tiles up the screen from the tile it stands on, so `App::pick_tile`'s answer —
  right for the Tile panel — names the tile *behind* the door and a player could
  never open the one they are pointing at. A hit is an opaque texel
  (`StaticAtlas::opaque_at`), because static art is mostly empty space and a
  bounding box picks whatever tall thing the cursor is merely inside; the
  topmost drawn wins, which is the largest `depth::Order` with the same
  later-wins tie-break the depth test gives the frame. `items::place` is the
  placement both `collect` and `pick` go through, so what is drawn and what is
  clicked cannot drift.
- `App::use_under_cursor`, on the second left click inside ClassicUO's own
  350ms (`Mouse.MOUSE_DELAY_DOUBLE_CLICK`), plus `App::item_serials` — the
  serial the renderer drops, put back by index.

The same pick runs every frame for the **highlight**: whatever the cursor is
over is drawn in `items::HIGHLIGHT_HUE` — ClassicUO's
`Constants.HIGHLIGHT_CURRENT_OBJECT_HUE`, replacing the item's own hue as the
reference does with `partial = false`, because a `hues.mul` ramp replaces a
pixel's colour rather than tinting it. Asked per frame and not remembered from
the last mouse event: the picture moves under a still cursor — the body walks,
the camera follows, a door swings — so what is pointed at is a question about
this frame's picture. `App::world_owns_pointer` gates the highlight, the tile
hover and the click alike, so a pointer over a panel lights nothing and uses
nothing.

A second way of saying the same thing — an **outline** round the sprite, pixel
first and glowing later — is planned in [`outline.md`](outline.md). It is
additive: the hue highlight stays, and the two compose.

Nothing is done locally on the way out: the door swings when the `0x1A` that
redraws it arrives. A client that also opened it itself would show a door the
shard refused — a lock, or reach — standing open.

### Picking a mobile — planned, not built

Today **nothing that breathes can be clicked**: `App::use_under_cursor` asks
`items::pick`, which walks ground items and nothing else, so the crowd is
scenery the cursor passes through. That was not a decision; a mobile simply has
no pick yet. The gap has a way of reading as a rendering bug — a creature drawn
from the wrong body looks like a broken sprite *and* refuses to answer the
mouse, and only one of those two is the defect (see the backlog entry below).

The shape of the answer is the item pick, one crate over, and the point of
writing it down before building it is that four of its five decisions are
already taken by `items::pick` and must not be re-taken differently.

1. **Pick against the picture, not the tile, and a hit is an opaque texel.** The
   same two rules and the same reasons as `items::pick`: a dragon's sprite is
   most of a screen of empty space, and a tile test names whatever the feet
   stand on. `AnimAtlas` needs the `opaque_at` that `StaticAtlas` has —
   `PackedFrame` already holds an origin and the atlas already keeps `pixels`
   on the CPU, so it is the same six lines keyed by `FrameKey` instead of
   `Graphic`.
2. **Go through `mobiles::place`.** It is already the one placement `collect`
   and `head_anchor` share, and a pick that computed its own rectangle would
   be a third answer that drifts. It also already carries the mirroring: a
   flipped frame is anchored from `width - center_x`, so the *texel* the cursor
   lands on has to be mirrored back before `opaque_at` is asked — the one piece
   of arithmetic here that the item pick does not have and cannot lend.
3. **A worn layer is a hit on its wearer.** Equipment draws with the wearer's
   `depth::Order` and no serial of its own; a click on a hat is a click on the
   head under it. So the pick tests the body frame *and* every layer
   `worn_graphic` resolves, and answers with the mobile either way.
4. **The topmost drawn wins, later-wins on a tie** — the largest `depth::Order`
   with `>=`, exactly `items::pick`'s tie-break, because it is exactly the same
   question about the same depth test. `Cutaway::shows_mobile` gates it with
   the same call `collect` makes: a body hidden under a roof this client is not
   drawing was not pointed at.
5. **The serial comes back by index**, `App::mobile_serials` beside
   `App::item_serials`, for the reason that one exists: the renderer is handed
   `Mobile`s with no identity in them, and putting the serial *into* the render
   struct would push a protocol type into the crate that draws.

Then three behaviours, in the order they are worth having:

- **Hover** — the outline, and the name. Both are local: the outline is the
  `outline.md` ring the items already get, and the name is what the view
  already knows, hung off `mobiles::head_anchor` rather than a fixed offset,
  because a rat and a dragon hold their heads at wildly different heights.
- **Double click → `0x06`** — built. `interact::use_object` writes it, the shard
  answers a mobile's use with a paperdoll (`0x88`), and the window that opens is
  M4's. Nothing on the way out says "paperdoll": it is the same packet an item
  gets, and which of the two it means is the shard's answer
  (`DoubleClick::interpret`). The serial comes from the pick — `App` keeps the
  `(Who, Mobile)` pairs rather than dropping the identity on the way into
  `mobiles::pick`, and a body with no serial (the offline viewer's placeholder)
  asks nothing.
- **Single click → `0x09`.** The shard has this end already —
  `Command::SingleClick` is dispatched and answered — so the whole cost on this
  side is the packet and the click. The answer is the name over the head, in
  the notoriety hue, which is the same overhead-message path the speech line
  already draws.

Done when: the cursor over any body in the crowd rings it and names it; a
double click on one sends a `0x06` naming its serial and a single click a
`0x09`; a click on a hat picks the wearer; a click on a creature standing
behind a wall picks nothing; and the pick and the draw cannot disagree, because
a test asserts that what `pick` answers for a cursor inside a frame is the
mobile `collect` drew there.

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

## M6 — sound

Built. The shard's two audio packets reach a device: `0x54 PlaySound` and
`0x6D PlayMusic` both decode, `crates/client/app/src/audio.rs` is the mixer, and
what it plays comes out of the player's own installation. The rule this
milestone existed for — **every visible action plays a sound and an animation,
not just a state change** — is honoured on the sound half; the picture half is
in the backlog below, which is the opposite of the order this was planned in.

- **The reader is `crates/common/uofiles/src/sound.rs`**, where the plan put it:
  the shard picks sound ids and may one day want to know an id exists. It opens
  `soundLegacyMUL.uop` under `build/soundlegacymul/{:08}.dat` when the install
  has one and falls back to the `soundidx.mul`/`sound.mul` pair, strips the
  40-byte name, and reads what is left as 22050 Hz mono 16-bit — while still
  recognising a RIFF/WAVE wrapper when there is one, because a rate read from a
  file beats a rate assumed about it. `UOSound.Delay` was never ported and its
  arithmetic never repeated: a length here is a sample count over a rate.
- **Music is not in the archive**, and is read as the plan said: the id → name
  map out of `Music/Digital/Config.txt`, the files themselves out of `Music/` —
  any capitalisation, any depth, mp3, ogg, flac or wav. `Music/Config.txt` is
  tried as well rather than being chosen by version, so the 4.0.1.1c boundary
  costs no `Feature::since` here. An install with no config at all falls back to
  the 67 classic names, and a numbered track is found by its id, which leaves a
  pack free to ship its own music without teaching the client a new protocol.
- **A track's third field is `loop`, and it is per track.** `9 britainpos,loop`
  repeats; `10 britain1` plays once. The fallback table carries the same flag
  per entry, from the reference's own (`SoundsLoader.cs`): `britain1` loops,
  `victory` does not. Reading the name and dropping the flag is how a victory
  sting ends up playing over a player who has walked away — so the flag is read,
  and the loop is the client restarting the track rather than a repeating
  source. See the second trap below for why that distinction is not stylistic.
- **The mixer is optional, not a trait with a null sink.** The property the plan
  wanted is the one that matters and it holds — nothing in the test tree ever
  asks for a device — but the shape is an `Option<NativeAudio>` decided once at
  startup rather than an implementation chosen where the renderer is: a missing
  output or an install with no sound archive prints one line and leaves the
  world playable. Under `wasm32` the whole of it compiles away.
- **Two gains, remembered.** `desk::Audio { effects, music }` — sliders on the
  HUD's Audio tab, persisted beside the light tuning and the window frame, for
  the same reason: someone who has turned the music down should not have to find
  the slider again every launch.
- **The shard's half is the region crossing.** `regions.rs::start_music` sends
  `0x6D` when a player crosses into a region that names a track, and refuses to
  re-send the track already playing, because `0x6D` *restarts* one rather than
  continuing it. 38 of the 128 saved regions carry a track.

**A silence with nothing wrong under it.** The first version of this played
nothing at all while every layer reported success: the packet decoded, the
config parsed, the file was found, the mp3 decoded, the volume was 0.45.
`rodio::Player::clear` *pauses* the player as well as emptying it, and `append`
lifts only the stopped flag — never the paused one — so the first track of a
session queued itself behind a pause nothing ever lifted and the client stayed
mute for the rest of the run. Clear, append and play are now one function,
`start_track`, with the reason written above it; and the test that holds it needs
no sound card, because `Player::new` builds the queue without a device. That is
the same property the null sink was for, arriving where the bug actually was.

**And behind it, the same silence again, from the other side.** With the pause
lifted the player reported a queued track, playing, at 0.45, and the device
stream ran — carrying zeroes. `Source::repeat_infinite` wraps a source in
rodio's `Buffered`, and `Buffered` asks how long the current span is *before*
pulling a sample; a freshly opened Symphonia decoder answers `Some(0)`, having
read no packet yet, and `Buffered` reads that as a stream that has already
ended. What `repeat_infinite` returns for a whole mp3 is therefore an infinity
of silence — and every symptom points at the device, which is the one part that
was working. **Two independent bugs with the identical symptom, stacked**: fixing
the first changed nothing a person could hear, which is exactly the shape that
makes a fix look wrong when it was right.

The loop is now the client's own — the track is remembered and started again
when the player runs dry, once a frame in `advance` — so nothing depends on
`Buffered`'s reading of a span it was never told. Priming the decoder with one
sample would also have worked, and would have left the silence one upstream
change away from returning. Two device-free tests hold it: one hands
`start_track` a source shaped like an unread decoder (`Some(0)` until its first
sample) and asserts the queue carries signal, and an `#[ignore]`d one does the
same through the real installed mp3 — `cargo test -p openshard-client-app --lib
audio -- --ignored`, with `OPENSHARD_CLIENT` set.

**How it was actually caught**, since none of the above is visible from the
code: a headless `sway`, the playground pointed at a dedicated PulseAudio null
sink (`PULSE_SINK`), and `parec` on that sink's monitor. Peak 0 is a fact; "I
hear nothing" is a report. The same measurement afterwards reads RMS 1221,
peak 5750 — and that difference is the whole verification, because every other
signal in the system said the music was playing the entire time.

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

## Decisions to take before they are taken by accident

- **The client is multi-shard and multi-session**, `[shard → {characters}]` —
  see M3b. It is a decision and not a feature, because it says three things that
  are cheap now and an audit later: everything downstream of a socket is per
  session, the data files are loaded once *per install* rather than once per
  process, and the claimed version belongs to a shard rather than to the client.
  The same argument as multi-era on the server, and for the same reason.
- **Crates.** `crates/client/net`, `crates/client/render`, `crates/client/app`,
  plus `crates/common/uofiles`. The direction rule stands: a client crate
  depends on `common`, never on `server`.
- **What we draw with.** `wgpu`, directly — no engine. Bevy and its neighbours
  would supply a window, input and sprite batching, and none of what is actually
  hard here: UO's draw order, the hue table, and streaming map blocks out of a
  155MB container. What they *would* impose is a second ECS beside `WorldView`,
  ownership of the main loop, and a frame that cannot be rendered inside
  `cargo test`. ClassicUO does not use an engine either, and not out of poverty.
- **The browser is a target, so it constrains the design now.** WebGL2's ceiling
  rather than native Vulkan: no compute, no storage buffers, instancing through
  vertex buffers, a 2048 atlas, and `async` device requests because a browser
  cannot be blocked on. Cheap to honour from the first triangle and painful to
  retrofit. What is *not* done: `uofiles` still opens paths, and a browser has
  no filesystem — the parsing is already separate from the reading, so the fix
  is byte-taking constructors and `std::fs` behind `cfg`, not a rewrite.
- **Colour is never converted.** Textures and targets are `Rgba8Unorm`, never
  `…Srgb`. The files hold five bits a channel with no colour space attached, and
  a gamma conversion anywhere means the pixel that went into the atlas is not
  the pixel in the frame — which turns every exact test assertion into a
  tolerance nobody can justify.
- **Which version we claim to be.** The client announces one in its seed, and
  every `Feature` gate on the server follows from it. 7.0.45.65 — what
  ClassicUO opens with — keeps us on the modern packet set instead of the legacy
  branches of every encoder. **Per shard, not per client** (M3b): the whole point
  of `Feature::since` is that a connection asks its own version, and a client
  facing two shards at two versions is exactly the case an era check gets wrong
  silently.
- **A client that only speaks what our server happens to send is a mirror, not
  a UO client.** That is the right scope for M1–M3, and it is also how both
  ends quietly agree on the same mistake. Every packet this client learns should
  be checked against a real client's behaviour, not only against our own server.

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
  number `load_facet` already had. `Map::load`, which has only a path, still
  cannot tell them apart, and its doc now says so.
- **`Map::load` is public and called only by its own tests.** It is also the one
  entry point that cannot resolve the collision above. Either it grows a facet
  argument or it stops being `pub` — but that is a decision about who is supposed
  to call it, and nobody does yet.
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
- **`Map` cannot be built in memory, so the renderer has no offline tests.**
  *(Planned: [`unenforced.md`](unenforced.md) S4.)*
  Every assertion about `ground::collect` lives in `tests/frame.rs` behind
  `OPENSHARD_CLIENT` and a GPU, because the only way to get a `Map` is to load
  one from a file. A constructor taking cells — or a small fixture facet — would
  let the projection and the visible-set logic be tested with neither. Both
  atlases now take pictures directly (`LandAtlas::pack`, `TexmapAtlas::pack`), so
  the *art* half of that no longer needs an install: the test that a slope is
  drawn from its texture and a level tile from its art is green with no client at
  all. The map is what is left.
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
  corners (`Map::average_land_z`, RunUO's `GetAverageZ`, ClassicUO's
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
     [`ttf_font.rs`](../crates/common/uofiles/src/ttf_font.rs), which already
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
    `find_path`, generic over `M: AsRef<Map>, T: AsRef<TileData>` so the server
    keeps building one owned at boot (`MapTerrain::new(map, tiles)`) and the
    client builds one borrowing (`MapTerrain::new(self.map.as_ref(),
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
    [`movement::Detour`](../crates/common/movement/src/detour.rs) answers with
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
    [`detour`](../crates/common/movement/src/detour.rs) — a scene (`Around`),
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

  The plan now reads the ground twice. `steer::Ground` carries both halves: the
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
  `docs/parity.md`'s standing argument, applied to a route.

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
  of `Map` + `TileData` and nothing else — no world state, no server crate. Moving
  it below `server/` (it is `common/*` material sitting in the wrong group, and
  `crates/common/movement` already owns the trait) gives the client the shard's
  own rules, byte for byte. Then, in order:

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
- **A pile's count has no switch and no size of its own.** Two things the
  counting above left open, both of them visible rather than wrong. One:
  every counted pile on screen is written over, always — a bank floor with
  fifty piles on it is fifty numbers, and there is no knob in the dev window's
  Graphics tab to turn them off the way there is for the crowd or the statics.
  Two: with `--ttf-font` on, the world's overhead text goes through `TtfAtlas`
  instead of `fonts.mul`, and that path does not read `Label::font` at all —
  one face at one pixel size, by construction (`text::collect_ttf`) — so the
  count is drawn in the size a spoken line is rather than in
  `items::STACK_COUNT_FONT`'s small face. A count wants to be smaller than
  speech, which is a *second* TTF size and therefore a second atlas, not a
  parameter.
- **The facet is a startup constant and `0x1B` only carries a size.** The app
  loads Felucca and compares the shard's map size once, warning when they
  differ rather than following. Following means decoding `0xBF 0x08` and
  reloading the facet, and the reload is the interesting half: `Map::load_facet`
  reads a few hundred megabytes. **M3b makes this blocking**: two sessions may
  stand on two facets, so the single shared `Arc<Map>` has to become a cache
  keyed by facet.
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
  [`EquipmentLayer`](../crates/client/render/src/mobiles.rs) carried a graphic
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
  let them.** [`lighting.md`](lighting.md) is the plan and the argument; the
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
  [`lighting.md`](lighting.md)'s, and the distance is now three-dimensional with
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

- **This client reads `anim.mul` and nothing else.** Everything drawn since LBR
  lives in `anim2.mul` through `anim5.mul`, and which file holds a body is a
  lookup in `Bodyconv.def` (`752 29 -1 -1 -1` — body 752 is index 29 of
  `anim2`); `Body.def` is a second redirect, an id drawn as another body under a
  hue. `uofiles::anim` knows neither, and `anim::animation_body` hardcodes three
  remaps where the client has a table. So **every body that lives in a later
  file draws nothing at all** — on the Felucca spawn set that is bodies 752 and
  764–794 among others, tens of spawn points, and each one is a creature that
  hits a player from an empty tile. This is the single largest hole in what the
  client can draw, and it is a file reader, not a renderer change.
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

## The dev HUD, and what it remembers

The five floating panels — Camera, Rig, Frames, World, Tile — are **one window
with five tabs**, and what the HUD looked like is written to `client_ui.toml`
beside `openshard.toml` when the client closes. See
`crates/client/app/src/desk.rs`.

This is worth writing down because the *absence* looked like a bug and was not
one: nothing was failing to save, there was nobody to save. `eframe` is the crate
that persists `egui::Memory` and the window's geometry, and this client is bare
`egui` on `winit` and `wgpu` — deliberately, because it owns its own event loop
and its own surface. So the saving is ours, and it is a named struct rather than
a serialized `egui::Memory`: the latter carries every widget id egui happened to
allocate, in a format whose meaning is egui's version, and an upgrade restores
nonsense or nothing.

What is remembered: the tab in front, whether the window is open, where it sits,
egui's `zoom_factor`, and the operating system's window — outer position, inner
size, maximized. A saved frame is only restored if its top-left corner still
lands inside some monitor: a laptop undocked since the last run has a saved frame
that opens the window offscreen, which looks exactly like a client that failed to
start.

The scale is *egui's* zoom and not the monitor's `scale_factor`, which stays the
platform's business — a file that pinned it would fight the compositor on the
next screen. Ctrl+`+` / Ctrl+`-` / Ctrl+`0` are egui's own shortcuts
(`Options::zoom_with_keyboard`); the status strip shows the number because a
client that reopened at yesterday's zoom and does not say so reads as one that is
rendering at the wrong size.

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

## The channel selector, and the whole of `0xBF`

Landed 2026-08-15, and it turned out to be two things.

### `0xBF` had no decoder at all

Not the party family — *the family*. `ServerPacket::decode` matches on the id
byte, and for `0xBF` the id byte is not a key: nine of the enum's variants share
it, and which one a packet is lives two bytes further in. There was no arm, so
every extended command the shard has ever sent reached this client as
`Undecoded` — the context menu, the spellbook's contents, the stat-lock arrows,
the map change and the close-gump, along with everything party.

The fix is a second dispatch (`decode_extended`) and, for party, a third
(`decode_party`), because party's four packets share a subcommand *and* differ by
a further byte. Every decoder reads that subcommand back and refuses a body that
is not its own — twice on purpose, once to pick and once to be sure, because
getting it wrong is not a malformed packet but a well-formed one read as the
wrong thing.

**Four of the nine are still undecoded and that is deliberate.** The context menu
(`0x14`), the spellbook (`0x1B`), the stat locks (`0x19`) and the map change
(`0x08`) each need a reader on this end that wants them, and a decoder written
before there is one only moves the packet from "undecoded" to "decoded and
dropped". Their subcommands are listed in `decode_extended`'s own doc so the next
person has them rather than a search.

### A channel, not a prefix

A guild line is ordinary `0xAD` speech with a different **mode byte** — it is a
property of the line, not of its first character. So `talk::say` takes a
`TalkMode` and `chat::Channel` is the selector: `say`, `guild`, `alliance`,
`party`, cycled with **Tab** while the line has the keyboard, drawn in the prompt
that already said `say:`.

The alternative considered and rejected was reserving `/` or `\` at the front of
the line, ClassicUO-style. It makes a character unsayable, and it hides the state
it sets — a player who typed a `/` and then deleted it has no way to know what
channel they are on. The prompt says, always, and the channel survives a send:
one that reset itself would make a conversation on any channel four keystrokes a
sentence.

**Party is not speech**, so the `Party` channel does not go through `say` at all
— it is `0xBF 0x06 0x04`, and putting it through the speech path would say it out
loud to the street.

### The party windows are egui, and that is a decision

Two of them: an invitation prompt and the roster, both `egui::Window`s in the
shell, following the stack-split prompt that was already there. Not gump art, and
not a shard-sent `0xB0`:

- **Not a gump**, because the roster window would be a thousand lines of render
  code (`client/render/skills.rs` is the measure) for a list of names and two
  buttons.
- **Not a shard-sent dialog**, which was the tempting shortcut — it would work on
  both clients with no client code at all — because ClassicUO draws its *own*
  invitation prompt off the `0x07`, and a shard that sent both would give
  ClassicUO users two.

Both windows are drawn straight off `WorldView::party` rather than from local
state, so neither can go stale: the question exists because the shard said so and
stops existing when the shard says otherwise.

**Members are shown by serial.** A `0x78` carries no name — a mobile is named by
a single click or by a tooltip, and this client may have done neither to the
person inviting it. A number beats a guess, and the fix is to ask for the
tooltip, which is now possible and is not wired here.

## The amount picker is a gump, and it is ClassicUO's

A Shift-drag out of a pile asks how much to take. That question used to be an
`egui::Window` with a `DragValue` and two buttons on it, anchored to the middle
of the screen; it is the reference client's own `SplitMenuGump` now —
`client/render/src/split.rs` for the layout, `client/app/src/panes/split.rs` for
the gestures — and it is the **seventh window kind**, with a
`WindowSubject::Split`, a `Drawn::Split` and a pane like every other.

Every coordinate is `SplitMenuGump.cs`'s: the frame is `0x085C` (164×74), the
knob is `0x0845` (15×15) on a 105-pixel bar at `(29, 16)` whose trough is
painted into the frame, the button is the three faces `0x085D`/`0x085E`/`0x085F`
at `(102, 37)`, and the number is written at `(29, 42)` in font 1, hue `0x0386`.
The bar's arithmetic is `HSliderBar`'s, including the part that is not quite
right — the value is measured from the trough's left edge without subtracting
half the knob, so the knob's *corner* follows the pointer. That is copied rather
than corrected, because it is what the gesture feels like in the client people
have played.

Three things it does not copy:

- **One number, not a number and a string.** The reference keeps a slider and a
  text box in step across forty lines of `UpdateText`. There is one `u16` here
  and two ways of writing to it: the bar scales it and a typed digit shifts it up
  a decimal place. The state a player is *not* looking at is the one that ends up
  in the packet, so there is only one.
- **The bar's top is the pile less one.** `ItemPress::dragged` has always said a
  split that takes everything is a lift with extra steps, and that rule is older
  than this window. The reference lets the bar reach the whole pile; here it
  reaches `amount - 1`, and the picker opens *at* that top so the button alone
  still means "as much as this gesture allows".
- **It is not dragged out from under the pointer on open.** The reference calls
  `AttemptDragControl` so the fresh gump follows the mouse that is still held
  down. Here it is placed at `pointer - (80, 40)` — the reference's own offset —
  and stays there.

**Escape and Enter stopped being one key.** `panes::Key` had three arms, because
a `{ textentry }` puts itself down either way. A modal is the window where they
are *opposite* answers, so there is a `Key::Cancel` now: Enter sends the number,
Escape dismisses the press the prompt was standing over. The dialog's field
answers both the same way it always did, in one arm.

The answer still travels the way it did — `Windows::prompt` names the presser and
`Input::Answered` is delivered to it by identity — but it no longer arrives a
frame late through `shell::Request`. It is an ordinary `Effect::Answered` on the
frame the button was pressed, which also means the gesture works in a build with
no HUD at all: the old effect was dropped on the floor when `App::shell` was
`None`.

## Tooltips, and the half that was never written

The AoS property list — the cliloc lines a modern client shows on hover — landed
on this end on 2026-08-15. The shard's half had been complete for months:
`WorldState::object_properties` built the list, `send_property_list` answered a
request, and `tooltip_packet` rode a revision alongside every draw. Nothing here
had ever asked for one, and a `0xD6` was framed and dropped as
`Event::Undecoded`. That skip was the fix that stopped a vendor's window taking
the connection down — it was never a reader, and it was easy to mistake for one.

### Two packets that had encoders and no arm

`0xDC` (the revision) and `0xD6` (the list) were both *encodable* and neither was
in `ServerPacket::decode`'s dispatch, so both reached this client as an id with
no name. `TooltipRevision` gained a `DecodePacket`; `PropertyListReply` is new,
and is the decoding counterpart of the `PropertyList` **builder** the server
writes through. Those two share no code — the builder accumulates a revision hash
as it writes and the reply carries one already computed — so they are pinned
together by a round-trip test instead: build a list, decode it, re-encode it, and
compare byte for byte. Without that, a change to either side's layout is a parity
break only a real client would ever notice.

Two wire details cost a reading of both references. The argument text is UTF-16
**little**-endian, unlike the `0xAE` speech packet's big-endian, so
`PacketReader::fixed_string_utf16` is the wrong reader and this module has its
own. And the byte length sometimes counts a NUL terminator this engine does not
write, so a trailing one is tolerated and dropped rather than drawn.

### Three modes, and why the cache has two fields

The shard has three `TooltipMode`s and they send different things: `version`
sends a revision with each drawn object and the list only when asked, `full`
sends the list unasked and never sends a revision at all, and `off` sends
neither. `WorldView::tooltips` therefore keeps "what the shard says is current"
and "what revision the lines we hold were built at" as two separate `Option`s.
Folded into one "do we have it" flag, `full` mode reads as permanently stale and
the client re-asks, on every hover, for a list it is already holding.

The lines are kept rather than cleared when a newer revision arrives, so a hover
during the round trip draws a name one edit out of date instead of a blank — a
blank reads as "this thing has no name".

### The hover asks, not the revision

The obvious wiring is to request a list the moment a `0xDC` says one is stale.
That is wrong, and it is wrong in the direction that costs bandwidth: the shard
sends a revision for *every object it draws*, so turning each into a request puts
every tooltip in view back on the wire and leaves the nine-byte announcement
doing nothing. The hover is what asks.

Which needs one piece of state the wire has no packet for — `app::tooltips`,
the set of questions already outstanding. "Stale" stays true until the answer
lands and a hover lasts many frames, so asking straight off the view's own flag
is one `0xD6` per frame for as long as the pointer sits still. A question is
remembered by the serial **and the revision it was asked at**, so a newer
revision asks again and a shard that never answers is silence rather than a
flood.

### What still is not right about it

- **No backing box.** The lines are drawn in `fonts.mul` face 1 at the cursor,
  the same look the container hover already had, over whatever is behind them.
  ClassicUO draws a dark translucent panel and it is much easier to read; that
  needs a filled-rect primitive in the gump pass, which does not exist yet.
- **One serial per request.** `0xD6` is a batch query and this sends batches of
  one, because the hover names one object. Anything that wants several at once —
  a container's whole contents, say, on the frame its window opens — would want
  the batching the packet was built for.
- **`0x09` single-click is still the other name path.** A shard in `off` mode
  falls back to the classic single-click label, and this client's handling of
  that is unchanged and unrelated. The two have never been reconciled into one
  "what is this called" question.
- **Nothing asks about a static.** The map's own furniture has no serial and no
  tooltip; hovering a wall says nothing, which matches the reference.

## A wheel notch that leaked out of the shop list

Scrolling a vendor's catalogue to its last row and rolling once more zoomed the
map. The window never moved and the pointer never left it, so what the player
saw was the shop staying put while the world jumped a zoom step behind it.

The cause is one `bool` answering two different questions. `App::scroll_vendor`
returned *whether the row offset changed*, and `WindowEvent::MouseWheel` chains
`scroll_skills() || scroll_vendor() || zoom()` — so at either end of the list the
offset stopped moving, the answer went `false`, and the chain fell through to the
camera. `App::scroll_skills` had the shape right from the start: it answers
`true` the moment the pointer is over its window, end of list or not. The vendor
now does the same.

### What is still not right about it

- **The chain conflates "taken" with "changed".** The `||` decides who gets the
  notch, and its answer is also what asks for a redraw. Those are two properties,
  and every handler that joins the chain has to know that the first one is the
  one being asked — the defect above is what happens when it answers the second.
  A third scrolling window will make the same mistake unless the two are split.
- **Only the catalogue swallows the wheel, not the whole vendor window.** The
  guard is `catalogue_contains`, so a notch over the window's frame, its buttons
  or its quantity field still reaches the camera. Skills claims its whole window;
  these two windows disagree about what a wheel over a window means, and the
  disagreement is invisible until a player's pointer sits an inch lower.
