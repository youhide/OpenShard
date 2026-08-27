# Engineering follow-up findings

[Client backlog](README.md) · [Backlog](../README.md) · [Roadmap](../../README.md)

## Found while closing the radar plan's section 9

- **`cargo clippy --workspace --all-targets` is not silent**, though this
  repo's own `CLAUDE.md` says all three commands are. Ten sites, none of them
  radar's: `interiors.rs` (a redundant guard, two `if .. else` chains, a loop
  variable used to index), `items.rs` and `statics.rs` (two functions past the
  argument limit), `world.rs`, `presentation.rs`'s composite-LOD arm and
  `examples/interior_census.rs` (a useless `i32` conversion). They are warnings
  rather than denials, so CI is green and the claim is stale — either the
  warnings go or the claim does.

  > **Those ten are gone**, swept since. What stands in their place is a
  > different, shorter list, and it is the one below under *the last of
  > `navigation_spans.md`'s filed observations* — three files, five findings.
  > The *claim* is still stale, which is the half of this entry that has never
  > been closed.

## Found while taking the last of `navigation_spans.md`'s filed observations

- **Nothing checks an intra-doc link.** `steer.rs` carried `[`Ground::real`]`
  and `[`Ground::through_doors`]` — two fields that stopped existing when the
  pair of terrains became one `Footing` — and both survived every `cargo test`,
  `cargo clippy` and CI run since. `cargo doc` is not one of the three commands
  `CLAUDE.md` names, and `rustdoc::broken_intra_doc_links` is a *rustdoc* lint,
  so nothing fires it. Either the workspace lint table gains it and `cargo doc`
  joins CI, or every `[`Type::member`]` in this repo is prose that happens to
  have brackets round it. They were found by reading, which does not scale.

  > **Counted since, and it is not two.** `cargo doc --no-deps` over
  > `openshard-movement` alone reports **15** — a mix of unresolved links and
  > public docs pointing at private items (`can_step` → `climbed`,
  > `Overlay::blocker_anywhere` from a path that does not resolve). `-state`,
  > `-protocol` and `-ai` each have their own. So the gate is not a tidy-up
  > before it is turned on: whoever adds the lint spends a session on the
  > backlog first, and should decide separately whether
  > `rustdoc::private_intra_doc_links` is wanted at all — a public doc naming
  > the private function it delegates to is often the *right* thing to write,
  > and half of these are that.
- **`cargo clippy --workspace --all-targets` is still not silent**, and none of
  what is left is that session's: `common/uofiles/src/map.rs` (a needless
  borrow), `client/render/tests/traced.rs` (three borrowed expressions that
  implement the trait already), and `client/app/src/link.rs` (a 640-byte
  difference between enum variants). The first three are a parallel session's
  open files, which is why they were left rather than swept.

## Found while pricing `navigation_spans.md`'s baked adjacency

- **The instrument carries its own copy of the rule it measures.**
  `examples/step_cost.rs`'s `expand` helper reimplements the diagonal flank rule
  — the two flanking cardinals of a diagonal, refused together — and **four** of
  its rows now go through it, including the *floor* and *all eight on one column*
  rows the baked-adjacency decision rests on. `steps_out_of` owns that rule, and
  the example cannot call it for the rows whose whole point is to swap one half
  of the expansion out. So a change to the flank rule leaves the example
  measuring the *old* rule and passing: no test fails, and the plan's next number
  is quietly about something else. Same class as
  [`parity.md`](../../../parity.md)'s frame assembled by hand in seven places, one layer
  down. What would close it is `steps_out_of` growing a seam the example can
  substitute into, so there is one flank rule and the harness borrows it.
- **A bench's default is a claim about the machine it was written on.**
  `step_cost --repeat` defaulted to five passes, which is enough on a quiet
  machine; at load average 33 on 24 cores it moved rows by 30% run to run and
  produced a stable-*looking* reading that twenty-five passes do not reproduce —
  and that reading reached `navigation_spans.md` before it was caught. The
  discipline is now a section in the example's own module doc. **The other
  measuring examples were not audited for the same thing.** `coarse_bench` is
  the one that already does the right thing and is worth copying — it prints
  `repeat={}` in its own header, so a number quoted from it carries how it was
  taken. `map_path_probe` has the flag and does not print it; `span_index` and
  `span_census` quote a bake time with no repeat at all, and it is a bake time
  the plans keep.

## Found while taking the radar plan's soak

- **An instrument nobody can run at the scale it is about stays unread.** R7's
  radar panel was complete for weeks and answered nothing, because its two open
  questions — "walking costs no raster work", "the page cache is reached only by
  a pathological view" — are questions about a HiDPI, desk-scaled surface, and
  the machine the HUD is in front of is one surface. What closed it was lifting
  the frame's own step into `radar::advance` and driving *that* from an example,
  so the scenario is an argument rather than a machine. **The scene composites
  are in exactly the state the radar was in**: `CompositeTelemetry` is read by
  the HUD and by nothing else, `CompositeWorkQueue` and `CompositeCache` have no
  headless driver, and their budgets (128 MiB, `builds_per_frame`) have the same
  shape of claim attached to them. The radar's answer transfers whole — one
  function that is the frame's step, one example that drives it — and it is
  worth doing before the next composite budget is argued about rather than
  measured.
- **`RadarFrame` derives `Default` and `App::new` calls `RadarFrame::default()`**
  ([`app/src/diagnostics.rs`](../../../../crates/client/app/src/diagnostics.rs),
  [`lib.rs`](../../../../crates/client/app/src/lib.rs)), which is the construction
  `docs/style.md` and this repo's house rules refuse: a value from nowhere, with
  no place where somebody decided what it should be. The `..Default::default()`
  that fed it in `presentation.rs` is gone — every field of that sample is named
  now, so a field added later cannot be silently swallowed and read as a frame
  that measured nothing — but the derive and the one `::default()` call remain,
  in a file a parallel session had open. A named constructor (`RadarFrame::
  empty()`, beside the doc that already explains why the sample is reset) is the
  shape.
- **A layer's `graphic` is two index spaces wearing one type.**
  [`EquipmentLayer::graphic`](../../../../crates/client/render/src/mobiles.rs) is
  an `AnimId` — a worn item's picture — for every layer except `Layer::MOUNT`,
  where it is a creature's `Graphic`. The two happen to index the same
  `anim.idx`, which is why it works, and nothing but a doc comment says which one
  a given layer holds; `mount_of` opens it with `Graphic(saddle.graphic.0)`,
  which is exactly the newtype-crossing `docs/style.md` is about. The shape is
  either a second field or an enum over the two, and it is not free: the field is
  built in three places and read in four.
- **A seam tested from each end separately is not a tested seam, and the
  dismount is the proof.** `world/src/tick/tests.rs`'s
  `a_horse_is_mounted_and_dismounted_by_double_click` asserted that the shard
  sends the rider's own client a `0x1D` naming the saddle — and it did, and had
  all along. `client/net`'s `WorldView` had a `remove_from_equipment` and three
  callers, none of them the `0x1D` arm. So one side proved it spoke and the other
  proved it could listen, and nobody asked whether the words arrived: the saddle
  stayed on the body for ever, the rider stayed drawn in it, and the horse stood
  beside them. `items::consume`'s worn-item-eaten path had the identical hole and
  nobody had noticed *it* either, which is what says this is a method problem
  rather than a mount one. The fix closed both, but the check that would have
  caught them is an end-to-end one, and
  [`crates/e2e/shard/tests/`](../../../../crates/e2e/shard/tests/) is where it
  belongs — `paperdoll_buttons.rs`'s own header already names this exact failure
  mode ("a client that never folds what it decoded") as the fourth of four.
  **A ride is not cheap to drive from there**, which is the reason it does not
  exist yet and the thing to price before starting: a creature can only be put in
  the world through `.admin` → the gump → a button → a target cursor → a click,
  because `.add` lays items and there is no staff verb that lays a mobile. Either
  that chain is walked, or the missing verb is the smaller piece of work and the
  test comes free after it.
- **The rider is seated by the frame's own anchor, not by species.** The
  reference client carries a per-mount pixel correction (`Mounts.cs`'s `OffsetY`,
  non-zero only for the unusually tall or short — a unicorn at −9, a tiger at
  +18) and this engine applies none, because every mount it currently spawns is
  an ordinary-height animal. `openshard_protocol::mounts` is where the column
  would go, and the day a unicorn is rideable is the day it is owed.
- **The lighting proptest fails on fresh seeds, and its regression file grows
  every time somebody runs the suite.**
  [`render/tests/lighting.rs`](../../../../crates/client/render/tests/lighting.rs)'s
  `a_fuzzed_flame_near_a_row_edge_agrees_with_the_brute_force_oracle` and its
  `_through_the_exact_walk` sibling failed on one seed in one full-workspace run
  during the map work of 2026-08-25, passed on the next run, and failed on two in
  the one after — each failure appending a case to
  `lighting.proptest-regressions`, which is then replayed for everybody. The
  message is the test's own: *"the point sampler landed inside a solid on 1 ray(s)
  that `deepest_crossing` says miss every box — a point in a box is in it, so this
  is the exact test's defect."* So it is a real disagreement between the sampler
  and the exact walk at a row edge rather than a flake in the harness, and the
  file is the record of how often it is reachable. Two things follow: the suite is
  not green on a random seed, and a session that runs `cargo test --workspace`
  leaves a dirty working tree it did not write. Whoever owns
  [`lighting_pitfalls.md`](../../../lighting_pitfalls.md)'s exact-walk ladder
  should read the saved cases before they are trimmed.
