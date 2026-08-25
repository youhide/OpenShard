# The artifact follows the graph

G1 made the coarse graph follow a patch. Nothing made the **file** follow the
graph, and the two together are what a shard is — so the session after an edit
did not start. [`navigation_graph.md`'s G2](../navigation_graph.md#g2--the-artifact-follows-the-graph)
is the whole of it.

## Where it stands

**Built.** The symptom was a shard refusing to boot on its own world:

```
world load: 8 patch(es) applied to facet 0; it is at revision 9
panicked at crates/e2e/shard/src/in_process.rs:131:
  navigation artifact ./felucca-navigation-0.bin is stale:
  built from map revision 7, expected 9
```

Not a rare state — **the state every edit leaves behind**. `FacetState::publish`
rebakes the graph around the edit on the tick that commits it, and the artifact
beside the base set is only ever as new as the last bake, so a `.setland` and a
restart put the shard a half-minute whole-facet bake away from coming up.

Boot now reads the artifact as far behind as the log can carry it, unions the
chunks the missed patches touched, runs the same `rebake_chunks` a publish runs,
and writes the artifact back. Exercised on the tree's own Felucca: an artifact
six revisions behind the world was carried forward and saved by `load_world`
itself, and the whole file — two world loads, a copy of the base set, a commit
and a catch-up — runs in under three seconds, against the 28.0 s a whole bake
costs.

- `bake::load_behind` beside `bake::load`, and an `Accept` that says which of the
  two is being asked for.
- `World::catch_up` → `WorldState::catch_up` → `FacetState::catch_up`, which is
  `publish`'s second half without the publish.
- `boot::missed_chunks`, where the log is read and a gap it cannot cover is
  found.

## What was decided, and against what

- **The log carries it, not a shutdown hook.** Saving the graph when the shard
  stops is a smaller diff and covers less: a killed shard, or one that fell over,
  leaves exactly the artifact this started with. The catch-up covers the crash
  for the same code.
- **Not on every publish.** The artifact is 7.8 MB, and writing it on the tick
  that commits an edit would put that write in a brush stroke. One rebake per
  restart is the whole of what the deferral costs.
- **The patch log is the only forgivable input.** An artifact baked at revision 7
  was stamped over a shorter log, or over no log file at all, so that entry is
  dropped from *both* stamps rather than compared leniently. A base set that was
  re-imported and a tile table that moved are refused exactly as before — nothing
  replays those, and a graph validated against them would be a router planning
  through a world it has never seen.
- **A file can only say it is *below*.** Whether the log holds the patches
  between the two revisions is a question for the log, and `load_behind` does not
  pretend to answer it. Ancestry is checked where the log is: `missed_chunks`.
- **Ahead is refused.** An artifact newer than the world it names is a log that
  lost records under a graph, and there is no direction to replay that in.
- **One rebake over the union of every missed patch**, because the rebuilt set
  derived from a union contains every set derived from a member of it, and the
  ground is at its final revision either way.
- **The catch-up runs after `with_facet`.** That is where the facet's span index
  already exists; carrying the graph forward before it would mean baking a second
  span index over the same facet.
- **A write that fails is a warning.** What is in memory is the world as it
  stands, and the next start catches up again rather than getting it wrong.

## What is next

- The other two bakes keyed to a `MapRevision` — the interiors flood and the
  occluder measurements — are still whole-artifact rebakes with no per-chunk
  answer, so neither can follow a patch and neither can be caught up. They fail
  the same way and are the same size of problem.
- `an_artifact_left_behind_by_an_edit_is_caught_up_from_the_log` runs only with
  `OPENSHARD_BASE_SET` and `OPENSHARD_CLIENT` set, as everything in that file
  does. The loader's half is a unit test in `bake.rs` and does run in CI.
- Loading a world **does** now rewrite the artifact beside it, which
  `a_shard_loads_facet_zero_out_of_a_base_set` does to the operator's own file
  when it runs. It writes it *current*, which is what a boot would do anyway.
