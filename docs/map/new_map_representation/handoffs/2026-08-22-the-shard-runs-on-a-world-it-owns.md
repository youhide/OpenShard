# 2026-08-22 — the shard runs on a world it owns

Fifth session of the day, and the one that finishes direction **B**. The
previous session left a base set on disk that nothing read; this one is the
step that makes it matter — `world.base_sets` in the config, and a shard that
boots facet 0 out of our own format with the install's map and statics files
never opened.

The two things the last handoff said wanted an answer first both got one, and
they are in "What was decided".

## Where it stands

Three commands, and the shard is running a world it owns:

```sh
cargo run --release -p openshard-uofiles --bin openshard-map-import -- \
    --facet 0 --out felucca.osbase --verify
cargo run --release -p openshard-movement --bin openshard-navigation-bake -- \
    --facet 0 --base-set felucca.osbase
```

```toml
[world]
client_files = "/path/to/Ultima Online Classic"
facets = [0]

[world.base_sets]
0 = "felucca.osbase"
```

Measured on the shipped Felucca: the import is 0.2 s, the navigation bake over
the base set is 52 s, and the shard reads the 102.6 MiB base set back in
**0.17 s** — against about a second for the same facet out of the install.

`cargo check --workspace --all-targets`, `cargo test --workspace`,
`cargo clippy --workspace --all-targets` and `cargo fmt --all` are silent.
Clippy's ten warnings are the interiors track's, in files this work did not
touch — the same ten the last handoff counted.

### What is new

| | |
|---|---|
| `WorldConfig::base_sets` | A table keyed by facet: `0 = "felucca.osbase"`. Three new `ConfigError` variants, all for a way of writing it that would otherwise run and do the wrong thing quietly. |
| `bake::stamp_of_base_set` | `stamp_of`'s other half. The install's map files are not what a base-set graph was built from, and stamping them would *pass*. |
| `openshard-navigation-bake --base-set FILE` | Builds over a base set; the artifact lands beside it. |
| `boot::facet_source` | Where a facet comes from, and the three things that follow from that — the stamp, where the artifact is, and the command that rebuilds it. |
| `movement/tests/base_set_terrain.rs` | The plan's real acceptance test: the movement rules over the new source. |
| `server/tests/base_set_world.rs` | The boot path itself. |

## What was decided

**The knob is a table of facet to path, and nothing is derived.** Not a
directory of conventionally named files, and not one path for one facet.
`client_files` is still the install; `base_sets` says, per facet, where that
facet's *world* comes from; a facet in `facets` and not in `base_sets` loads out
of the install exactly as before. The consequence worth having is that a shard
converts **one facet at a time** — Felucca from a base set while Trammel is
still the install's — which is the migration path and not an extra feature.
A path guessed from a file-name convention, or from `client_files`, would be a
shard silently running the wrong world; the alternative to guessing is one line
of TOML.

**Naming a base set with `client_files` empty is refused, not degraded.** A base
set holds the **map**. `tiledata.mul` holds what a tile *is* — water, wall,
stair, climbable — and the multis hold what a house is, and neither is in the
format. A shard with a map and no tile table would run and answer every question
about the ground wrongly, which reads as a broken walk rather than as a missing
setting. So `Config::validate` refuses it, and `load_world` refuses it again for
the configs nobody wrote down (a test's, the playground's) that never go through
`validate`.

**Direction D came forward by one caller, and this is the whole reason B's last
step was two decisions rather than plumbing.** `bake::stamp_of` records the
install's `map0LegacyMUL.uop`, `staidx0.mul`, `statics0.mul` and `tiledata.mul`
— their names, lengths and mtimes. For a facet read from a base set those files
are not the source any more, and they are *still sitting there with those
mtimes*, so the check would pass: a stale bake answering for a world it was
never built from, which is the one thing a stamp exists to stop.
`stamp_of_base_set` names the base set and the tile table instead. The `Stamp`
carries the source revision either way, and that is what D will eventually keep
when the file stamps go; until then it carries both, which is strictly more than
either alone.

**The artifact lives beside the base set.** Not beside the install. It falls out
of the same argument: an artifact belongs to the world it was derived from, and
two worlds of one facet must not share a path. It also makes the acceptance test
say something — a UO install that has ever been baked has an
`openshard-navigation-0.bin` in it, stamped against the install's map files, so
a boot that was still looking there would find that file and *refuse* it. A
green `base_set_world` run is therefore also the statement that it looked
somewhere else.

**The file's own facet is checked against the config's.** A base set records
which facet it is, and `world.base_sets` says which facet it was named for. Two
answers to one question is Tokuno loading as Felucca — every coordinate valid,
every place wrong — so `facet_source` refuses the disagreement by name.

**`FacetKey`, because TOML has no integer keys.** A table's keys are strings,
always, and serde will not turn `"0"` into a facet on the way past. The choice
was a `BTreeMap<String, _>` that every reader parses again — and that can hold
`"felucca"` without anybody noticing until boot — or one conversion with a type
around it. It wraps `Facet`, not a bare `u8`, which is what
`facet_bare_fields`'s allowlist is for.

**The acceptance test asks whether the world *behaves*, not whether it
round-trips.** The importer's test already pins the same land cells, the same
statics and the same bytes twice. Nothing between a `Map` and a step is an
identity, though: `MapTerrain` reads flags out of `tiledata.mul`, averages four
corners for a slope, sorts statics per block and walks a Bresenham line for a
look. So `base_set_terrain` builds two terrains over one tile table — one facet
from the install, one from the base set — and compares `land_tile`, `ground_z`,
`land_is_water`, `statics_at` (as a *sequence*, since the draw order takes the
last), `stand_z`, `spawn_z`, `can_fit`, `can_step` in all eight directions, and
`sight_clear` over a twelve-tile line. On the shipped Felucca that is 30,856
sampled tiles, 61,264 allowed steps, 2,401 tiles with something standing on
them and 5,629 blocked looks, and the floors under those counts are asserted:
a run where everything was refused would agree perfectly and prove nothing.

It deliberately does **not** run the existing terrain tests over a base set.
Those pin *rules* — a staircase is climbable, a wall is not walked through — and
a rule that is wrong is wrong in both columns. This pins the *source*, and a
failure in it says which.

## What is next

**Direction C, unchanged**: patches, and the resolved snapshot. Nothing in B was
left half-done for it — `StaticId` still does not need bytes, for the reason the
last handoff gave.

The one thing C should know: the base set read is 0.17 s for a whole Felucca,
so "apply patches to the touched chunks and republish" has a cheap worst case to
measure against — a full re-read is already a fifth of a second.

## Found along the way

**The client end still reads the install, and now it can disagree.**
`crates/client/app/src/lib.rs:736` loads its own facet from `client_files` and
stamps its own navigation artifact against those files. That is direction E and
was always going to be, but the shape of the gap changed today: until now both
ends read the same files, so they agreed by construction. They still agree,
because a base set is a byte-exact import of the same install — but the moment C
lands a patch, the shard's world and the client's world are different worlds and
nothing in the code says so. E is what closes it; what is worth writing down now
is that the *reason* E is required moved from "we would like the client not to
need files" to "the two ends can be wrong about each other".

**`openshard-client-artscan`'s `interiors::stamp_of` has the same shape.** It
takes a client directory and stamps the install's map files, exactly as
`bake::stamp_of` did this morning. It is the client's bake, so it is E's or D's
rather than B's, but it is the second instance of the pattern and the fix is the
same one.

**`OPENSHARD_NAVIGATION` is now ambiguous.** It overrides `artifact_path` for
*any* facet from *any* source, so one variable can point a base-set shard at an
install-baked artifact. The stamp refuses it, so the outcome is an error message
rather than a wrong world — but the knob names one file and there are now two
kinds of world it could belong to. Worth a facet-and-source-aware spelling when
something else touches it.

**`artifact_path` keys on a directory, so two base sets for one facet in one
directory would share an artifact path.** Harmless today for the same reason —
the stamp catches it — but the artifact's name is derived from the facet alone
when it could be derived from the base set it belongs to.

**The playground does not know about base sets.** It force-overrides
`client_files` with the window's install directory and leaves `base_sets` alone,
so an operator's config naming one still works for the shard half while the
window reads the install. That is correct until E, and it is exactly the
disagreement the first finding describes — worth a look when E starts, not
before.

**`load_world` now checks something `Config::validate` already checked.** On
purpose, and said out loud in the code: `load_world` also takes configs that
never went through `validate` — `openshard_e2e_shard::stock_config`, and a test
building a `Config` by hand — and the failure mode without the second check is
that a base set an operator named is silently not loaded while the log says only
that there is no map. It is a duplicated *check*, not duplicated logic, and it
would be a good thing for something to make impossible rather than doubled.

**`world.facets` is still `Vec<u8>` while `world.base_sets` is keyed by
`Facet`.** Two spellings of a facet in one config struct. `facets` predates the
newtype and converting it is a one-line change plus its callers; it did not
belong in this session's diff.
