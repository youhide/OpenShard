# 2026-08-22 — the step is a packet where it is encoded

Third session of the day, and the last of the items
[`snapshot.md`](2026-08-25-one-world-one-door.md) left open inside its own scope. Nothing in
either phase changes; what moved is where the "these bytes are one whole
packet" claim is made.

## Where it stands

Directions A0 and A are built, and **`snapshot.md` now has nothing open in it**.
The one item its phase-2 list still named — `Walk::step` returning `Vec<u8>` —
is closed.

- **`Walk::step` returns `Result<FramedClientPacket, NotSent>`.** The previous
  session put the type on `Command::Send`, `Link::step` and `Link::resync`; the
  step's *producer* kept handing out bare bytes, so `ui_command` re-derived the
  claim with an `expect` beside the call. The encoder is the one place that can
  make it without checking anything, and it is where it lives now.
- **`ui_command::walk` lost that `expect` and its import.** It passes what
  `Walk::step` gave it straight to `Link::step`.
- **`dst.rs`'s simulated wire carries the packet, not a `Vec<u8>`.** Its queue
  is the mpsc plus the socket, and `deliver` opens the wrapper at the read —
  the same single unwrap point `link::play` has in the real client.
- **The four `crates/e2e/shard` tests send `step.bytes()`.** That is the other
  socket, and the only other place the wrapper is opened.
- **`link.rs`'s two doc comments stopped being wrong.** `Command::Send` said
  `Link::step` and `Link::resync` were "the only two constructors of one", and
  `Link::step` said the check belonged to "the caller, who is the one holding
  the connection's `ClientVersion`". Neither survives the move.

## What was decided

**`Walk::step` passes `None` for the version, and does not take one.** `0x02`
is a fixed seven bytes for every client version — `client_packet_length` says
so — and it is the one packet on this seam that is *not* version-dependent, so
threading a `ClientVersion` through `Walk` would buy an argument that could only
ever be ignored. `Link::resync` already had exactly this reasoning written out
for `0x22`; this is its second instance rather than a new decision.

The alternative considered and refused was an unchecked constructor on
`FramedClientPacket` for "bytes I just encoded myself". It would have made the
`expect` disappear rather than move, and it would be a door into the type that
does not check — which is the whole of what the type is for. An `expect` on the
checked constructor, at the encoder, states the invariant where it is true and
costs one framing pass on a seven-byte buffer per step.

**`dst.rs`'s queue holds the packet rather than bytes.** Both readings are
defensible — the queue models a channel *and* a socket — and the packet was
chosen because it makes the simulation have the same number of unwrap points as
the client it simulates: one, at the read. A queue of `Vec<u8>` would have put
the unwrap at the *write*, which is not where the real one is.

## What is next

Direction [B](2026-08-25-seven-directions.md#b--our-own-chunk-format-and-a-uo-importer), unchanged.
`snapshot.md` no longer carries anything a session could pick up on its own.

## Found along the way

**The workspace does not build from the working tree, and it is not this
diff.** A parallel session is mid-refactor in `client/render` —
`chunk_cache.rs` is untracked and `composite.rs` names fields its
`CompositeWorkQueue` no longer has — so `openshard-client-app` and everything
above it cannot be type-checked while that stands. `openshard-client-net`,
`openshard-protocol` and `openshard-e2e-shard` are green with these changes and
carry every consumer of `Walk::step` except `ui_command` and `dst.rs`, whose
changes are a queue's element type and two renamed locals. Whoever next runs a
full suite here should know the red is upstream of this track.
