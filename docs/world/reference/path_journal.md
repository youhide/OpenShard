# The route journal: a click in the game, a test in the tree

A session writes down every route it planned; afterwards a tool asks the same
questions again over the real facet and says what is different about the
answers. What it is for is the one report nobody could act on before: *"I
clicked over there and my body walked at a wall."*

Crates: [`common/pathlog`](../../../crates/common/pathlog) writes and reads the
file; `client/app`'s `steer.rs` is what fills it in; the replay is
[`common/movement`'s `path_replay` example](../../../crates/common/movement/examples/path_replay.rs).

## Recording

**It is already on.** A client writes `path-journal.jsonl` where it was
started — beside `client_ui.ron` — from the first click of every session:

```sh
cargo run -p openshard-playground
```

The switch is in the **F1 window, Tile tab**, under the terrain overlay: a
checkbox and a line saying what has been written this session (`12 orders, 47
plans, 31 KiB`). It persists in `client_ui.ron` like every other F1 setting, and
a settings file written before the journal existed keeps one — a missing field
is not a person having said no.

It is not an environment variable, and that is the point. A route walks into a
wall *once*, in the middle of playing; a diagnostic that has to be predicted
before that session is a diagnostic that is not there the one time it matters.

Three properties worth knowing:

- **Nothing is opened until there is a line worth writing.** A client started to
  look at a gump plans no route and creates no file.
- **The session before this one is kept** as `path-journal.prev.jsonl`: the
  first line of a new session moves the old file aside rather than over it.
- **Every line is flushed as it is written**, so a client that is killed still
  leaves the click that killed it on disk. Turning the switch off keeps what is
  already there and stops adding to it; turning it back on writes a fresh
  `session` line, so a reader can see where the gap was.

The journal stops itself at 64 MiB and says so in the file (a `closed` line) and
in the F1 status line — a journal that is always on would otherwise outlive the
session it was interesting for.

One JSON object per line, five kinds of them:

| line | when |
|---|---|
| `session` | once, at startup: the facet, whether a coarse graph was loaded, the node budget, the weight |
| `order` | a destination was named — a Ctrl-click, or a drag that moved it somewhere new |
| `plan` | one search answered that destination — **several per order**, because a route is replanned whenever what is left of the last one runs out |
| `arrived` | the body reached the place the order named |
| `abandoned` | the order gave up: four steps that did not move the body |
| `closed` | the journal reached its size cap and stopped — the difference between a file cut short by policy and one cut short by a crash |

A `plan` line carries the question (`from`, `to`, and the standing place `to`
resolves to), both searches as they reported themselves (`arrived`, `exit`,
`explored`, `written`, and the long query's `long` where the coarse graph was
asked), the route in both halves (`open`, and `barred` past whatever was in the
way), the points those steps land on, the refusal a player was shown, and what
the whole plan cost.

**A drag does not write a line per mouse-move.** The client restates a
destination on every raw move and only a *new* one is an order; the plan behind
it is lazy and runs at most once per step. A journal of a minute's walking about
is a few dozen lines.

## What is deliberately not in it

**The world.** No map, no live layer, no crowd — nothing that was standing on
the ground at the time.

That is the whole design and not an omission. A journal that carried a slice of
the overlay would replay its own answer by construction and prove nothing, and
the fixture it produced would be a wall of captured covers nobody can read. What
a person wants out of a report is a **test**, and a test says

```rust
overlay.set(Tile::new(1342, 1676), vec![Cover::door(52, 20)]);
```

— a scene somebody can reason about and edit. The replay's job is to say which
tile that line goes on.

## Replaying

```sh
cargo run --release -p openshard-movement --example path_replay -- --list
cargo run --release -p openshard-movement --example path_replay -- --episode 3
cargo run --release -p openshard-movement --example path_replay -- --episode 3 --plan 2 --radius 14
```

`--release` matters: the run replays every plan of an episode, and a debug
build's A\* is roughly twenty times slower than the one the session ran. The
journal defaults to `path-journal.jsonl` in the working directory — pass
`--journal path-journal.prev.jsonl` for the session before this one — and the
facet comes from `OPENSHARD_CLIENT` (plus `OPENSHARD_BASE_SET` when the shard is
running a world of ours). With no `--episode` it replays the last one, which is
almost always the click somebody has just come to complain about.

An **episode** is one destination: the click, every replan under it, and how it
ended. `--list` prints one line each.

The replay opens the same facet and asks the same question over the **bare
map** — no doors, no crates, no houses, nobody standing about. So the two
answers agreeing means the live layer had no part in the report, and the two
disagreeing localises what did.

### The verdicts

Each plan gets three paragraphs.

**Is the recorded route walkable here?** The sharpest question, because its
answer is a tile: a step the bare map refuses is a step something was carrying
the body over — a house floor, a ship's deck, a placed stair — and the run
prints the tile and draws the ground around it.

**Do the two plans arrive?** Compared before the steps are, because two routes
that both arrive differing is an ordinary fact about a map with two ways round a
building, and one of them not arriving is the report:

- the session arrived and the bare map does not → the route ran over something
  the map does not have;
- the bare map arrives and the session refused → something was in the way that
  is not here, a shut door or a crate or a body;
- neither arrives → the refusal is the map's, and a test needs no live layer at
  all.

**Was it the budget?** A refusal is re-asked at 50,000 nodes, and the answer is
read against the session's *own* budget. A way that costs fifteen nodes was
never refused by a budget of seven hundred — so a session that refused it was
not walking on this ground, and the honest verdict is "not the budget" rather
than the "a bigger budget would have arrived" that reading `arrived` alone
gives.

### Two things the replay will tell you about itself

- **The coarse graph.** The session line says whether the client had one; the
  replay says whether it found one. A stale or missing artifact turns every long
  destination into a refusal the session never made, so a run that differs there
  says so up front. Rebake before believing anything about a long route — see
  [`navigation_artifact.md`](navigation_artifact.md).
- **The resolved destination.** A click carries a picture's height and the
  search compares against a place to stand. Where this ground resolves the same
  click to a different height, the live layer had a surface on that column, and
  the run names both.

## From a record to a test

The point of the whole loop. Once the replay has named the tile:

1. If the bare map reproduces the session's answer, the test belongs beside the
   other real-facet routes — `common/movement/tests/real_routes.rs`, which is
   `#[ignore]`d and reads a client install.
2. If the disagreement is the live layer, the test builds that layer instead of
   reading one: an `Overlay` with the door, the crate or the floor at the tile
   the replay named, over the facet or over nothing at all. `steer.rs`'s own
   tests and `path.rs`'s are both written that way, and neither needs a client
   install to run.

The recorded `from`, `to`, budget and weight are the arguments; the recorded
`open` route is the assertion's expected value or the thing being contradicted.
