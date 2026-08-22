# 2026-08-21 — the track opened

## Where it stands

Documents only; no code. The three track documents were written from scratch
after a first draft was cut down: that draft settled two dozen questions on
paper — chunk fields, patch schema, materialisation transaction, a runtime
`WorldGeometry` type with its array layouts — none of which had been reached by
any code. What survived is in [`mechanics.md`](../mechanics.md); what was
premature is now a row in its open-questions table with the measurement that
would close it.

The map docs that were scattered across `docs/` moved into `docs/map/`, because
every one of them bakes something off terrain and the track changes what
terrain *is*.

## What was decided

- **The map's size is the only premise.** Chunks and patches follow from it.
  Nothing else in the track follows from anything else, and that is the test a
  new section has to pass.
- **Byte thrift is not a goal.** The classic format was shaped by expensive
  bandwidth; whole self-contained chunks over a cache that sometimes re-fetches
  too much beat any delta scheme that can leave a client half-patched.
- **A baked client map is a convenience, not a pillar.** An honest cache with
  background download shows the player the same world.
- **The classic protocol is not to be tortured.** Our own encoding either rides
  the `0xBF` envelope as an opaque body or gets a second stream; the map codec
  must not know which. Decided in direction E, not before.

## What is next

Direction A in [`plan.md`](../plan.md): one revisioned snapshot that every
reader takes a handle to, still over today's `WorldMap`, with no format, network
or patch machinery. It is a refactor with no feature in it, it is worth landing
on its own, and everything after it is cheap only if it lands first.

Two things to check while doing it, both found while inventorying the readers:

- The client and the server load the same install independently and agree by
  luck. Whatever the snapshot is, that must become one answer, not two.
- Three bakes — navigation, the building flood, the occluder table — are keyed
  by input file name, size and mtime. That key is what direction D replaces,
  and a changed world would lie to a player through all three at once.
