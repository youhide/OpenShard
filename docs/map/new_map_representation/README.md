# Track: a new map representation

The world should be data the shard owns and can edit — not the map files in
each player's UO install. This track is that change, from the first imported
chunk to an editor that commits.

## The documents

| | |
|---|---|
| [`overview.md`](overview.md) | **Start here.** What we want, and what is wrong today — stated as facts about this codebase, not as a wish list. Short on purpose. |
| [`mechanics.md`](mechanics.md) | How a changeable map works: base, patch, snapshot; chunks; what goes stale; how a chunk reaches the client. Where a decision is not made, it says so and names what would settle it. |
| [`plan.md`](plan.md) | Seven directions with the code each one touches, in order, with what "done" means for each, plus one deferred on purpose. A0 and A are refactors with no feature in them. |
| [`snapshot.md`](snapshot.md) | 🚩 **The plan being executed first.** Directions A0 and A on their own — the block order gets a type, and the map gets one revisioned owner. No format, no patches, no network. Start a session here. |
| [`client_today.md`](client_today.md) | What direction A takes a handle to, measured: the layout `WorldMap` actually has, what each bake costs in memory and on disk, and the ranked backlog found while inventorying the readers. |

Read them in that order. `overview.md` is the only one that has to be read to
argue about the idea; the others are for doing the work.

## Handoffs

[`handoffs/`](handoffs/) — one file per session that moved the track, newest
last. A handoff says where the work stands, what was decided, and what the next
session should pick up. The plan holds the *intent*; a handoff holds the
*state*, so a plan is never edited to record progress that a handoff should
carry.

## Status

[`snapshot.md`](snapshot.md) is built, all three phases of it: the block order
is `LandGrid`'s and only `LandGrid`'s, every reader takes a handle to one
revisioned `MapSnapshot` per facet, and the world is one type in `openshard-map`
that has never opened a file. That was directions A0 and A plus the move they
turned out to imply, and none of it added a feature.

**Direction B is built, and it was the first thing here that added a feature.**
There is a chunk — 64×64 tiles, decided by measurement — with a canonical
encoding, and `openshard-basemap` is the file a facet goes in.
`openshard-map-import` bakes a UO facet into one: on Felucca that is 7,168
chunks and 102.6 MiB, and reading it back reproduces all 29,360,128 tiles and
writes the same bytes again. **And the shard runs on it**: `world.base_sets`
names a base set per facet, `openshard-navigation-bake --base-set` builds the
graph over it, and the movement rules answer identically over both sources at
tens of thousands of sampled places. What a base set does *not* replace is
`tiledata.mul` and the multis, which are still the install's.

**Direction C has its first half.** A world is now a base set *plus a log of
patches over it*: `Patch` and its three operations, `MapSnapshot::publish`,
the `.ospatch` log beside the base set, and `openshard_basemap::load` — the one
call both the shard and the navigation bake resolve a facet through, so the two
cannot arrive at different revisions of it. `openshard-map-patch` commits one
change from a command line, and a committed change survives a restart and
changes what the server allows.

**And its second: the live publish is built.** A running shard edits its own
ground — `.tile`, `.setland`, `.addstatic`, `.rmstatic` — and the whole of what
that adds over the command line is an *order*: the world moves first, because
applying a patch is the only honest way to ask whether it applies, and the log is
written second, so a log that refuses puts the world back. The span bake follows
the ground and the coarse router is dropped, because a graph baked over the world
before the edit is a graph of somewhere else.
See [`mapedit`](../../../crates/server/world/src/mapedit.rs) and the
[handoff](handoffs/2026-08-24-the-ground-moves-while-people-stand-on-it.md).

**This track is era S of [`map_rebuild.md`](../map_rebuild.md)**, which is the
map area's entry point and the document that ordered the nine plans here. What
is left below resumes after era R (the runtime map — the tile table out of the
file reader, the live layer joining the type, a house with floors, the statics as
one immutable run) and era P (spans, and the graph the server finally reads).
Nothing here is blocked by them; it is sequenced behind them so that a bake keyed
to a revision, and an editor previewing through the runtime's apply path, are not
written against a layout that is still moving.

What is left of C is the **client**: an edit reaches a running shard's rules and
no picture at all, because both ends still draw the facet they loaded off disk.
That is direction E, and it is now the only thing between here and C's own
"done". The decisions and the leftovers are in the newest
[handoff](handoffs/2026-08-24-the-ground-moves-while-people-stand-on-it.md); the
plan itself records intent, not progress.
