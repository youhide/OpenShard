# A world keeps its name, and its chunks say what they are

S1 of [`what_a_change_costs.md`](../what_a_change_costs.md), which is the plan
this session opened for what era S has left. Base sets are now version 2.

## Where it stands

**Built.** `OSBS` version 2 carries the three things a version byte should only
ever be spent on once:

- **Deflated chunks**, through `openshard_protocol::chunks::deflate` — the pair
  the wire has used since E1, now `pub` and used by both, so a chunk cannot be
  packed at two levels by two callers. The fixture's four chunks store fewer
  bytes than their records; on Felucca the measured figure is 107,528,650 →
  22,363,473.
- **A manifest**, twelve bytes a chunk: FNV-1a of the chunk's **record** and the
  length that record inflates to. It is read back — a chunk that does not inflate
  to its declared length is `BaseError::NotDeflated` and one that does not hash
  to its entry is `BaseError::HashMismatch`, both naming which chunk.
- **A world identity in the header**, minted once and carried afterwards.

`identity_of` now reads the header and nothing else, so it costs a seek where it
used to read the whole file.

Every caller of `write` names which of the two it is doing, because the third
argument is `Identity::Mint` or `Identity::Keep(WorldId)` — `openshard-map-import`
mints and prints, the client's chunk cache keeps what the shard named, and every
test mints.

**A version 1 base set does not load.** Re-import is the migration, and that is
the whole of it: nothing in a base set is not reproducible by writing it again.

## What was decided, and against what

- **One version bump, not three.** Deflation, the manifest and the identity were
  each worth a bump on their own. A version byte names a layout, so doing them
  separately would have been three migrations of every base set on disk for one
  quarter's worth of work.
- **The manifest hashes the record, not the stored bytes.** What a reader
  downstream keys on is a chunk's *content*; two builds of a compressor are
  allowed to disagree about the bytes they pack it into, and a file written by
  one and refused by the other would be a defect nobody could see.
- **The identity is minted over the manifest, not over the file.** Two reasons
  and they are one: a hash of the file cannot be minted from inside the file it
  is written into, and a hash of what the chunks hash to does not move when the
  compressor under it is upgraded. A re-import of the same facet on another
  machine still mints the same world, which is the property E3 chose a hash of
  the bytes for in the first place.
- **`write` asks whose world it is writing rather than deriving it.** The
  alternative — mint on every write — is what made a squash impossible: it would
  rewrite a world's bytes without changing the world and send every client back
  to fetching a facet nothing moved in. A cache is the same case from the other
  end: it is somebody else's world in a file of ours.
- **`inflate` asserts the declared length rather than using it as a ceiling.**
  `decompress_to_vec_zlib_with_limit` accepts a stream that stops short, and a
  chunk record short of its own header decodes into a refusal a long way from the
  blob that caused it. `join`'s own doc already claimed this check; now it is
  one.

## What this cost, and what it did not

`read` hashes every record it inflates. Nothing measured that on a real facet
yet — the fixture is four chunks — and the honest expectation is that it is
tens of milliseconds against the 0.12–0.19 s a Felucca read already takes. If it
turns out to matter, the answer is not to stop checking: it is that S2 wants the
hashes in the snapshot anyway, so they will be computed once for both jobs.

**Measured on Felucca since**, by the migration this version bump obliges — a
re-import with `--verify`, which is a write followed by a full read back with
every record inflated and hashed: 5.1 s to import and write, 1.2 s for the
read-back over all 7,168 chunks. The expectation above holds, and the file went
from 107.6 MB to 22.6 MB now that the chunks are stored deflated.

`BaseError::Chunk` is now nearly unreachable through this crate's own writer — a
record that inflates to its length and hashes to its entry and is still not a
chunk is a file somebody else wrote. Its test builds exactly that by hand, so the
variant keeps a caller.

## Filed: the migration is diagnosed twice, and neither time by name

A version 1 base set left in a working tree does not report itself as one. The
playground panics twice — `load_world(config).expect("a world")` in
[`in_process.rs`](../../../../crates/e2e/shard/src/in_process.rs) kills the shard
thread with the real reason, and the main thread then panics on `RecvError` from
the readiness channel, which says nothing at all. The `BaseError::Version`
`Display` is exact and names the fix; what loses it is that it is printed by a
background thread and immediately followed by a second, emptier panic.

Two things are worth doing about it, neither of which is S2's: the readiness
channel should carry the boot error rather than be dropped, so the surviving
panic is the one with the reason in it; and the boot path is the place that
knows a re-import is the migration, so a version mismatch is the one
`BaseError` whose message could say so.

The client's cache does not have this problem — `cache::read` turns any
`BaseError` into `CacheError::Unreadable`, and every variant of that already
means *fetch the facet instead*. A stale cache file survives under its old
identity until [`sweep`](../../../../crates/client/net/src/cache.rs) ranks it
out, which is what that ranking is for.

## What is next

**S2 — a product is keyed by the chunk it was built from.** The manifest exists
now, which was S1's whole job for it. What S2 does with it:

- `MapSnapshot` holds the hashes it was read with, and `publish` rehashes only
  the chunks `Patch::touched_chunks` names — 57 KiB on Felucca, one rehash per
  edited square.
- The radar cache, the navigation bake and the building flood swap
  `(facet, chunk, facet revision)` for `(facet, chunk, chunk hash)`. That is what
  closes [`radar.md`](../../radar.md)'s 10.2, which names the shard that gives
  the path a production writer as the one who owes it.

`openshard_protocol::chunks`'s own doc for `WorldRevision` already states the
defect S2 removes, in the words the wire needed it in: *"after a publish every
chunk re-cut from the facet carries the new number while only the touched ones
changed content, so a cache keyed on a chunk's field throws away 7,167 good
chunks per one-tile edit."*

**Nothing blocks S3**, which is the one a person feels, and it is independent of
both.
