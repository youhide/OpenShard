# 2026-08-25 — a publish reaches a connected client

Direction **E**'s fifth and last phase, and it closes [direction C's own
"done"](../plan.md#c--patches-and-the-resolved-snapshot) with it. Every phase
before this one was about a client *starting*: the world as a parameter, the
world off the wire, the world kept. This one is about a client that is already
drawing — an operator types `.setland 3 40`, and the tile under them changes
colour on every screen standing on that facet without anybody reconnecting.

```sh
cargo run -p openshard-playground -- --world-from-shard
# and then, in the game window: .setland 3 40
```

## Where it stands

```sh
cargo test -p openshard-protocol --lib               # 450
cargo test -p openshard-world --lib chunks_tests     # 14, of which two are E4's
cargo test -p openshard-client-net --lib             # 127
cargo test -p openshard-map --lib world              # World::take_chunks
OPENSHARD_CLIENT="/path/to/Ultima Online Classic" \
    cargo test -p openshard-e2e-shard --test chunks -- --ignored --nocapture
```

Green: **four** e2e tests in 9.9 s, the fourth of which is E4's own — one
connection, a `.setland` said over it, and the publish notice caught coming back
on the same socket.

## What is new

| | |
|---|---|
| `0xE005 PublishNotice` ([`chunks.rs`](../../../../crates/common/protocol/src/chunks.rs)) | The number E1 left free. `ChangesReply`'s body under a second subcommand: a facet, the revision it moved to, and what moved |
| [`mapedit::announce`](../../../../crates/server/world/src/mapedit.rs) | The shard's half — one packet per connection on the facet, after the log has taken the patch |
| `Fetch::moved` · `Fetched` | The same transfer with the world one thread further away. `finish` now answers with a world **or** the chunks of one |
| [`World::take_chunks`](../../../../crates/common/map/src/world.rs) | `publish`'s counterpart on the client's side: squares somebody else cut, and no patch to check them against |
| `Ground::take_chunks` | The same with the span bake taken in the same statement, exactly as `Ground::publish` is |
| `Update::GroundMoved` | What crosses the seam to the window, and the first update that invalidates something already drawn |
| [`App::ground_moved`](../../../../crates/client/app/src/net_command.rs) | The window's half: the chunks in, and every picture of the ground they replaced out |

## What was decided

**The notice goes to everyone standing on the facet, and there is no
subscription.** The obvious alternative — remember which connections have sent a
`0xE002`, and tell only those — is wrong for the reason `WorldNotice` is sent
unasked in the first place: *a client cannot ask about something nobody told it
happened*. And it fails hardest exactly where E3 works best: a client whose kept
world is already at the shard's revision asks for **nothing at all** on the way
in, so a subscription built out of "who asked" would leave the client with the
best cache the one that never hears about an edit. A stock client reads `0xBF`'s
length out of the envelope and drops the subcommand; a client of ours drawing its
own disk's facet ignores it, because the ground on that screen is not this
shard's to move.

**The two packets that say what moved are one encoder.** `ChangesReply` and
`PublishNotice` carry the same three facts for two different reasons — an answer
and an announcement — so the body is written once and the subcommand is what
differs. `Changes::Everything` then means the same thing in both, and in a notice
it can only ever have one of the reply's four reasons: *more chunks than a packet
can name*. A test writes both and compares the bodies byte for byte, because the
one mistake a shared encoder makes possible is writing one subcommand and reading
the other.

**The chunks cross the seam, not the world.** This is the decision the phase
turns on. By the time a publish reaches a client that is drawing, the facet
belongs to the *window* — `Update::Ground` handed it over a whole fetch ago, and
a `MapSnapshot` has one owner per process by construction — so the thread that
owns the socket has nothing left to apply them over. Hence `Fetch::moved`, which
ends in `Fetched::Chunks`; hence `World::take_chunks` at the far end, which is
`chunk::apply` with the world's own facet and revision bookkeeping around it.

`finish` answering with an enum rather than a second terminal method is the same
decision one level down: a fetch has two kinds of caller, they are not
interchangeable, and a method that panicked for the arm it was not built for
would be a contract the compiler cannot see.

**The kept file is left at the revision it was written at.** Rewriting it would
mean the world coming back across that seam to be written from — the only copy is
the window's, and the window is a frame loop. What not rewriting costs is one
small fetch on the next connection, which is *exactly the mechanism E3 built*: the
next start asks what moved, is told these same chunks, and writes the file then.

**The invalidation is by block, except the radar, which is by revision.** That
asymmetry is the caches' own rather than a choice: the composited pictures are
keyed by where they are, so what is dropped is named by the blocks the chunks
cover — one rectangle per chunk, so that two edits at opposite ends of a facet do
not drop everything between them. The radar's products carry the source revision
in their key, so naming the new one makes all of them unreachable at once while
`select_ready`'s stale-exact path keeps a minimap from blinking empty. `RadarCache`
was built with that field and no writer for it — *"this path has no production
writer today, the client's `WorldMap` cannot change at runtime"* — and this is the
writer it was waiting for.

**The coarse graph is dropped and not rebuilt.** Eleven seconds of flood. It is
the same trade the shard already makes when it publishes, and the same answer it
gives its operator: long routes fall back on the bounded search until the client
reconnects, which is when a graph is looked for beside the kept world again.

**A publish that lands while ground is still arriving ends the connection.** The
answers to the fetch in flight are already on the wire at the revision the publish
has just moved past, and nothing tells them apart from the answers a restarted
fetch would ask for. So it ends *there*, naming the publish, rather than seconds
later inside `assemble` naming a mixed set of chunks. That is not a recovery and
is not pretended to be one — the recovery is drain-and-restart, and it is written
up in the backlog with the three pieces it needs.

**A second `Update::Ground` is now a thing that can happen**, and it invalidates.
`Changes::Everything` is answered by taking the facet again, so the arm that used
to be able to say "nothing has been drawn yet, so there is nothing to throw away"
now asks whether it is replacing a facet and, if it is, clears the whole of each
cache the block-wise path clears in part.

## What is clean

`cargo check --workspace --all-targets`: silent. `cargo clippy --workspace
--all-targets`: silent except for one file a parallel session is mid-way through
(`openshard-font-upscale`). Tests: 450 + 652 + 127 + 88 + 160 + 402 green, and
the four `#[ignore]`d chunk tests pass against a real install. `rustfmt` on the
files this session wrote — not `cargo fmt --all`, because the tree carries
several parallel sessions' work and it would reformat theirs.

**Not run: the playground itself.** E4's own acceptance is the two lines at the
top of this file — start it, type `.setland 3 40`, watch the tile under you
change colour — and it is a GPU path with a person at the keyboard. Everything
under it is covered: the shard's half by `chunks_tests`, the wire and the fetch by
the e2e test, the apply by `openshard-map`'s own. What the run proves that none of
them can is the *window* half — `App::ground_moved` — since no test here has a
window.

**Three client files are uncommitted.** `link.rs`, `net_command.rs` and `app.rs`
carry E4's client side *and* a parallel session's in-flight HUD work, which spans
seven more files this session did not touch; committing the three alone would not
build, because `App::navigation` lives in one of the others. They are green in the
working tree and are for whoever finishes that change to land. Everything else —
the protocol, the shard, `openshard-map`, `openshard-movement`,
`openshard-client-net` and the e2e test — is committed.

## What is next

E has no sixth phase. What is left is its
[backlog](../to_the_client.md#backlog), and three entries are worth naming here.

**The fetch that straddles a publish.** The recovery E2 found, E3 handed on and
E4 did not close — a client is now *told*, which is what a recovery needs, and it
still ends the connection. The shape it wants is drain-and-restart: keep the
abandoned `Fetch` beside the new one, route chunk packets to it while it is still
owed any (`outstanding.len()` is exactly how many), discard what completes, and
then start again. It needs a `Fetch::discard` that eats a packet without decoding
it — a refusal has to come out of `outstanding` too, which `on_packet` does not do
— a way to get a held world back out of a `Fetch` for E3's arm, and a rule for a
publish that lands while a client is *asking* what moved, where the reply is
already stale and the honest answer is to ask again rather than fetch against it.

**`link::play`'s wiring has grown a third decision and still has no test.** E2 and
E3 both recorded this. E4's arm is the one that runs *while the connection is
doing something else*, which makes its ordering against a fetch in flight the
hardest part of the file to get right by reading. Same fix, one caller worse: a
`tests/` in `e2e/playground`, which links both ends.

**A base set could store its chunks deflated** — 107.5 MB → 22.4 MB on the same
content, measured. It is a version 2 of the file and touches
`openshard_basemap::write`/`read` alone. Not E's, but E3's cache is the caller
that would want it most, and E4 has just made the file something a session writes
more than once.

## Found along the way

**A publish costs the window a whole facet's rebuild, and nobody has measured
it.** `chunk::apply` rebuilds rather than splices — a block's statics are one run
in a facet-wide vector, so a chunk whose item count changed moves every static
after it — and `Ground::take_chunks` rebakes the span index over the result,
which is 0.07 s on Felucca. Both happen on the *event-loop thread*, on the frame
the edit lands, so a one-tile `.setland` is a visible hitch on a facet that size.
Paid once per publish by whoever is watching, which is why it is a note rather
than a defect. What would retire it is what direction D already wants for the
shard — a span layer that rebuilds in pieces — plus an `apply` that can splice a
chunk whose static count did not change.

**A quarantined composite block is never un-quarantined.** `CompositeCache`
permanently marks a block whose ground `FlatGroundBlock::inspect` refused, on the
stated grounds that "map terrain is immutable for the lifetime of this cache" —
which is exactly what this phase stops being true. A publish that *flattens* a
block leaves it on the direct path for the rest of the session, which is slower
and not wrong; the reverse is handled, since the invalidation drops the composite
and the next preparation rejects it again. What is missing is one line of API.

**The client never takes up an interiors bake for a world off the wire.**
`Update::Ground` takes up the navigation graph and not the building flood — that
artifact is looked for only in `run`, from an install path — so
`Resources::interiors` is `None` for the whole of a `--world-from-shard` session.
Worth writing down because the day it *is* taken up, it is a third thing a publish
makes stale, and this phase's invalidation says nothing about it.

**`Update::Ground` did not invalidate, and nothing had noticed because it could
not arrive twice.** Found while writing E4's whole-facet arm rather than by a
test: the doc on that arm argued at length that it needed no invalidation because
no frame had been drawn yet, which was true right up until `Changes::Everything`
gave it a second caller. The general shape is worth keeping in mind — *"it arrives
once" is a property of the callers, not of the code*, and it stops being true
without anything in the arm changing.
