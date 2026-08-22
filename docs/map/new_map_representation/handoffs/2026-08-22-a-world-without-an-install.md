# 2026-08-22 — a world without an install

Fourth session of the day, and the first one in this track that adds a
*feature*. A0 and A were refactors with nothing new in them; this is direction
**B**, and at the end of it there is a file on disk that is a facet of
Britannia and did not come from a client install.

Landed in three commits: `7642de4c` the chunk and its encoding, `ece9ad21`
the base set and the importer, `d25cf2b4` a leftover the last handoff named.

## Where it stands

Run this once and the shard has a world it owns:

```sh
cargo run --release -p openshard-uofiles --bin openshard-map-import -- \
    --facet 0 --out felucca.osbase --verify
```

On the shipped Felucca that is **7,168 chunks, 2,906,871 statics, 102.6 MiB**,
and `--verify` reads it back and checks all 29,360,128 tiles. It takes under a
second in release.

Two new things in the workspace:

| | |
|---|---|
| `openshard-map`'s `chunk` and `codec` | The square the world is stored, cached, invalidated and transferred in, and its canonical bytes. Still opens no files. |
| `openshard-basemap` | Where our format meets a path: the file, the table of chunk offsets, the read back. It has never heard of a UO install. |

The importer binary lives in `uofiles/src/bin` and the dependency runs
**uofiles → basemap**, not the reverse. That is the shape the track wants: the
base set knows nothing of UO, and the thing that reads an install is what knows
both.

`cargo check --workspace --all-targets`, `cargo test --workspace` and
`cargo fmt --all` are silent. Clippy's ten warnings are the interiors track's,
in files this work did not touch.

## What was decided

**A chunk is 64×64 tiles, and it was a measurement.** `mechanics.md` said this
was not an opinion, so it was measured on the real `staidx0.mul` at 8, 16, 32,
64 and 128 tiles. The surprise is that the base set's *total* size is flat
across all of them — 137 to 151 MiB — so size decides nothing and what is left
is overhead against blast radius. UO's own 8×8 loses on overhead and not
narrowly: a manifest with a hash per chunk is 17.5 MiB, a ninth of the set it
indexes, and one widest-zoom rectangle pins 625 chunks against sixteen. The
argument the other way is that one wall then rewrites 18 KiB, and `overview.md`
had already refused that argument by name. Sixty-four is also the grid every
artefact derived from terrain is already keyed to. The full table is in
[`chunk.rs`](../../../../crates/common/map/src/chunk.rs)'s header and in
[`mechanics.md`](../mechanics.md).

**The order inside a chunk is the order outside it, by being the same call.**
`BlockExtent` now owns the column-major arithmetic `LandGrid` used to spell out,
and `LandGrid` asks it. So a chunk's local block order is not a second layout
that agrees with the facet's — it is the facet's, restricted. That was the
cheapest way to make sure the one thing A0 was about does not come back at a
smaller scale.

**A chunk is not always whole.** Tokuno is 181 blocks square and 181 is not a
multiple of eight, so a chunk carries its own `BlockExtent` and an edge chunk is
simply smaller. Padding a facet to whole chunks was rejected: it invents ocean a
reader cannot tell from real ocean. The test fixture is nine blocks square for
exactly this reason — three of its four chunks are edge chunks.

**The chunk reader is a second importer, not a second world.** `assemble` ends
in `Map::from_parts`, the same call the `.mul` reader ends in, so the per-block
sort every later lookup binary-searches over is imposed by the type either way.
This is what makes the acceptance test an assertion about bytes.

**The encoding is canonical, and nothing in it sorts.** One world encodes to
exactly one byte string, which rests entirely on the `(y, x)` sort being stable
one layer down. A decoder therefore refuses a trailing byte — a tail nothing
reads is a second byte string for one chunk — and refuses a packed position with
bits set that are not a position, rather than masking them into a static
standing somewhere it never stood.

**Two things were deliberately left out of the record, with reasons.** A hue
table: only 0.95% of Felucca's statics are hued, so an inline `u16` costs 5.6
MiB on a 137 MiB set, and a sparse side table would buy that back for a second
resolve on every item. A draw-order field, which `client_today.md`'s finding 10
asks for: it needs no field, because the order the items are *written* in is the
draw order — the sort is stable and `statics::pick` takes the last. A height
index is something a chunk could gain later; a reordering is what would need the
field, and that is a change to the picture rather than to the format.

**`MapSnapshot::restored` is new, and is why a base set is worth writing.** A
facet read back arrives at the revision the file recorded rather than at a fresh
one, so a bake stamped against that revision stays valid across the round trip.
`new` keeps its job — minting a first revision for a world that arrived without
one — and the two are now the only ways to make a snapshot.

**`StaticId` was not built, and does not need bytes.** It is on B's entity list
but everything that wants it is C's. The base is immutable, so "the *n*th static
of block *b* at revision *r*" is already a stable identity; what makes it stable
is the patch model, not the encoding. Adding a field now would be guessing at C.

## What is next

**B's last step: the server reads the base set instead of the install.**
`plan.md` calls this B's real acceptance test — existing movement, LoS and
harvesting tests passing unchanged over the new source. It was not started
because it is two decisions rather than plumbing, and both want an answer first:

1. **A config knob.** `WorldConfig` has `client_files`; a world from a base set
   needs its own path, and nothing should guess one from the other.
2. **It drags direction D forward.** `boot.rs:614` stamps the navigation
   artifact with `bake::stamp_of(dir, facet, …)`, which records the *install's*
   file names, sizes and mtimes. A world loaded from a base set would validate
   its navigation graph against files that are no longer the source — and would
   pass, because those files still exist and still have those mtimes. That is a
   stale bake lying to a player, which is the thing D exists to stop. The
   interim that is honest: when the world came from a base set, stamp the base
   set. The real fix is D's, and it is now one caller away from being cheap.

After that, direction C, unchanged.

## Found along the way

**The base set is smaller than the install it came from.** 102.6 MiB against
`map0LegacyMUL.uop` + `statics0.mul` + `staidx0.mul` = 115.7 MB, with the land
still at three bytes a cell and no compression anywhere. Direction G's
whole-chunk compression is measured against that number, not against 150 MiB of
resident `Map`.

**`Map` could take the CSR layout from the chunk now, and did not.**
`client_today.md`'s finding 6 — `Vec<Vec<StaticItem>>` is 120,744 allocations
and 38.2 MiB where a CSR pair is two allocations and 13.5 — is now *implemented*
in `Chunk`, which holds one flat run and a prefix sum. Nothing carries it into
`Map`, because `from_parts` and all four static accessors are shaped around
per-block vectors and `place_static` inserts into one. It is a contained
refactor of one type with a measured payoff, and it is the largest single item
left in that backlog.

**Nothing walks a base set lazily yet.** The offset table exists so that
direction G can seek to a chunk, and no reader does — `read` takes the whole
file. That is deliberate and not an oversight, but it means the table is
currently 57 KiB of untested affordance: the first lazy reader is what will find
out whether the table is the right shape.

**The `--verify` pass is the whole-facet oracle the tests cannot afford.** The
acceptance test samples the ground on a stride of 31 and compares whole blocks
of statics; the binary's `--verify` compares all 29 million tiles and takes 0.6
seconds. Anything in this format that a sample could miss should be checked by
running the binary, not by making the test bigger.
