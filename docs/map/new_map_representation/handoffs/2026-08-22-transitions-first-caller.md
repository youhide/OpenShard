# 2026-08-22 — the transitions get their first caller

Second session of the day, and a small one: it closes the first of the three
things [the previous handoff](2026-08-22-snapshot-built.md) left landable on
their own, and files a finding that belongs to another track.

## Where it stands

Directions A0 and A are built and unchanged. What moved is the half of A0 that
had no caller.

- **`WorldMap::land_in_row(y, from_x, to_x)`** is the door, and it is
  `WorldMap::statics_in_row`'s other half — the same signature, for the same
  reason: a rectangle is walked row by row, and a row is where the cost is. It
  reads through `LandGrid::cells_in_row`, so each cell is one step east of the
  last rather than a block index and an offset derived per tile.
  `LandGrid::cell` is what a walk that stepped its way to an index reads
  through.
- **Three walks use it.** `radar::fill` walked its rectangle twice — once for
  the colour and once for `best_z`, which is the same lookup asked the same
  question twice — and now walks it once. `ground.rs`'s `LandWindow::gather`
  and `for_each_cell_in` are the per-frame pair: the camera's rectangle copied
  once a frame, and the walk that turns it into quads.
- **`radar.rs`'s open-coded tile → block is gone**, which is what the previous
  handoff found while collapsing the two block types: `origin_x / BLOCK_TILES`
  is `BlockCoord::containing`.
- **`snapshot.md`'s "Where a session starts" no longer sends a session to a
  built phase.** It pointed at phase 1 in `map.rs` and at five spellings of the
  block order that are not there any more.

## What was decided

**The door yields cells, not positions, and it ends where the facet does.** A
caller walking a rectangle already knows where it is, so a row that runs off
the eastern edge simply stops — and a caller must not assume it got one cell
per tile it asked for. That is stated in the doc comment because it is the one
way to misuse it: the tiles past the end are exactly the ones `WorldMap::land`
answers `None` for, and a `zip` that forgets them silently shortens a row.

The alternative was an iterator of `(x, y, LandCell)`, which would have made
the misuse impossible and made every caller pay for a position it already had.
`for_each_cell_in`'s clamp puts both ranges inside the facet, so it zips
against its own range and cannot be short; `gather` pads with `None` on both
ends and is the only caller that has to think about it.

**The typed extent for `WorldMap::from_blocks` was deferred to direction B**, on
the plan's own words — "small, and it is the kind of thing B will want anyway".
It is ~40 call sites in ~20 files, almost all of them test scenes reading
`from_blocks(1, 1, …)`, and each file would gain an import for a value used
once. The benefit today is that a literal names which number is which; B picks
a chunk layout and will have a real consumer for the type. Deferring is a
decision, not an oversight — reopen it there, with named public fields, which
is the only shape that actually stops `wide` and `down` being swapped.

**The newtype for `Command::Send`'s bytes was not taken, for a reason outside
this track.** `link.rs` is being edited by a parallel session, and its working
copy references a module that is not committed yet — so committing the file by
pathspec would carry that session's work into a `HEAD` that does not build. The
item stays where phase 2 left it. Whoever takes it should know the shape that
was worked out first: the checked constructor wants the connection's
`ClientVersion`, because `client_packet_length` is version-dependent for one id
(`0x08`, whose body grew a byte) — so `frame_client_packet` can say whether the
bytes really are one whole packet, which is the check worth having.

## What is next

Direction B, unchanged, with the three things the previous handoff named for
it. Of the smaller items it listed, one is done and two are open: the typed
extent, above, and the `Command::Send` newtype once `link.rs` is quiet.

## Found along the way, and filed elsewhere

**The flame fuzzer went red on a fresh seed**, in a run that touched neither
`light.rs` nor `lighting.rs` — both walks say blocked, the brute-force oracle
says open, which is exactly the family of
`docs/occluders.md`'s pinned corner graze. The seed line and what it costs to
pin it are in that document's backlog; it is deliberately **not** in
`lighting.proptest-regressions`, because pinned it makes `cargo test
--workspace` red for every session until another track settles it.

Worth carrying into any session that reads a red suite here: the fuzzers draw a
fresh seed per run, so a red run is not automatically about the diff in hand,
and a green one is evidence about the seeds that ran.
