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
| [`to_the_client.md`](to_the_client.md) | 🚩 **The plan being executed now.** Direction E on its own: the pipe, chosen off measurements rather than preference, and five phases from "the client's world is a parameter" to "an operator types `.setland` and a connected screen changes". |
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
"done".

**E has started**, and its plan is [`to_the_client.md`](to_the_client.md). The
pipe is chosen — the `0xBF` envelope in the `0xE000` range this engine already
reserved for its own subcommands, with the chunk deflated before it is framed,
because a deflated chunk of Felucca is at most 16,050 bytes and every one of the
7,168 fits in a packet. **E0 is built**: the client takes a `WorldSource` rather
than always reading the install, `--base-set` is the other arm, and the
resolution that used to be spelled out in the shard's boot and both bake binaries
is one function all four now go through.

**E1 is built too**: the wire carries a chunk. Four subcommands — a request, a
deflated chunk in fragments of at most 8,192 bytes, a notice on world entry
saying which facet at which revision, and a refusal — with the deflating and the
joining as one pair of functions in `openshard-protocol`, so the two ends of the
wire are a round trip rather than two implementations.

**And E2: the client's world comes off the wire.** `--world-from-shard` starts a
client with no facet at all; it is told on world entry how big the one it is
standing in is, asks for every chunk of it in requests of 64 with 256 in flight,
and assembles the world through the same `chunk::assemble` a base set is read
through. What it cost is the startup order — `run` used to read the facet before
the window existed, and everything after it was built from a map that was already
there. The client now starts with a `Ground` that has no base and grows one when
the fetch lands, and the gap between the two is closed by one gate checked at the
frame and at the window's events rather than by holding the world back. The
decisions and the leftovers are in the [handoffs](handoffs/); the plan itself
records intent, not progress.

**And E3: the client keeps what it was given.** The 21.3 MiB is paid once — what
arrives is written as a base set of ours and read back through
`openshard_basemap::load`, so a cache hit is E0's reader and not a second format.
It is filed under the *world* rather than under the shard: a shard names its
world in the notice, by a hash of its base set's own bytes, because our own
playground dials no address at all and a re-imported facet is a different world
at the same one. A second start over an unchanged world asks for no chunks; one
over a world an operator edited asks the shard what moved and fetches exactly
that. The open question the plan left — rewrite whole or grow a tail — was closed
by measurement: writing all of Felucca is 0.10–0.13 s, so it is rewritten whole.

**And E4: a publish reaches a connected client**, which is direction C's own
"done" and the last clause of it. `mapedit::commit` sends `0xE005` after the log
has taken the patch, to every connection standing on the facet — the same
exception `WorldNotice` already is, because a client cannot ask about something
nobody told it happened. What a client of ours does with it is fetch the named
chunks and hand them to its *window*, since by then the facet belongs there and a
`MapSnapshot` has one owner per process: `Fetch::moved` ends in the squares
themselves and `Ground::take_chunks` puts them in, rebaking the spans in the same
statement. Everything derived over the ground they replaced goes with them — the
composited blocks by name, the radar's products by naming a revision they were
not built at, and the coarse graph outright, which is the trade the shard already
makes when it publishes.

**And the largest entry of E's backlog is closed: a fetch survives the ground
moving.** A publish that lands while chunks are on the wire used to end the
connection, because the answers still coming were cut at a revision the shard had
moved past and nothing tells them apart from the answers a second fetch would ask
for. Now the fetch is *abandoned* rather than failed — what it is still owed is
eaten without being decoded, and when the wire is clean it asks again for the
**union** of what it was asking about and what the publish named. A publish that
arrives while the client is still *asking* what moved has no list to union, so
the stale answer is recognised by its revision and the question is asked again.

What is left of E is the rest of that backlog, and none of it is load-bearing:
the untested joining in `link::play`, a base set that could store its chunks
deflated, and the small cache manners — a world that moved by an empty patch, and
nothing sweeping an orphaned one.
