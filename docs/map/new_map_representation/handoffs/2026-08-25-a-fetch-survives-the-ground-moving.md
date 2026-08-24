# 2026-08-25 — a fetch survives the ground moving

Not a phase: direction **E** has five and they are built. This is the largest
entry of [its backlog](../to_the_client.md#backlog), which three handoffs in a
row recorded and none closed — **the fetch that straddles a publish**. A client
told the ground moved while ground was still arriving ended the connection and
said so; now it keeps the connection and asks again for the right squares.

```sh
cargo run -p openshard-playground -- --world-from-shard
# and then, in the game window, twice in quick succession: .setland 3 40
```

## Where it stands

```sh
cargo test -p openshard-client-net --lib            # 138, of which seven are this
cargo test -p openshard-client-app --lib            # 402
```

`cargo clippy -p openshard-client-net -p openshard-client-app --all-targets`:
silent. `cargo check --workspace --all-targets` has **one** error and it is not
this session's — `e2e/shard/tests/chunks.rs` against a `cache::Kept` a parallel
session is mid-way through introducing. Everything else compiles.

## What is new

| | |
|---|---|
| [`Fetch::abandon`](../../../../crates/client/net/src/chunks.rs) | Stop, because the world moved. One fetch in, two values out: what the shard still owes, and what to ask for again |
| `Drain` | The answers an abandoned fetch is owed, counted and thrown away. It decodes nothing — one entry per outstanding chunk, however big the facet is |
| `Restart` · `Restart::and` · `Restart::begin` | The union of what the fetch was asking about and what each publish since has named, and the fetch it becomes |
| `Pending::Draining` ([`link.rs`](../../../../crates/client/app/src/link.rs)) | The state a connection is in while the wire empties. Nothing goes out on it |
| `resume` · `begin` | Where an abandonment leaves the connection, and the one place a restart's requests are put on the wire |
| `latest` | The newest revision the shard has announced, which is what tells a stale `ChangesReply` from a current one |

## What was decided

**The abandoned fetch is not dropped, it is drained.** The obstacle the backlog
named is real and it is the whole difficulty: the shard answers a chunk request
*exactly once*, and nothing in an answer says which request it belongs to. So a
connection that abandoned a fetch and asked again immediately would have two sets
of answers on the wire for overlapping squares, and the wrong one might land
second — a world made of one square from before the edit and the rest from after,
which is exactly what `assemble`'s `MixedRevisions` exists to refuse and which no
check downstream could attribute. Hence a state and not a filter: while a `Drain`
is owed anything, this connection asks for no ground at all.

**A drain decodes nothing.** It is bytes to count, not chunks: fragments are
dropped as they arrive rather than joined, so what an abandoned fetch of Felucca
costs while it empties is at most 256 map entries rather than the megabyte of
half-assembled fragments the live fetch holds. It also handles the half
`Fetch::on_packet` never had to — **a refusal comes out of `outstanding`** —
because there a refusal ends the fetch and what it leaves behind never matters,
and here a chunk still owed after one would hold the connection shut for a packet
that is never coming.

**What to ask for again is a union, and this is the decision the fix turns on.**
The list a fetch was asking about is what moved between the world this end can
still show and the revision it was fetching; the list a publish names is what
moved from that revision to the new one. A square in either has moved as far as
this end is concerned, and **nothing will ever name it again** — the shard's next
notice is about its next patch. So the two are put together rather than the
second replacing the first, and the union is fetched at the newest revision. The
alternative — ask the shard `0xE007` again and use its answer — is a round trip
for a list this end can already compute, and it needs a revision for the window's
world that the socket thread does not have.

The union can name more chunks than one `ChangesReply` could. That is not a
violation of `MAX_MOVED`: the cap bounds a *packet*, and this is a list of things
to request in batches of `MAX_CHUNKS`. Where a union stops being narrower than
the facet is `Changes::Everything`, which absorbs whatever it meets in both
directions.

**Each of the three fetches restarts as itself.** A whole-facet fetch is
abandoned as `Everything` however few squares the publish names — nothing of that
facet is on this side, so there is nothing narrower to ask for. A fetch over a
kept world hands the world *back* and fills it in again. A fetch for the window's
world still ends in chunks. That is `Whose`, which is `Filling` after the fact
and a second enum rather than that one, because what survives an abandonment is
which of the three to start again and none of the fragments, cursors or chunks.

**A publish while the client is *asking* what moved needs no drain at all.** The
fourth case the backlog named separately, and it is different in kind: there is
no list yet to union, because the answer that would carry one is still on the
wire. What arrives is a difference to a revision the shard has already left, and
fetching against it would be `WrongRevision` one fetch later. So the reply is
recognised as stale — by the revision it carries against the newest the shard has
announced — and **the question is asked again**. One request in flight at a time,
one round trip, no state. The comparison is `!=` and not "older than": the two
numbers have to agree, and a reply from ahead of every notice this connection has
seen is as unusable as one from behind.

**A drain that is already empty restarts at once.** Not a special case worth
skipping: a fetch asks in whole requests and the window only empties, so a
publish landing in the gap between the last answer and the next request finds
nothing outstanding — and waiting for a packet that is not coming would leave
that client one revision behind for the rest of the connection.

## What is clean

The seven tests are `chunks.rs`'s own, against the fixture the rest of that
module uses, and the oracle for the restart is deliberately the same one E3's and
E4's arms use: the world the shard is holding *after* both edits, tile for tile.
What they cover is a fetch abandoned mid-flight eating exactly what it was owed
(including a refusal), the union asking for both lists, `Everything` absorbing,
a whole facet abandoned whole, the window's arm still ending in chunks, and a
second publish growing the list without naming a square twice.

**Not run: the playground.** The two lines at the top of this file are the
acceptance, and the second `.setland` has to land while the first one's chunks
are still on the wire — which on a facet of Felucca's size is a matter of typing
them one after the other, and on a small test world is a race. Everything under
it is covered by the tests above; what a run would prove is the *joining*, which
is the backlog entry below.

## What is next

E's backlog, minus its largest entry. What is left there is the untested joining
in `link::play`, a base set that could store its chunks deflated (107.5 MB →
22.4 MB, measured), and the two small cache manners — a world that moved by an
empty patch keeping its old revision, and nothing sweeping an orphaned one.

## Found along the way

**`link::play`'s wiring has a fourth decision now, and still no test.** E2, E3
and E4 all recorded this, and this session is the worst addition to it: the
publish arm is a four-way match on what the connection is in the middle of, and
two of its arms are *new states* rather than new packets. The fix is unchanged
and is now four handoffs old — a `tests/` in `e2e/playground`, which links both
ends.

**A whole-facet fetch restarts from zero on every publish, and that is a
starvation this session did not close.** A client with no kept world fetches
7,168 chunks over seconds; a publish at any point in that abandons all of them,
because a chunk that already arrived carries the old revision and `assemble`
refuses a mixed set — even though a chunk the publish did not name has *identical
content* at both revisions. So an operator publishing faster than a facet arrives
can keep a first-time client from ever finishing. What would retire it is a way
for `assemble` to take a set whose revisions differ but whose content is known
not to: the shard would have to say which chunks a revision left untouched, which
is what `changes_since` already computes and what the arriving chunk's own
`revision` field cannot express. Rare, and worth writing down because the
mechanism that makes the common case free — E3's cache — is exactly what a client
in this state does not have.

**A fetch applies at its end and never as chunks arrive, and `abandon` now
depends on that.** `Filling::Held` hands the kept world back untouched, which is
only right because nothing has been written into it yet — `finish` is where
`take_chunks` happens. If a future streaming apply ever moves that earlier, the
world handed back here would be half a revision ahead of its own number, and
`Fetch::over`'s next `WrongRevision` would be the least of it. Worth a line
because a streaming apply is an obvious optimisation for a client watching a
blank window.

**A `Drain` eats every chunk packet, and recognises fewer than it eats.** A chunk
of a facet nobody asked about, or a square that already completed, is consumed
and not counted. That is deliberate — nothing on this connection has asked for a
chunk since the abandonment, so a chunk packet is an abandoned answer by
construction, and one that is not is still nothing the window can be told about.
The failure it protects against is the opposite one: a drain that ended early
would let the restart's answers race the abandoned ones, which is the whole thing
it exists to prevent.
