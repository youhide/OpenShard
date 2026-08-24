# 2026-08-24 — the client's world becomes a parameter

Direction **E** opened, and its first phase built. The session before this one
let a running shard edit its own ground; this one starts on the half that makes
anyone *see* it — and the first thing that needed was for the client to stop
being welded to `map0LegacyMUL.uop`.

Two commits' worth of work in one: a plan for E with the pipe decided, and E0.

## Where it stands

```sh
cargo run -p openshard-client-app -- --client "$UO" --base-set felucca.osbase
```

The client reads its ground out of a base set of ours and the patch log beside
it, and the navigation graph and the interiors flood are read from beside that
base set rather than from the install. `openshard.toml`'s `world.base_sets`
reaches the playground's window as well as its shard, so the one process cannot
have its two ends on two different worlds.

**The pipe is chosen**, and the surprise is that it already existed:
[`access.rs`](../../../../crates/common/protocol/src/access.rs#L75) reserves
`0xE000` as *"where every other subcommand this engine invents will live"*, with
the argument for why a stock client survives one already written. So E is `0xBF`
subcommands, on the game connection, and not a second stream.

### What is new

| | |
|---|---|
| [`to_the_client.md`](../to_the_client.md) | E's executable plan: the measurements, the decisions, five phases E0–E4 with a "done" each, and what it must not do |
| `bake::WorldSource` · `bake::FacetWorld` · `bake::SourceError` | *Read a facet from the source named, refuse a file that is another facet, and say where things derived from it live.* One function; it was written three times |
| `bake::file_name_of` | Public, because the interiors flood stamps the same base set and two spellings of "what is this input called" are two artifacts that disagree about one file |
| `WorldConfig::base_set(facet)` | The `world.base_sets` lookup, once, instead of a `FacetKey` built at each call site |
| `interiors::stamp_of(dir, &world, facet)` | Takes the world rather than a revision — which is what decides whether the stamp names the install's map files or the base set |
| `interiors::build(dir, source, facet)` | And hands back the `FacetWorld` it flooded, so the caller stamps what actually ran |
| `openshard-interiors-bake --base-set`, `openshard-interiors-inspect --base-set` | The navigation bake already had one |
| `openshard-client-app --base-set` / `OPENSHARD_BASE_SET` | And `run` grew the parameter that carries it |
| `e2e_shard::window_base_set` | What the playground's window should read, given the config its shard is about to run on |
| `movement/tests/facet_source.rs` | Four tests, no install needed: the base set arm, the install arm's refusal, the wrong-facet refusal, and a committed patch being part of what the source resolves to |

## What was decided

**The pipe is `0xBF` in the `0xE000` range, and the chunk is deflated first.**
Both halves are measurements rather than preferences.

A chunk record is 12,568 bytes before a single static — Felucca's mean is 15,001
and its max 45,382 — so **21.3% of chunks do not fit in an 18,000-byte packet**.
That is the fact that argued for a second stream over `Dial`. Deflated at level
6 the same 7,168 chunks are 21.3 MiB in total, median 1,739 bytes, **max 16,050,
and none over the cap**. An ocean chunk goes from 12,568 bytes to 56.

So a second stream buys nothing and costs a port, a third method on `Dial` that
every implementation including the in-process one has to grow, a second
authentication, and two streams with no order between them. And the failure
modes are asymmetric: a private packet *id* desynchronises a stock client that
ever received one, because framing has no length rule for it and there is no
resynchronising a UO stream. An unknown `0xBF` subcommand is skipped.

**A chunk's `revision` field is not the world's revision**, and E3's cache
depends on the difference. After a publish every chunk re-cut from the facet
would carry the new number while only the touched ones changed content, so a
cache keyed on that field throws away 7,167 good chunks per one-tile edit. The
world's revision is the base set header's.

**`--base-set` on the client is not a stepping stone.** It is
`openshard_basemap::load`, which is what E3's cache is read back through. One
reader, one format, one revision rule, whether an operator put the file there or
the client wrote it.

**The resolution is one function.** `boot.rs`'s `facet_source`, the navigation
bake's `source` and the client's new one were about to be three spellings of one
question, and the question is one where being wrong is silent: a stamp naming
`map0LegacyMUL.uop` for a base-set world *passes*, because that file still exists
with its old mtime. It went into `openshard_movement::bake` — beside the two
stamp functions that are its only reason to exist, in the crate whose package
already depends on both readers for its own binary, and which all four callers
already depend on.

**`WorldSource` is an enum and not `Option<&Path>`.** The install is a source,
not the absence of one.

**The playground's window takes the shard's base set.** After one committed
patch a base set *is* a different world from the install it was imported from,
and two ends of one process reading two different worlds is the disagreement
that playground exists to make impossible.

## What is clean

`cargo check --workspace --all-targets` builds everything except the one target
below. `cargo clippy` on movement, artscan, client-app, server, config,
e2e-shard and playground: silent. `cargo test` on movement, config, artscan,
e2e-shard, server, client-app, state and world: all green, including the four new
ones. `rustfmt` on every touched file, and `cargo fmt --all --check` is silent
apart from one file this session did not touch.

`crates/client/render/tests/frame.rs:5672` still reads `rows.start` on a
`DirtyRows` that has no such field — a parallel session's work in progress,
carried over from the last handoff and left alone. It is also why
`cargo fmt --all` was **not** run: it wants to reformat
`crates/server/world/src/tick.rs`, which is not this session's either.

## What is next

**E1 — a chunk is asked for and arrives.** Three subcommands, laid out in
[`to_the_client.md`](../to_the_client.md): `0xE002` a request naming chunks,
`0xE003` a deflated chunk in fragments of at most 8,192 bytes, `0xE004` a notice
on world entry saying which facet, what size and which revision. Only a client
that asked is answered, which is the whole of the capability negotiation.

The fragment cap is deliberate and is not the packet cap: at 8,192 bytes **4.58%
of Felucca's chunks are more than one fragment**, so reassembly is on the
ordinary path rather than being a branch that runs for the first time on a world
nobody has built yet.

**And E2 is where the real cost is.** `run` loads the facet before the window
exists and everything after it — the span bake, the coarse graph, the interiors
flood, the camera's opening `z` — is built from a map that is already there. A
world that arrives after login cannot keep that order, and
`Resources::map`'s `expect("a client that got as far as drawing opened a facet")`
is the assertion that has to become a real state.

## Found along the way

**`miniz_oxide` is already a workspace dependency and `openshard-protocol`
already uses it** — [`design.rs`](../../../../crates/common/protocol/src/design.rs#L544)
deflates a house's planes into a packet and carries the inflated length beside
the blob, inflating *with that length as a limit* on the way in. E's chunk packet
is that shape exactly, and costs no new crate.

**A base set could store its chunks deflated**: 107,528,650 → 22,363,473 bytes on
the same content. It is a version 2 of the file and touches
`openshard_basemap::write`/`read` alone, since the table already makes each chunk
independently addressable. Filed in the plan's backlog — E3's cache is the caller
that would want it most.

**UO's Huffman table gives 0.808 on raw chunk bytes and inflates a deflated blob
by 15%.** Deflate-then-Huffman is still 0.241 of raw, so the whole facet is
24.6 MiB on the wire rather than 82.8 MiB. Worth writing down because the
intuition "the stream is already compressed" is the one that would skip the
deflate.

**`World` derives `Default`** ([`world.rs`](../../../../crates/common/map/src/world.rs#L45)),
which `docs/style.md` bans, and `World::new(None)` is the named constructor it
already has. Left alone: it is one derive and a sweep of the callers, and it is
not E's.

**`Opening` is constructed with `..Default::default()` in both binaries**, which
is the failure mode the style rule names — a field added later is silently
filled in rather than named at each call. Not touched here, and the reason `run`
grew a *parameter* rather than a field on `Opening`: a new parameter is one the
compiler makes every caller answer.
