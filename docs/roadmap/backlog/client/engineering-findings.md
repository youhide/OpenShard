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
