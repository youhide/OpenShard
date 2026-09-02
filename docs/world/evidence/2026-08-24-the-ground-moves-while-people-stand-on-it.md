# 2026-08-24 — the ground moves while people are standing on it

Direction **C's second half**: the live publish. The session before this one gave
the world a history a command line could write; this one lets a running shard
write it, between two ticks, with players on the facet.

One commit, and the whole of what it adds over the offline tool is **an order** —
which of the world and the log moves first, and what happens to the other one
when the second fails.

## Where it stands

Four staff verbs, and a facet whose ground answers differently afterwards:

```
.tile                     # what the map holds under you, with each static's ordinal
.setland 3 40             # the ground under you becomes land 3 at z 40
.addstatic 0x0edd         # a static goes into the map under you
.rmstatic 0               # and comes back out
```

Each commits one patch to the `.ospatch` log beside the base set, so the edit is
**durable**: a shard started again over the same files resolves to the same
revision and refuses the same steps.
`a_committed_patch_changes_what_the_shard_allows_and_survives_a_restart` is that
sentence as a test — it walks east on flat ground, raises the tile east of the
player to z 60, finds the step refused, and then builds a *second* shard over the
same two files and finds it refused there too.

The blocker the last handoff named is gone and was gone before this session
started: `FacetState::terrain` is no longer a `Box<dyn Terrain>`. Era R's R2 took
the trait object out, so the snapshot was reachable and this needed no state
surgery at all.

### What is new

| | |
|---|---|
| `openshard_map::patch::Undo` | The way back from a publish: the inverses, in the order they replay, and the revision to return to |
| `patch::apply` → `Vec<PatchOp>` | It always built the inverses; it used to throw them away on success |
| `patch::revert` | Replays inverses this module produced. Infallible by construction, and says so |
| `PatchOp::set_land` / `add_static` / `remove_static` | The three constructors that read `was` out of the world. `openshard-map-patch`'s `build` is now four lines of match over them |
| `MapSnapshot::undo`, `World::publish` / `undo` | The map's half, with `PatchError::NoGround` for a facet that has no map at all |
| `Ground::publish` / `undo` | The bake moves with the ground, in the same statement — the type's existing invariant, extended to the one thing that moves the base at runtime |
| `FacetState::publish` / `undo`, `FacetUndo` | The same, plus the coarse router |
| `WorldHome`, `FacetState::home` | Where a facet's world lives on disk. `Some` exactly for a facet read from a base set |
| `openshard_world::mapedit` | `commit` — the order, in one function — and `rebake_command` |
| `gm.rs`'s four verbs | `.tile`, `.setland`, `.addstatic`, `.rmstatic` |
| `world/tests/mapedit.rs` | Five tests: the edit, the bake, the router, the facet that is not ours, and the rollback |

## What was decided

**The world moves first, and the log second.** Both orders have a failure
window, and this is the survivable one. Appending is what realistically fails —
a full disk, a read-only file, a log belonging to another world — but *whether a
patch applies* is a question about a world, and the only honest way to ask it is
to apply it. A log-first order would either write down a patch nobody checked —
and a patch that does not apply is a shard that will not boot, because
`openshard_basemap::load` refuses the **world** rather than the record — or ask
the question twice, in two places, in two spellings of one rule.

So the world moves, the log is written, and **if the log refuses, the world goes
back**. That is the discipline the last handoff called for by name and left to
this phase.

**`apply` stops throwing its inverses away.** It already built them — the
half-applied rollback needs them — and the only reason they were private is that
nothing had asked. `Undo` carries the revision as well as the ops, because a
world put back at the number it left is a different thing from a world put
*forward* to a third revision: this publish was never visible to anybody, and the
borrow that made it atomic has not been given up yet.

**An undo is not a revert, and the type says so.** A revert an operator asks for
is a new patch over the world as it now stands, committed to the log like
anything else — history is append-only and a mistake is part of it. `Undo` is the
other case: a publish that never became history at all. Not `Clone`, because
there is exactly one way back from one publish.

**The coarse router is dropped on a publish, not kept and not rebuilt.**
Rebuilding is a 52-second offline bake and cannot happen in a tick; keeping it
would be a router that confidently plans through a wall somebody just built.
Dropping it costs long routes until the shard is rebaked and restarted — and
every commit prints the command that does it, because the navigation artifact is
stamped against the world this just moved and a shard with a stale one does not
boot. Direction **D** is what makes the rebuild local enough to do here.

**It travels in the undo.** `FacetUndo` is the map's `Undo` plus the router the
publish took, so a rolled-back commit puts both back: the router was never stale,
because the world it describes is the world being restored.

**A facet says where its world lives, and it says it at construction.**
`FacetState::new` grew a `home: Option<WorldHome>` rather than a setter, and the
sixteen call sites that now pass `None` are the price of the invariant: a facet
that could be told a *second* home is a facet whose edits could be written into
somebody else's world. `None` is not "not known yet" — it is "this facet is the
client's files, and cannot be edited", which is `openshard_map::patch`'s own rule
one layer up.

**The log's path is derived and the base revision is carried.** `WorldHome` holds
the base set's path and the revision of the *file* — which is the log's header,
not the revision the world reached by replaying it. `openshard-state` holds no
I/O: it is a path and a number, and `openshard-world` is where they meet
`openshard_basemap::patches`.

**One op per patch, because a command line has no spelling for "and also".** The
model holds a list and the applier walks it in order, so a brush that batches
them needs nothing here. That is direction F's, and it is the same conclusion the
offline tool reached.

**The author is the operator's own name.** A history whose every entry says
"staff" cannot answer who raised the hill.

**`PatchTime` is the wall clock, and that is not the tick reading a clock.** A
patch's time is for a person reading the history; the *order* is the chain of
revisions. Nothing in the simulation reads it.

## What is clean

`cargo test` over the crates this touched — map, movement, state, world, basemap,
commands, server, housing, boats, guilds, party: **all green**, including the
five new integration tests. `cargo clippy` on the same set: silent. `rustfmt` on
every touched file.

`cargo test --workspace` does **not** build, for a reason that is not this
session's: `crates/client/render/tests/frame.rs:5672` reads `rows.start` on a
`DirtyRows` that has no such field — a parallel session's work in progress, left
alone.

## What is next

**Direction E, and it is now the only thing between here and C's own "done".**
C asks that a patch be *visible to a connected client*, and nothing here moved
that: our client draws the facet it loaded out of the install, and the classic
client draws the one on the player's own disk. So today an edit changes what the
shard **allows** — where a body may stand, what a step is refused for — while
every picture on every screen is the world as it was. An operator who runs
`.setland` sees nothing happen and then cannot walk there.

That is the same disagreement the last handoff recorded, arriving one step
closer: it used to need a command line, and now it needs a sentence typed in the
game.

**Then D**, which is what makes the two facet-wide costs above local: the span
bake is 0.07 s and the router is a 52-second rebake, for an edit that touches
`Patch::touched_chunks` and nothing else.

## Found along the way

**`WorldState::publish` answers `PatchError::NoGround` for a facet that does not
exist**, which is true — there is no ground — but it is not what the caller got
wrong. Every other reader of a missing facet in that file panics with
`expect("an entity's facet is always loaded")`, and this one should say the same
thing rather than dress a caller's bug as a map with no ground.

**`.tile` deliberately does not list the live layer.** A door, a crate or a house
floor is an *entity*, and none of them is something a patch can name — so
listing them beside the statics would offer ordinals that `.rmstatic` cannot
take. Worth a second verb one day; not this one.

**A patch that deletes the ground under a door leaves the door.** `World::publish`
touches the base and never the live layer, which is the model — the two move on
different clocks — but nothing anywhere asks what a door standing in a doorway
that no longer exists should do. Not a defect today, because no editor can make
that shape by accident; it is F's to answer when a brush can.

**The four verbs are staff-gated by the caller, and nothing in `mapedit` checks.**
Same as every other command in `gm.rs`, and stated here because a map edit is the
first one whose blast radius is the whole facet rather than the actor.

**The commit is one tick's work, and the span bake inside it is 0.07 s.** Nobody
has measured what that does to a tick that also has players in it — the number is
from the span plan's own bench, on a facet with nothing happening. Worth knowing
before an editor publishes on every brush stroke.
