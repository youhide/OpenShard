# 2026-08-24 — a rope is not a floor

The backlog's first player-visible entry, taken: **the real cause of the pier
report**, whose first suspect was *a boat moored at a pier*. The suspect held a
defect, the defect is fixed, and **it is still not the report**.

What it cost to find out is two surveys and one rule. The rule is that **a boat
was the only placement on this shard that did not read its art through
`Cover::of_static`** — and on the shipped multi table that turned eighty ropes,
rudders and tillers into floors.

## Where it stands

### The entry that sent this session is stale at the top

[The backlog handoff](2026-08-23-the-backlog-is-weeded-and-what-is-left.md) says
a session starts at the top of its first table or at era S. Read the table
against `roadmap.md` before trusting it: since it was written, the **mobile
obstacle** closed on both sides of the wire, **`Sectors::nearby`** closed with
it, the routing half of **"do bodies block?"** closed, and the clock came out of
both searches, which retires the load-sensitive test entry. The world-map LOD
entry was already struck through in the handoff itself.

What was left at the top was the pier report. That is what this session did.

### 🚩 A ship read its art by a rule nobody else uses ✅

`planks_of` split a component on `is_blocking()` alone — hull if it stops a
body, **deck if it does not**. Every other placement here — housing, decoration,
the persistence reload, the client — goes through `Cover::of_static`, which
splits on `is_platform()`. So does the reference:
`(flags & ImpassableSurface) == TileFlag.Surface`,
`Scripts/Services/Pathing/Movement.cs:211`, where something to stand on must
*carry* `Surface` rather than merely fail to carry `Impassable`.

`openshard-boats`'s [`moored_boat`](../../../crates/server/boats/tests/moored_boat.rs)
prices it over the real table — **24 ships, every one affected**:

| | |
|---|---|
| floors invented out of art that is neither platform nor blocker | **80** |
| decks with no thickness (the platform's own blocking half) | **352** |
| tiles per ship a body can walk on that no other reader believes in | **2–3** |
| climbables unhalved, platforms lost to a blocking flag | 0, 0 |

The invented floors are the half with a fall in it. `walk::aboard` takes the
**nearest** live surface to the body's feet and `Overlay::surface_at` bounds
only the climb, so a rope at the ship's own z is a floor **two under the deck
beside it**, and a body boarding with its feet near that height lands on the
rope.

`Plank` now holds the `Covers` its art lays, filled only by `Plank::of_art`,
with the field **private** so there is nowhere left to write a second reading.
`hull_blocks` became `blocks_at`, because a ship's own deck answers it now, and
it borrows `Cover::meets` rather than repeating the span arithmetic.

### 🚩 And then the pier was actually walked ✅

The backlog asked for "a shard-side sweep with a live overlay". This is it: over
facet 0, **400** pier and bridge decks with sea beside them, **260** with room
for a small boat within four tiles, and every step off the pier through
`step_allowed`.

| | the ship makes legal | onto its deck | **under its deck** |
|---|---|---|---|
| the reading now | 352 | 300 | **0** |
| the reading retired | 403 | 298 | **3** (worst 2) |

**One ship per pier, each in an overlay of its own.** The first version of this
survey laid every ship into one harbour and had to refuse berths that overlapped
— which dropped the sample from 400 piers to 26 without saying so. A cap on
coverage disguised as a fixture is exactly what `roadmap.md`'s "no silent caps"
asks about, and one ship at a time has neither problem.

**So a moored ship did put a walker under a deck — and it is not this report.**
Three times over a facet, two units deep. A player who falls underground on a
pier is not two units low.

### 🚩 The swimmer's question, which `boats.md` said needed this overlay ✅

`boats.md` records, since 2026-08-23, that its reason for keeping
`MapTerrain::swimming` off is **unmeasured**: with the flag on, `check` answers
water instead of refusing it, so a body that cannot climb to the deck would
stand on the sea under its own ship. The same survey has the overlay that
prediction needed.

The distinction that makes it the right question is **where the swimmer starts**:
off a pier a body reaches from the top of the pier's own art and clears a deck
easily, so the first version of this pass measured nothing. Walked in from the
water instead — reach is the waterline plus two — over **3,866** tiles alongside
a hull and **8,450** steps toward a ship:

| | refused | on the deck | **under it** |
|---|---|---|---|
| the reading now | 8,450 | 0 | **0** |
| the reading retired | 7,467 | 93 | **890** (worst 3) |

**The prediction was correct, and the same fix closes it** — not the flag. A
deck's blocking half starts at the waterline, so there is no gap under it to
float in. A swimmer can also no longer clamber aboard over the gunwale, which is
UO's own answer rather than a loss: you board over the plank, and this shard has
not built one.

## What was decided

**The reading is `Cover::of_static`'s, and `Plank`'s field is private.** The
alternative was to teach `planks_of` the platform bit and leave the triple
alone, which fixes the numbers and leaves the second reading in place. A public
`(z, height, blocks)` is somewhere for a third one to be written.

**A deck lays its blocking half, and that is a behaviour change.** A body can no
longer stand inside three units of planking. It costs nothing to a walker
boarding from a pier — 300 boardings now against 298 before — and it is what
closes the swimmer's case.

**The survey keeps the retired rule written out.** `the_reading_that_was` is a
free function in the test, not a paragraph in the history: a survey that
compared the current reading against itself would print zeroes whatever either
end did, and this one would notice a regression back.

**Both walks are judged against the deck as the shard reads it *now*.** What
counts as a fall cannot be defined by the reading under test — the retired one,
asked about its own invented floors, reports itself correct.

## What is clean

`cargo test -p openshard-state -p openshard-boats -p openshard-world`: **794
passed, 0 failed, 3 ignored.** `cargo clippy` silent on the three. `rustfmt` on every
touched file. The workspace as a whole was not run: a parallel session has
`movement`'s `path.rs` and `navigation.rs` open and the tree does not always
compile.

## What is next

| | what would close it |
|---|---|
| **The pier report's cause, still unknown.** Two suspects left: a **multi-step walk**, where each step is right and the sequence drifts; and **arriving rather than walking** — a login, spawn, gate or teleport, which reach `spawn_z` and not `check` | The second is the cheaper: `spawn_z` has no `start_top` to reach from and no overlay in it at all, so a deck is not a surface it can choose. A body gated onto a moored ship lands in the sea by construction — worth one test before another sweep |
| ~~**This client has no notion of a boat at all**~~ **Superseded 2026-08-25** | The handoff recorded the defect as found. The client now expands every known multi, projects its components into the live overlay, and predicts both walk arms from the complete `Footing`; `docs/boats.md` carries the repair. |
| **A ship can be moored through a dock** | `check_berth` asks only that a berth tile is water, and a water tile can carry a pier plank — **52 of 352** boardings land on the plank rather than the deck under it. A second clause beside the "all sea" one |
| **`Boats::deck_at`, `carries` and `blocks_at` have no caller outside tests** | They are the third, fourth and fifth spelling of what the overlay now answers. Either they read it or they go |
| **The plank, and boarding over it** | `boats.md`'s own phase, and now with a number in front of it: a pier stands **−7..7 above the deck a body boards onto, median 7**, so "step aboard over the gunwale" is a seven-unit drop the reference does not have |

**Where a session starts:** era S's live publish is still the only *plan* node
open, and it still has a design question in front of it — who calls
`MapSnapshot::publish` and where in the tick. Everything above is repair work
that needs no plan.
