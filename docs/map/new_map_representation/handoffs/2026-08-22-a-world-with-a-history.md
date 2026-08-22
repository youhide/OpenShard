# 2026-08-22 — a world with a history

Sixth session of the day, and the first half of direction **C**. The previous
one made the shard run on a world it owns; this one gives that world a way to
*change* — a patch with a parent, an author and an exact undo, a log beside the
base set, and a resolve that both the shard and the offline bake go through.

What is not here is the *live* publish: an edit taking effect in a running shard
between two ticks. That is C2, and "What is next" names the one thing standing
in its way.

## Where it stands

Four commands, and a world with two edits in it:

```sh
cargo run --release -p openshard-uofiles --bin openshard-map-import -- \
    --facet 0 --out felucca.osbase
cargo run --release -p openshard-basemap --bin openshard-map-patch -- \
    --base-set felucca.osbase --author stas set-land --x 1495 --y 1629 --tile 1004 --z 25
cargo run --release -p openshard-basemap --bin openshard-map-patch -- \
    --base-set felucca.osbase --author stas add-static --graphic 3980 --x 1495 --y 1629 --z 25
cargo run --release -p openshard-movement --bin openshard-navigation-bake -- \
    --facet 0 --base-set felucca.osbase
```

Measured on the shipped Felucca: the two patches are **115 bytes** of log
between them, the whole world — 102.6 MiB of base set plus its log — resolves to
revision 3 in **0.14 s**, and `show` reads the history back as two records with
the ops that made them. `base_set_world` then boots the shard on it: the
navigation artifact is stamped against the base set, the log and `tiledata.mul`
at revision 3, and it validates.

Two things that run confirmed rather than merely compiled:

- **The graph over the patched world is a different graph.** 8,527,823 bytes
  against 8,527,780 for the same facet at revision 1. One raised tile and one
  added static, and the routing changed — which is what makes the stamp's
  refusal worth having rather than pedantic.
- **The stamp refused the stale one, by name.** A graph built over the base set
  alone reported `built from map revision 1, expected 3` and printed the command
  that rebuilds it. That happened by accident here — an offline bake run with a
  binary from before this session — which is exactly the accident the check
  exists for.

`cargo check --workspace --all-targets`, `cargo test --workspace`,
`cargo clippy --workspace --all-targets` and `cargo fmt --all` are silent.
Clippy's ten warnings are the interiors track's, in files this work did not
touch — the same ten the last two handoffs counted.

### What is new

| | |
|---|---|
| `openshard_map::patch` | `Patch`, `PatchOp`'s three operations, `StaticId`, `PatchAuthor`, `PatchTime`, `PatchError`, and the applier. The module header is the decision record. |
| `MapSnapshot::publish` | Apply a patch and move to the revision it produces. `&mut self`, which is the atomicity. |
| `MapRevision::after` | The third way a revision comes into being, and the only one that is not a reading. |
| `Map::remove_static` | `place_static`'s inverse, and the patch applier's alone. |
| `codec::encode_patch` / `decode_patch` | The second record in that module: a patch as canonical bytes. |
| `openshard_basemap::patches` | The `.ospatch` log — framed, checksummed, append-only. |
| `openshard_basemap::load` | **The one door to a world of ours**: base set plus log, resolved. |
| `openshard-map-patch` | Commit one change from a command line, list a tile, read the history. |
| `bake::stamp_of_base_set` | Now stamps the log as well as the base set. |
| `basemap/tests/patch_log.rs` | Eleven tests: the world resolves, and every way the pair fails. |

## What was decided

**The log is a file beside the base set, not a table in `persistence`.**
`plan.md` says patches are persisted through `crates/server/persistence`, and
this is a deliberate departure from it — for a reason the plan could not have
had before B landed. The navigation bake is a binary in `openshard-movement`,
which cannot see a server crate; direction D requires every bake to be built
over the *resolved* world; so a world only the shard could resolve would be a
world no offline tool could bake. Two more callers point the same way: direction
E ships a world to a client that has no database, and F's editor is not
necessarily inside the shard process. The database holds *entities*. The world
is `openshard-basemap`'s, and now both halves of it are.

**The log's path is derived, where `base_sets` derives nothing.** The last
handoff refused a path guessed from a file-name convention; this one takes one,
and the difference is what is being named. `base_sets` says *which world* a
facet is, and guessing that is a shard silently running the wrong one. A patch
log is not another world — it is the rest of the one already named, and a base
set without its log is a world missing its edits. Two files that must travel
together are better joined by a rule than by a second line of configuration an
operator can forget.

**A static's identity is its ordinal on its tile, read against the patch's
parent revision.** `mechanics.md` asked for a stable identity because two
identical rocks can stand on one tile at one height. It is not a field: what
makes an ordinal exact is that a patch is only ever applied to the one world it
was made against, and `publish` refuses any other. `StaticId` therefore still
needs no bytes in the base format, which is what the handoff before last
predicted and this one confirms by building it.

**An op carries what it replaces.** `SetLand` carries the cell that was there,
`RemoveStatic` the item it takes away. Two things pay for the bytes: the inverse
is exact and needs no world, so a revert is these ops read backwards; and a log
paired with the *wrong* base set is caught. That second one is real rather than
theoretical — a re-imported facet is revision 1 again, so the parent check
passes and the ops would apply, tile by plausible tile, to somewhere else.

**All of a patch or none of it, and the rollback is free.** Applying an op
*returns* its own inverse — which it has to, because `AddStatic`'s inverse names
an ordinal that is only knowable once the item is in. So a patch that fails
halfway undoes what it did by walking those inverses backwards, and the same
list is what a revert will be built out of. There is one apply path, which is
what `plan.md` asks of F's preview for the same reason.

**`&mut self` is the whole of "a reader never sees half a change".**
`mechanics.md` asks that a new revision become visible between ticks and never
during one. Every reader borrows a `&Map` out of the snapshot, so a `&mut` on
the publish cannot be taken while one is alive. It is the borrow checker rather
than a rule about tick boundaries that somebody has to remember — and it is why
`publish` is on `MapSnapshot` rather than on something beside it.

**A publish changes only the tiles the ops name.** No chunk is re-cut, nothing
is re-hashed, no facet is rebuilt: `plan.md`'s "a publish never rebuilds a
facet", taken literally. `Patch::touched_chunks` is what says which bakes died,
derived from the ops rather than stored beside them — a stored list could
disagree with the ops it claims to describe, and then an invalidation would miss
a chunk that really did change.

**The log is stamped, so a patch makes the navigation graph stale.** It is an
input in exactly the sense the base set is, and a graph built before an edge was
committed is a graph of a different world. The consequence is real and is *the
cost of this phase*: committing one patch means a 52-second rebake before the
shard will boot. That is D's rule working exactly as intended, and D is what
makes the rebuild local to the touched chunks instead of the facet.

**A torn tail is refused, not trimmed.** A crash between a record's length and
its payload leaves a file the reader names and refuses, rather than a world
quietly missing its last edit. The safe version of trimming needs the publisher
to have flushed the record before acting on it — at which point a torn tail is
provably an unacknowledged patch — and that discipline belongs with C2's live
publish rather than here.

**Patches lie over a base set, and a facet still read out of an install cannot
have one.** There is nowhere to keep a log beside an install we do not own, and
no guarantee the operator will not replace the files under it. `world.base_sets`
is what says a facet has a world of ours; the conversion path the last handoff
built is therefore also the path to being able to edit.

## What is next

**C2 — the live publish. The one thing in the way is `FacetState::terrain`.**
`with_facet` boxes `MapTerrain<MapSnapshot, _>` as `Box<dyn Terrain + Send +
Sync>` ([`tick.rs:444`](../../../../crates/server/world/src/tick.rs#L444)), so
the running shard holds the snapshot inside a trait object and has no handle to
call `publish` on. `FacetState` needs to own the snapshot in a shape a publish
can reach — and that is a change to the state crate's shape rather than to the
patch model, which is exactly why it did not belong in this session's diff.

Everything after that is small by comparison: a publish appends to the log,
swaps the snapshot, and invalidates the coarse router for the touched chunks —
and the third of those is direction **D**, which C2 will run into on its first
patch rather than later. The `52 s` above is the measurement that says so.

**Then the client.** C's own "done" asks that a patch be visible to a connected
client, and nothing in this session moved that: our client still loads facet 0
out of the install. See the first finding.

## Found along the way

**The two ends are now actually different worlds, not just able to be.** The
last handoff wrote that the client end reads the install and "the moment C lands
a patch, the shard's world and the client's world are different worlds and
nothing in the code says so". C has landed a patch. Anyone who runs
`openshard-map-patch` against the base set their shard is configured with now
has exactly that disagreement, and the client draws the unpatched ground while
the server refuses the step. It is still direction E's to close; what changed is
that it is now reachable by an operator rather than a prediction.

**`openshard-client-artscan`'s `interiors::stamp_of` still stamps the install's
map files.** Unchanged from the last handoff, and now the third instance of the
pattern rather than the second — `bake::stamp_of_base_set` has been extended
twice in two sessions while that one has not moved at all.

**The revision arithmetic escaped once and was pulled back.** `openshard-map-patch`
briefly worked out which revision the log lay over by subtracting the patch count
from the snapshot's revision. That is only right while one patch means one
revision — true today and not a property worth depending on from outside — so
`Loaded` carries `base` instead. Worth remembering the next time something wants
to count revisions rather than be told one.

**An empty patch is legal.** It is a revision that changed nothing, and refusing
it would have meant a fallible constructor for no gain. It does invalidate every
bake over the facet, so an editor should not publish one — which is a note about
F, not a hole in the model.

**`world.facets` is still `Vec<u8>` while `world.base_sets` is keyed by
`Facet`.** Carried over untouched from the last handoff; a one-line change plus
its callers, and it did not belong in this diff either.

**`openshard-map-patch` is one op per patch.** A patch holds a list and the
applier walks it in order, so the model is ready for many; the command line just
has no spelling for "and also". A brush is what would want one, and that is F's.

**The patch codec's `Cursor` is the second bounds-checked reader in the
workspace.** `openshard_map::codec::decode` checks its whole length up front
because a chunk's header says how big it is; a patch is a list, so the check has
to travel with the read. If a third record of the list-shaped kind appears, that
cursor is the thing to lift out rather than to write again.
