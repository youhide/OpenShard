# The wire, read in the client's direction

How this client reaches a shard: the half of `crates/common/protocol` the server
never needed, the sans-io `Connection` and login machine in `crates/client/net`,
the transport both ends name as a parameter, and the one word that stops a
shard. The sections keep the milestone numbers the work was ordered by, because
the code and the tests still cite them.

Status and what is left are [`README.md`](README.md); the findings this work
turned up are
[`evidence/2026-08-30-the-client-backlog.md`](evidence/2026-08-30-the-client-backlog.md).

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

What this section once said was still owed — `SIGTERM`, the bytes still in the
outbox, and telling the player why the world went away — has landed on the
server's side of the word:
[`server/design_shutdown.md`](../server/design_shutdown.md).

Two smaller decisions. The shard is given the same install the window reads
(`world.client_files`), because the client predicts each step's `z` from its own
copy of the facet and two ends reading different ground is a stream of `0x21`
rollbacks that looks like a client bug. And none of this lives in `client/app`,
which is why that crate is now a library with a thin binary — the move
`crates/server/server` already made, and for the same reason: something that
wants a client should call one rather than build one.

