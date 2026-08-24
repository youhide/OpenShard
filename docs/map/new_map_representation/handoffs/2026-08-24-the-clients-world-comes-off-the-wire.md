# 2026-08-24 — the client's world comes off the wire

Direction **E**'s third phase, and the one the plan called "where the real cost
is". The session before this put a chunk on the wire and drew none of it; this
one starts a client with **no facet at all**, has it ask the shard for the whole
of one, and puts the assembled world under the picture.

```sh
cargo run -p openshard-playground -- --world-from-shard
cargo run -p openshard-client-app -- --world-from-shard --account admin
```

Neither needs `map0LegacyMUL.uop`, `staidx0.mul` or `statics0.mul` to exist.
`--client` still does: the art, the hues, the multis and — the one that decides
what a tile *means* — `tiledata.mul` are not on the wire and are not going to be.

## Where it stands

```sh
cargo test -p openshard-client-net --lib chunks          # nine, 0.05 s
OPENSHARD_CLIENT="/path/to/Ultima Online Classic" \
    cargo test -p openshard-e2e-shard --test chunks -- --ignored --nocapture
```

Green: two e2e tests in 14.4 s, of which eleven seconds are one navigation bake.

### The startup order, which is what actually changed

`run` used to read the facet before the window existed, and everything after it —
the span bake, the coarse graph, the interiors flood, the camera's opening `z` —
was built from a map that was already there. Under the new arm none of that is
true, so the three of them became one `match`:

| | before | with `WorldSource::Shard` |
|---|---|---|
| the facet | `FacetWorld::read` at the top of `run` | `None`, and `Ground::new(None, …)` |
| the span bake | taken with the facet | taken at `Ground::set_base`, when the fetch lands |
| the coarse graph | loaded beside the world | absent — there is no file it could have been baked beside |
| the interiors flood | the same | the same |
| the camera's `z` | the ground under `START` | `START`'s own, and nothing draws it |

The two absent artifacts are not a regression to fix later: they are bakes of a
world this client has no file for, exactly as they are absent today for an
install with nothing baked beside it. Long routes and the interior diagnostic
each already have an off state, and both say so on the way past.

### What is new

| | |
|---|---|
| [`client/net/src/chunks.rs`](../../../../crates/client/net/src/chunks.rs) | `Fetch` — the whole facet's transfer as a state machine with no socket in it. `next_request` hands out a packet, `on_packet` takes one, `finish` is the facet |
| `chunks::FetchError` | Seven ways a fetch is not a world, each naming the chunk. Every one of them ends the connection |
| `chunks::IN_FLIGHT_CHUNKS` | 256 — four requests deep. A *pacing* choice, and the doc says why it is not `MAX_CHUNKS`'s question |
| [`client/app/src/link.rs`](../../../../crates/client/app/src/link.rs) | `GroundSource`, `Update::Ground`, and the twenty lines in `play` that drive the fetch between the login and the first packet the window sees |
| [`WorldSource`](../../../../crates/client/app/src/lib.rs) | The client's own three-armed enum, and `on_disk` is the seam to `openshard_movement::bake`'s two |
| `Resources::grounded` | The invariant `Resources::map`'s `expect` is now stated in terms of, checked at the frame and at the window's events |
| `--world-from-shard` | On `openshard-client-app` and on `openshard-playground`. Exclusive with `--base-set`; no environment variable, on purpose |
| [`e2e/shard/tests/chunks.rs`](../../../../crates/e2e/shard/tests/chunks.rs) | A second test, over **81 chunks** — more than one request may name — driving `Fetch` over a real socket and comparing the facet it ends up holding against the base set the shard booted from, tile for tile |

## What was decided

**The window keeps its ground behind a gate, not behind an ordering.** This is
the load-bearing one and the obvious alternative was available: hold the first
`Update::World` back until the facet is here, and a window never exists without
ground. It is refused because the packets that keep arriving during a 21 MiB
fetch have to go somewhere, and there are only two somewheres — the shard
thread's own unbounded buffer, or the bounded mailbox the window drains. The
window cannot drain that mailbox while it is blocked waiting for a value the
shard thread is holding back, so the second is a deadlock and the first is an
unbounded queue with a shard's whole login burst in it.

So the gap between "there is a world" and "there is ground under it" is real, and
`Resources::grounded` closes it at the two doors that can reach the map:

- **the frame** — `App::draw`, beside the `render_ready` check it already had,
  because those are two facts. `render_ready` is *the shard authorised a
  picture*; this is *there is something to make one of*.
- **the window's events** — one guard arm in `window_event`, after the three
  arms that are the window's own rather than the world's. Close, resize and
  redraw still work while the ground is on its way; a click, a key and a wheel
  do not, because every one of them ends in a tile lookup.

Everything else that reads the map is inside one of those two. The one exception
is `App::cutaway`, which a *packet* reaches through `apply_view`, and it answers
for itself: no ground, no architecture between the eye and the body, so
`Cutaway::OPEN`.

**The third arm belongs to the client.** `openshard_movement::bake::WorldSource`
is shared with the shard's boot and both bake binaries, and not one of them can
ever take a `Shard` arm — a shard has no shard to ask, and a bake reads the world
it is baking. An arm they can never take is an arm all three would have to write
a `match` for. So `openshard_client_app::WorldSource` has three and converts at
one seam, `on_disk`, where a fourth file arm added to either type is a compile
error rather than a client quietly reading the install.

**A request is full, or it is the last one.** The in-flight window is a count of
*chunks*, so one chunk coming back is room for exactly one going out — top up per
chunk and Felucca is four full requests followed by 6,912 naming one chunk each.
Waiting until there is room for a whole request leaves 192 chunks on the wire
while the next one goes out, and the tail of the facet is the one short request.
It cannot deadlock, because the window only ever empties.

**A chunk that arrives is checked against the chunk that was asked for.** The
check `join`'s own doc leaves to the caller. It has to be made here because a
chunk is self-contained and names itself: a swapped pair is indistinguishable
downstream from a missing one, and `assemble` would refuse the facet for the
blocks the duplicate did not cover while naming the innocent chunk. There is a
test that swaps two, and it is the reason the forged-fixture route into every
other test is closed.

**A chunk nobody asked for is an error, not a duplicate to ignore.** The shard's
rule is that every chunk named is answered exactly once; this end's is the mirror
— a chunk leaves `outstanding` when it is whole and nothing puts it back — so
"was this asked for" and "is this outstanding" are one question. `Fetch` is
*taken* when it completes for the same reason: a fetch left sitting there would
refuse the `ChunkData` E4 sends after a publish as one nobody asked for.

**Every failure of the fetch ends the connection.** A refusal, a join that does
not join, a record that will not decode, a set that does not assemble. A client
told to take the shard's ground and handed something that is not a facet has
nothing to draw and no second source, so the honest answer is one line naming the
chunk and the reason, through the `Update::Lost` the window already shows.

**`Update` stopped being `Clone`.** `Update::Ground` carries a `MapSnapshot`,
which has one owner per process by construction. Nothing had ever cloned an
update; this is what stops one from starting.

## What is clean

`cargo check --workspace --all-targets`: silent. `cargo clippy --all-targets` on
`client-app`, `client-net` and `e2e-shard`: silent. `cargo test` on those three:
green (398 + 118 + the whole non-ignored e2e suite), and the two `#[ignore]`d
chunk tests pass against a real install. `cargo fmt` on those three packages only
— not `--all`, because the tree carries several parallel sessions' work and it
would reformat theirs.

## What is next

**E3 — the client keeps what it was given.** The 21.3 MiB is paid once: what
arrived is written as a base set of ours under a path derived from the shard's
identity and the facet, and read back through `openshard_basemap::load`, which is
E0's reader. On connect the client compares its cache's revision against
`WorldNotice`'s and asks only for what moved.

What E2 leaves for it, ready: `Fetch` already walks the facet in `chunks_of`
order, which is the order a base set is written in — so a cache can be written as
the chunks arrive rather than assembled and then re-cut. `WorldNotice` already
carries the revision to compare against. And `WorldSource` already has the arm
that reads a file, so a cache hit is not a new startup path, it is `BaseSet` with
a path the client chose.

The open question E3 has to close is still the one the plan states: whether the
cache is rewritten whole when the world moves, or grows an append-only tail. 102
MiB rewritten for a one-tile edit is the cost that decides it, and it should be
measured.

## Found along the way

**`link::play`'s own wiring has no test, and it is the one seam this session
added that does not.** Everything under it is covered — `Fetch` against a fixture,
the whole loop against a real shard — but the twenty lines in `play` that join
them are checked only by running the playground. The obstacle is structural:
`link` is private to `openshard-client-app`, and that crate cannot see a shard;
`openshard-e2e-shard` can see both ends and cannot see `link`. The smaller of the
two fixes is a `tests/` in `e2e/playground`, which already links both.

**`world_of_ours` is written twice**, in `map_edit.rs` and in `chunks.rs` — the
same twenty-five lines, differing only in whether statics are placed. This
session generalised `chunks.rs`'s copy by a `blocks` argument rather than adding
a third; the lift is a `tests/common/mod.rs` those two share.

**A fetch that straddles a publish fails the whole facet.** `assemble` refuses
`MixedRevisions`, which is right, but the client's answer is to end the
connection. The recovery — start again at the new revision — belongs with E4,
which is where a client learns a publish happened at all.

**The window is blank for the length of the fetch and only the terminal says
why.** E3 makes the common case instant, so this may never be worth a picture; if
it is, the state already exists.
