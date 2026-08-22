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

[`snapshot.md`](snapshot.md) is built, both phases of it: the block order is
`LandGrid`'s and only `LandGrid`'s, and every reader takes a handle to one
revisioned `MapSnapshot` per facet. That was directions A0 and A, and neither
added a feature.

The next move is direction B — our own chunk format and a UO importer. Where it
stands in detail, what was decided along the way, and the smaller landable
things left behind are in the newest
[handoff](handoffs/2026-08-22-transitions-first-caller.md); the plan itself
records intent, not progress.
