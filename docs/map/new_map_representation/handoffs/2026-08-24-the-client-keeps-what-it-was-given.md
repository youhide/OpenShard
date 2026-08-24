# 2026-08-24 — the client keeps what it was given

Direction **E**'s fourth phase. The session before this taught a client with no
map files to take the whole facet off the wire; this one makes it pay for that
**once**. A second start over an unchanged world asks for no chunks at all, and
one over a world an operator has edited asks for exactly the chunks that edit
touched.

```sh
cargo run -p openshard-playground -- --world-from-shard   # twice: the second is instant
```

The world is kept in the working directory as `openshard-world-<id>-<facet>.osbase`
— a base set of ours, read back through `openshard_basemap::load`, which is E0's
reader. Deleting the file is how you ask for the whole facet again.

## Where it stands

```sh
cargo test -p openshard-client-net --lib            # 125, of which cache and chunks are 15
cargo test -p openshard-world --lib chunks_tests    # 12
OPENSHARD_CLIENT="/path/to/Ultima Online Classic" \
    cargo test -p openshard-e2e-shard --test chunks -- --ignored --nocapture
```

Green: three e2e tests in 9.6 s, the third of which is E3's own — three
connections to one shard, with a `.setland` between the second and the third.

### The open question, closed by measurement

`to_the_client.md` left one decision to this phase by name: **is the cache
rewritten whole when the world moves, or does it grow an append-only tail of
newer chunks?** It asked for a measurement rather than an argument. On the
shipped Felucca — 7,168 chunks, 102.6 MiB, `felucca.osbase`:

| | |
|---|---|
| `openshard_basemap::load` (read, decode, assemble 29.4M tiles) | **0.12–0.19 s** |
| `openshard_basemap::write` (encode 7,168 chunks, 102.6 MiB out) | **0.10–0.13 s** |
| flushing that to the device afterwards | below 0.01 s |

*Measured on ext4 on this machine's NVMe, deliberately not in `/tmp`, which is
tmpfs here — a cache written to RAM measures nothing about a cache written to a
disk.*

**So it is rewritten whole.** A tail would save a tenth of a second per edit and
cost a version 2 of the base set format, a second read path, and a rule for when
to compact. The number retires it; if a facet ever gets big enough for that to
change, the measurement is repeatable in ten lines.

The other number is the one E3 exists for: **0.12 s to read the world back
against seconds to fetch 21.3 MiB**, and against nothing at all on the wire.

## What is new

| | |
|---|---|
| [`client/net/src/cache.rs`](../../../../crates/client/net/src/cache.rs) | The kept world: where it lives, reading it, writing it. `CacheError` — five ways there is nothing to take, **none of them fatal** |
| [`WorldId`](../../../../crates/common/protocol/src/world.rs) | Which *world* a facet's ground is, beside `Facet` because it is the same kind of fact |
| `openshard_basemap::identity_of` | FNV-1a-64 over a base set's own bytes. One caller: the shard's boot |
| `WorldNotice.world` | `Option<WorldId>` on E1's packet, behind a presence byte. `None` is a facet out of an install — ground a client must not keep |
| `0xE007 ChangesRequest` · `0xE008 ChangesReply` | "What has moved since this revision?", answered exactly once with `Changes::These(..)` or `Changes::Everything` |
| [`World::changes_since`](../../../../crates/server/world/src/tick/chunks.rs) | The shard's answer, computed out of the patch log — a pure read, testable without a connection |
| [`chunk::apply`](../../../../crates/common/map/src/chunk.rs) | `assemble`'s other half: some chunks put back into a world somebody already holds. E4 needs it too |
| `Fetch::over` | The same transfer with a shorter list, ending in `apply` instead of `assemble` |
| `WorldHome.identity` | What the shard tells a client its world is called |

## What was decided

**A cache is filed under the *world*, not under the shard.** The obvious key is
the address dialled, and it is wrong twice. Our own launcher — the playground —
dials nothing at all, so two runs over two different `openshard.toml` worlds
would share one file; and a shard that re-imports its facet serves a different
world at the same address whose first revision is 1 again, so the revision beside
it would agree and the client would draw a world nobody built. That is the
failure `openshard_basemap::patches` already guards three times, and a cache is a
fourth place to have it.

So the shard names its world once, in the notice: a hash of the base set's own
bytes, computed at boot. The client never computes one — it is opaque here — and
the name goes in the *file name*, so two worlds are two files and one world is
one file from any address. A facet the shard cannot name is not kept at all,
which is the ordinary state of a shard running on a UO install.

What the pair (identity, revision) does not separate is two shards whose logs
forked from one base set at the same revision. That is a log taken apart by hand,
and the append-only rule is what makes it not a thing that happens.

**The cache is read on the shard thread, after the notice — not at `run`.** The
E2 handoff wrote that "a cache hit is not a new startup path, it is `BaseSet`
with a path the client chose", and that turns out to be half right: the path
cannot be chosen before login, because the identity that names it arrives *in*
the notice. So the startup order E2 built stays exactly as it is — the window
begins with no ground and grows one through `Ground::set_base` — and what a cache
changes is only how long that takes: a tenth of a second instead of seconds. The
two absent artifacts stay absent for E2's reason.

**What moved is the patch log's answer, and it is computed per request.** A facet
in memory has no memory of which tiles moved to get it there; the only other way
to compute a difference is to hold both worlds, which is exactly what the client
has and the shard has not. So the log is read on the way past — one record per
committed edit, asked once per connection. The alternative is an index in memory
invalidated on publish, which is the cache `chunks.rs` already refuses to keep
for the same reason.

**"Take the facet again" is one answer for four different facts.** A revision the
shard never published, one older than its base set, a log it could not read, and
a difference too big to name in one packet. They are one variant because the
client does the same thing with each; what separates them is a line in the
shard's own log. And the cap is what a packet holds — 4,096 chunks is 16,401
bytes against 18,000 — because past that point the list has also stopped being a
saving.

**An empty list is knowledge, not a refusal.** `Changes::These(vec![])` is a
world that has not moved, and it is what a client one revision behind an *empty*
patch is told. Collapsing it into `Everything` would send a client to fetch a
facet it already has.

**Every chunk of an incremental fetch is checked against the revision the
difference was asked about.** The list of chunks is a statement about two
particular revisions, so a publish landing between the answer and the fetch makes
it a list of the wrong squares — other chunks moved too, and nothing at this end
was told which. A whole-facet fetch has no equivalent check because `assemble`
compares the set with itself; this is that rule one level up, and it is what
`FetchError::WrongRevision` says.

**`chunk::apply` rebuilds rather than splices.** A block's statics are one run in
a facet-wide vector, so a chunk whose item count changed moves every static after
it — there is no splice that is not a copy of the tail. Since the copy is
unavoidable it is made once, through the same `WorldMap::from_parts` `assemble`
and the `.mul` importer both end in, so a world grown a chunk at a time cannot
have a different idea of the per-block order from one read whole.

**A cache that will not read or will not write is a line, not a lost
connection.** Every variant of `FetchError` is terminal — a client told to take
the shard's ground and handed something that is not a facet has nothing to draw.
`CacheError` is the opposite: each variant costs the fetch this mechanism exists
to avoid, and nothing else. That is why they are two types and not one.

## What is clean

`cargo check --workspace --all-targets`: silent. `cargo clippy --all-targets` on
the nine packages this touched: silent. Their tests: green (448 + 125 + 649 + the
rest). The three `#[ignore]`d chunk tests and `map_edit`'s pass against a real
install. `rustfmt` on the files this session wrote — not `cargo fmt --all`,
because the tree carries several parallel sessions' work and it would reformat
theirs.

**Not run: the playground itself.** The loop in `link.rs` that joins the cache to
the fetch is the one seam with no test, for the reason the E2 handoff already
recorded, so `cargo run -p openshard-playground -- --world-from-shard` twice is
the acceptance that is still owed.

## What is next

**E4 — a publish reaches a connected client.** `0xE005 PublishNotice`, sent from
`mapedit::commit` after the log has accepted the patch, naming the revision and
the chunks it touched. The client refetches those chunks, applies them, rebakes
what is derived over them and drops its coarse graph.

Most of what E4 needs now exists: `chunk::apply` is the primitive, `Fetch::over`
is the transfer, and `cache::write` is where the new world is kept. What is new
in E4 is that all three happen while the window is *drawing* — E3 does its work
before the window has ground at all — so the invalidation of everything baked
over the facet is E4's real content, and it is the first time a second
`Update::Ground` is a thing that can happen.

E4 also inherits the recovery this phase left: a fetch that straddles a publish
ends the connection, and the honest answer is to start again at the new revision.
Both fetches now fail that way for the same reason, and E4 is where a client
learns that a publish happened at all.

## Found along the way

**`link::play`'s wiring still has no test, and E3 added to it.** Carried from E2
and now larger: the decision between "keep what we have", "ask what moved" and
"fetch the facet" is `decide` in `link.rs`, and the packet loop's new `Asking`
state is beside it. Both are private to `openshard-client-app`, which cannot see
a shard. The e2e test writes the same three branches out by hand, so what is
untested is only the *joining* — but that is where a state machine goes wrong. The
smaller fix is still a `tests/` in `e2e/playground`, which already links both.

**A world that moved by an empty patch keeps its old revision in the cache.** The
client is told `These([])`, keeps the world it has — which is right — and does not
re-stamp it, so the next connection asks the same question and is told the same
thing. It costs one packet per connection on a world nobody will ever have, and
the fix is a `MapSnapshot` that can be re-stamped without being rebuilt.

**`world_of_ours` is now written twice and a half.** `map_edit.rs` and
`chunks.rs` still hold the same twenty-five lines, and this session added
`say_and_hear` to the second one as a copy of the first's. The lift is a
`tests/common/mod.rs` those two share — the same entry E2 left, one caller worse.

**The cache directory is the working directory and nothing can move it.** No flag
and no environment variable, on the grounds that `client_ui.toml` sets that
precedent and nobody has asked. A read-only checkout would be the case that
changes it, and `bake::artifact_path`'s `OPENSHARD_NAVIGATION` is the shape it
would take.

**Nothing sweeps an orphaned world.** A shard that re-imports its facet leaves
the old file behind under the old identity, and 102 MiB is not nothing. A client
knows the names of the worlds it has kept — they are in one directory — so the
sweep is a rule about how many to keep rather than a mechanism that needs
inventing.
