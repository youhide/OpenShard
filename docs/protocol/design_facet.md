# A facet is carried, not rewrapped

`protocol::world::Facet(pub u8)` is what a facet number is everywhere in the
engine. It is not unwrapped to satisfy a neighbour's signature and rebuilt on
the other side: a caller holding a `Facet` passes a `Facet`, and the only places
that hold the bare byte are the two seams F2 names and the one op2 parameter F4
does. [`facet_bare_fields.rs`](../../crates/common/protocol/tests/facet_bare_fields.rs)
is the allowlist of exactly those, checked against a workspace-wide text walk.

`Facet` lives in `protocol` because it is a packet field first, the way `Serial`
is — `state::components::Facet` is gone and every crate reads
`openshard_protocol::world::Facet` directly. The sweep that carried the type
through the ~70 call sites in between, crate by crate, is
[the facet sweep](evidence/2026-08-11-the-facet-sweep.md); it is a sibling of
[`design_wire_types.md`](design_wire_types.md) and deliberately does **not** use
that document's machinery, for the reason F1 gives.

## Decisions

Settled. Do not re-open mid-sweep.

**F1. This is not `protocol_newtypes.md`'s problem, so it does not get that
plan's machinery.** `Facet` has no domain to validate — every one of the 256
values a `u8` can hold is a legal facet number in principle (an unloaded one
is refused at the point that matters, `state.facets.contains_key`, which is a
*shard-config* fact, not a wire-format one). There is no `RawFacet`, no
`interpret`, no `validate`, no class A/B/C/D split. A field either already
carries `Facet` or gets changed to, full stop. This is why the sweep is worth
a plan and not a `sed` run despite being simpler than the protocol one: it
touches ~70 call sites across eight crates, and a plan is what keeps an agent
from "fixing" a boundary that is supposed to stay bare (F2).

**F2. Two boundaries stay bare, on purpose, and are not part of this sweep's
count.**

- **`persistence::{record, sqlite, pg}.rs` is the disk/SQL boundary.**
  `tick/regions.rs`'s own comment already says it — `// .0 at the record
  seam: a saved facet is a SQL column.` A `u8` is what SQLite and Postgres
  columns hold; `record.rs`'s structs are what serde reads a save file into.
  Converting there is not a bug, it is the seam, exactly
  [N3 amendment 7](evidence/2026-08-31-the-newtype-sweep.md#amendments-forced-by-n3-speechrs)'s
  `localized_message` argument and the `Contained.grid` exception in
  [N4 amendment 4](evidence/2026-08-31-the-newtype-sweep.md#amendments-forced-by-n4-containersrs).
  Left as-is; not on this sweep's task list and not on its coverage count
  either — it was never bare by omission.
- **`uofiles::map::{read_facet, candidate_shapes, facet_size,
  largest_facet_within}`'s `facet: Option<u8>` indexes a fixed-size array
  (`FACET_SHAPES.get(facet as usize)`) and formats client filenames
  (`format!("map{facet}LegacyMUL.uop")`).** Both uses want the raw number,
  not the domain type — an array index and a path component are exactly
  [N1 amendment 2](evidence/2026-08-31-the-newtype-sweep.md#amendments-forced-by-n1-the-rest-of-worldrs)'s
  `Point` case: the *number itself* is what is being used, not "a value
  believed to be a legal facet." `uofiles` is below `protocol` in the
  dependency graph for everything else it parses (`Graphic`, `Hue`), but
  `Facet` is the one type this file would gain nothing from taking a
  dependency on. Stays bare; documented here so a future scan does not
  rediscover it as an open question.

**F3. Order: the crate that discards it first, to the crate with the most
call sites.** `ai` (the pilot, in
[the record](evidence/2026-08-11-the-facet-sweep.md)) is small,
self-contained, and every one of
its callers already holds a `Facet` — proving the pattern costs one file. From
there: `npc`, `items`, `magic` (each under ten occurrences, each the same
shape as the pilot); `state::harvest` and `skills::handlers::harvest`
together, since the harvest handler is `state::harvest`'s only caller; `world`
last, because it is both the largest (`tick.rs` and eight `tick/*.rs` files)
and the one place `Command`'s enum variants live — the type has to exist on
every upstream caller (`ai`, `npc`, `items`, `magic`) before `Command`'s own
fields can stop needing a `.0` to build. `scripting` closes the sweep, not
because it is hardest technically but because its `facet: u8` fields are a
**third kind of boundary**, not yet decided (F4).

**F4. Answered — corrected premise first.** This section originally asked how
"this crate's Rhai integration resolves native function parameter types."
There is no Rhai integration: `scripting` embeds `deno_core`/V8 (a
TypeScript/JS runtime, see the crate's own `Cargo.toml` description), and its
native-function boundary is the `#[op2]` proc macro, not `rhai::register_fn`.
The question the wrong name was pointing at still had a real answer, once
asked correctly.

`engine/ops.rs`'s `#[derive(serde::Deserialize)] *Spec` structs (`SpawnSpec`,
`ContainerSpec`, region/door specs) and `lib.rs`'s `Event`/`Command` enum
fields are a serde boundary exactly like F2's persistence carve-out — but
plausibly closable the same way `ClilocId`/`SoundId` closed theirs
([the newtype sweep](evidence/2026-08-31-the-newtype-sweep.md)'s N3 backlog):
give `Facet` `#[serde(transparent)]`
and these fields hold `pub facet: Facet` directly, no per-call
`Facet(spec.facet)` conversion.

The one site that is *not* this shape: `op_clear_regions(state: &mut OpState,
facet: u8)` (`ops.rs:866`) binds `facet` as a **direct `#[op2(fast)]`
argument**. `#[op2]`'s fast path only accepts primitives it knows natively
(numeric types, `bool`, `#[string] String`, byte buffers) — it has no
mechanism to bind a single-field tuple struct positionally, the same
constraint Rhai's fast path would have imposed. The crate's own precedent
settles this rather than leaving it open: `Serial`, the crate's most-used
domain identifier, is `pub type Serial = u32` here (`lib.rs:42`) — a plain
alias, not a newtype, chosen specifically so it crosses the op2/JS boundary
bare. No newtype (`Graphic`, `Hue`, `ClilocId`, `SoundId`) is ever bound as a
direct op2 parameter; all of them appear only as primitive fields inside
`#[serde]` spec structs. `op_clear_regions` stays `facet: u8`, converted with
`Facet(facet)`/`.0` at its call site — the same exception `Serial` already is,
not a gap this sweep left open.

**F5. No compatibility shims.** Same as
[`design_wire_types.md`](design_wire_types.md)'s N11: a stage
wraps a group of signatures **and** updates every call site in the same
commit. A `.0` left in place "to keep the diff small" is the exact
invisibility this sweep exists to remove.

**F6. Coverage is counted, not assumed.** Each stage's commit message records
the file's `facet: u8`/`facet:u8` occurrence count before and after, the same
discipline as N10. The gate below was added at the end of F3's order rather
than the start: while most of the workspace was still red, maintaining it would
have cost more than it caught.

## The gate

A text scan in the same spirit as
[`bare_integer_fields.rs`](../../crates/common/protocol/tests/bare_integer_fields.rs),
but simpler: `Facet` is one name, not a class hierarchy, so the check is "does
`facet: u8` (or `facet:u8`) appear in this fixed list of files" rather than a
directory walk with a type-shape matcher. It lives in
[`facet_bare_fields.rs`](../../crates/common/protocol/tests/facet_bare_fields.rs)
— `protocol` is where `Facet` is defined, and the test reaches out to the other
crates' source by a workspace-relative path (`CARGO_MANIFEST_DIR/../../..`),
the same way the survey did, because the thing being asserted is a property of
the workspace, not of `protocol`'s own `src/`.

Its `ALLOWLIST` is the enforced list and the place to read the current one:
`(file, occurrence count, reason)` rows, holding F2's two carve-outs, F4's one
op2 parameter, and the standalone examples that follow their crate's fix. Every
other `facet: u8` in the workspace is gone, and a new one — a packet field, a
`Command` variant, a spec struct — fails the next `cargo test`.

