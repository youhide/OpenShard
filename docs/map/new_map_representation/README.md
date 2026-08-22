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
| [`client_today.md`](client_today.md) | What direction A takes a handle to, measured: the layout `Map` actually has, what each bake costs in memory and on disk, and the ranked backlog found while inventorying the readers. |

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

**Direction B is most of the way built, and it is the first thing here that
does.** There is a chunk — 64×64 tiles, decided by measurement — with a
canonical encoding, and `openshard-basemap` is the file a facet goes in.
`openshard-map-import` bakes a UO facet into one: on Felucca that is 7,168
chunks and 102.6 MiB, and reading it back reproduces all 29,360,128 tiles and
writes the same bytes again. What is left of B is the step that makes it
*matter* — the server reading a base set instead of the install — which needs a
config decision and drags a piece of direction D forward. Both are named in the
newest [handoff](handoffs/2026-08-22-a-world-without-an-install.md), which is
also where the decisions and the leftovers are; the plan itself records intent,
not progress.
