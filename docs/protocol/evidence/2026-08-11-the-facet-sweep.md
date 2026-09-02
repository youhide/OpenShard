# The facet sweep: carrying a wrap that already exists

Record of the multi-session sweep of the bare `facet: u8` left across
`crates/server/*` and a few of `crates/common/*`, opened 2026-08-11 by a newtype
hunt across `server`, `common` and `render` (its handoff is kept outside this
repository) as that pass's single largest finding, out of scope for it on
purpose. The decisions it settled — F1–F6 and the gate — are
[`design_facet.md`](../design_facet.md); below is the survey it started from,
the pilot, and the amendment each crate forced.

## The survey

[The protocol phase record](2026-08-24-the-protocol-phase.md)'s "Two types for
one facet byte" entry (closed before this hunt) had already unified two
competing types into one `protocol::world::Facet` and converted every
*state-owning* call — `state.facet_of`, `state.facet_state`,
`WorldState::move_to`, the `facets: BTreeMap<Facet, FacetState>` table itself —
to the wrapped type. That closed fix did not, and was not asked to, touch the
*functions in between*: a caller holds a `Facet` from `state.facet_of(entity)`,
unwraps it to `.0` to satisfy a neighbour's `fn foo(facet: u8, ...)`, and that
neighbour immediately rewraps it (`Facet(facet)`) to call back into `state`. The
type is never wrong, never absent at either end — it is thrown away and rebuilt
at every function boundary in between, which is exactly the shape
[N2's `Serial` finding](2026-08-31-the-newtype-sweep.md#amendments-forced-by-n2-mobilers)
described: "the call sites already held a `Serial`... and were unwrapping them
to satisfy a `u32`."

By grep, `facet: u8` (as a function parameter, struct field, or enum variant
field) appeared in the crates below. The survey is the one taken when the sweep
was scoped; the **status** column was kept current as stages landed, because a
count that only ever described the starting position is a count nobody can act
on.

| crate | files | occurrences | status |
|---|---|---|---|
| `world` | `tick.rs`, `tick/{command,gates,decor,regions,travel,fields,death,tests}.rs`, `gm.rs`, `spawner.rs`, `events.rs` | ~38 | done |
| `scripting` | `lib.rs`, `engine/ops.rs` | ~16 | done — `op_clear_regions`'s one parameter allowlisted, F4 |
| `persistence` | `record.rs`, `sqlite.rs`, `pg.rs` | ~10 | **not in scope — F2** |
| `ai` | `lib.rs` | 7 | done (pilot) |
| `magic` | `travel.rs` | 5 | done |
| `npc` | `guards.rs`, `live.rs`, `spawn.rs` | 5 | done |
| `items` | `spawn.rs` | 4 | done |
| `state` | `harvest.rs` | 2 | done — but the crate's real stage was four `components.rs` fields the survey missed, also done |
| `skills` | `handlers/harvest.rs` | 1 | done |
| `uofiles` | `map.rs` | 1 | **not in scope — F2** |
| examples (`render`, `uofiles`) | | 2 | follow their crate's fix, allowlisted |

Every occurrence outside the two carve-outs was the same shape, checked by hand
while scoping: `ai::lib.rs`'s `foe_in_sight`, `probe`, `chase_step`,
`flee_step`, `kite_step` and `step_toward` all took `facet: u8` while every one
of their callers held a `Facet` and wrote `facet.0` to call in;
`npc::guards.rs`'s `call_guards`/`guarded_here`/`nearest_candidate` and
`npc::spawn.rs`, `items::spawn.rs`, `magic::travel.rs`'s `may_travel`/
`describe`, `world::gm.rs`'s admin commands and every `world::tick/*.rs` file
repeated it. Nowhere did a bare `facet: u8` in this set hold a value that
*wasn't* a `Facet` a moment before or after — the wrap was never the question;
carrying it was.

## The pilot: `ai::lib.rs`

Smallest file with the pattern, seven occurrences, one file plus two call
sites in `npc::live.rs` (`openshard_ai::step_toward`). Every function already
receives a `Facet` from its own caller and unwraps it on the way in:

| function | today | becomes |
|---|---|---|
| `step_toward` (pub) | `facet: u8` | `facet: Facet` |
| `foe_in_sight` | `facet: u8`, compares `state.facet_of(entity) == Facet(facet)` | `facet: Facet`, compares `== facet` |
| `probe` | `facet: u8`, calls `state.facet_state(Facet(facet))` | `facet: Facet`, calls `state.facet_state(facet)` |
| `flee_step` | `facet: u8` | `facet: Facet` |
| `chase_step` | `facet: u8` | `facet: Facet` |
| `kite_step` | `facet: u8` | `facet: Facet` |

Every caller inside `lib.rs` currently writes `facet.0` to call one of these
and gets a `Facet` back moments later from `state.facet_of`/`state.facet_state`
— all six become `facet` with no `.0`. The two external callers
(`npc::live.rs::nearest_player`/`step_toward` call site) go the same way:
`facet.0` becomes `facet`, and `nearest_player`'s own `facet: u8` parameter
picks up the type too, since it exists for the same reason.

## Amendments forced by the pilot

1. **`step_toward`'s `Option<u8>` return stayed `Option<u8>` — direction, not
   facet, and the two were never confused. This sweep is about the *facet*
   parameter only; a heading/direction sweep is a separate, already-recorded
   backlog item** ([the server/common/render newtype
   hunt](../../client/evidence/2026-08-25-server-and-render-type-findings.md), entry
   #2, `Direction` unwrapped through `ai`'s pathing core). Touching it
   here would have widened the pilot's blast radius for no reason connected
   to `Facet`.
2. **`npc::live.rs::nearest_player` picked up `Facet` at the same time as its
   one caller, `live.rs`'s own tick function**, because both sides of that
   call already held a `Facet` and were unwrapping/rewrapping across a single
   crate-internal boundary — the same shape F3 predicted `npc` would be, one
   function early.
3. **A third external caller of `step_toward` turned up outside both crates
   the survey named: `quests::progress.rs`'s escort-following beat.** F3's
   `npc`-stage estimate only counted `npc`'s own call sites; `quests` depends
   on `ai` directly for the same "walk toward, planned around obstacles"
   behaviour an escortable uses. Its one call site (`facet.0` → `facet`, the
   variable already a `Facet` from `state.facet_of(npc)` two lines above)
   went with the pilot rather than waiting for a `quests` stage that F3 never
   scheduled — the fix was one line and the alternative was leaving a known
   call site broken until some later stage happened to notice it. Any stage
   after this one should grep a function's callers workspace-wide before
   trusting a single crate's occurrence count.
4. Bare-integer `facet` count in `ai/lib.rs`: 7 before, 0 after. `npc/live.rs`:
   2 before (two call-site `.0`s: `nearest_player`'s call and `walk_home`'s)
   plus `nearest_player`'s own parameter, all 0 after — `nearest_player`
   folded into the pilot per amendment 2. `quests/progress.rs`: 1 before, 0
   after, per amendment 3.

## `npc::guards.rs` — done

`call_guards`, `guarded_here`, `nearest_candidate` → `facet: Facet`, same
shape as the pilot: every caller already held one (`guard_keywords`,
`hunt_with_guards`, both from `state.facet_of`). `guarded_here`'s
`Facet(facet)` construction and `nearest_candidate`'s `Facet(facet)` map key
both dropped. `hunt_with_guards`/`guard_keywords` are unchanged in signature
(neither took a bare `facet`), so the one external caller
(`world::tick/regions.rs`'s `hunt_with_guards`) needed no edit. Count: 3
before, 0 after (`guarded_here`'s param, `nearest_candidate`'s param,
`call_guards`'s param — `guard_keywords`/`hunt_with_guards` already wrote
`facet.0` at call sites, now write `facet`).

## Amendment forced by `guards.rs`: `SpawnSpec`-shaped structs are a fourth
## kind of boundary, not counted by F3

`npc::spawn.rs`'s `SpawnSpec.facet: u8` (and, by the same shape,
`items::spawn.rs`'s and `scripting::engine::ops.rs`'s own `SpawnSpec`s) is
**not** the pilot's pattern of "every caller already holds a `Facet` a moment
before." It is populated from `world`'s `Command::SpawnMobile` variant
(`world::tick.rs:851`, `world::tick/spawners.rs:80` via `area.facet`), which
is itself read off a bare `facet: u8` `Command` field — still unconverted,
`world`'s own stage. Converting `SpawnSpec.facet` now would force `world`'s
two call sites to convert early, which is exactly the dependency F3 already
named ("the type has to exist on every upstream caller... before `Command`'s
own fields can stop needing a `.0`") — just discovered one level lower than
F3's text described it, at a struct field instead of at `Command` itself.
`guards.rs`'s own `make_guard` keeps writing `facet: facet.0` into
`SpawnSpec` for this reason; it is not an oversight, it is the same boundary
F2/F4 already carve out, one more instance of it. **Any `SpawnSpec`-shaped
struct (`npc`, `items`, `scripting`) stays bare until `world`'s stage converts
`Command::SpawnMobile`/`RegisterRegions`/etc. — do not convert these fields as
part of the `npc`/`items`/`magic` stages F3 scheduled them under.** `npc`'s
count is `guards.rs` 3→0, `live.rs` (done in the pilot) — `spawn.rs`'s field
stays bare on purpose, not pending.

## Amendment: `magic::travel.rs` is blocked the same way, one level further out

Checked before starting the `magic` stage. `magic::may_travel`/`describe`'s
`facet: u8` params look like the pilot shape from inside `magic` — every
`world::tick/{travel,gates}.rs` call site already holds a `Facet` a moment
before (`facet.0`, `self.state.facet_of(entity).0`) — but `magic::
destination_of` reads `mark.facet` off `state::components::RuneMark {
pub facet: u8, .. }`, and `world::tick/travel.rs`'s own `travel_to`/`recall`
keep `facet: u8` all the way through (`Facet(facet) != here`,
`self.state.facets.contains_key(&Facet(facet))`, `can_stand_at(facet, ..)`) —
that function is `world`'s own, not `magic`'s, and is explicitly the large
stage F3 puts last. Converting `magic`'s signatures now would force
`RuneMark` (a `state` component, not in the F3 count at all) and a slice of
`world::tick/travel.rs` to convert early — the same shape as the `SpawnSpec`
finding above, one hop further from `magic`. `PublicGate.facet`/`gate()`/
`public_gate_at` are a separate, self-contained const table with no such
chain (`world::tick/gates.rs`'s two call sites already hold `Facet` outright)
and could move alone, but doing only that and leaving `may_travel`/
`describe`/`standing_at`/`destination_of` bare would split one file's stage
across two sessions for a handful of lines — deferred to when `magic`'s stage
is picked up properly, alongside `RuneMark`. **`RuneMark.facet` needs adding
to `state`'s stage list** (it was not in the original ~87-occurrence survey;
`state` table above only counted `state::harvest.rs`).

## `items::spawn.rs` — half done, the other half blocked the same way

`spawn_leftover`, `place_on_ground` → `facet: Facet`. Both had every caller
already holding one: `items::drag.rs` (three call sites, one from a local
`facet: Facet` already in scope, two from `state.facet_of(..)`) and
`items::trade.rs`'s one call site (`state.facet_of(receiver)`) — none crossed
a `Command`-shaped boundary, so both converted clean, pilot pattern. `spawn_item`/
`spawn_container` did **not**: `spawn_item` has one caller
(`world::tick.rs:813`, `Command::SpawnItem`'s handler) that is still bare,
same as `npc::spawn.rs`'s `SpawnSpec` finding — left untouched, on purpose,
not an oversight. `world::gm.rs`'s three `spawn_item` calls and
`skills::handlers::harvest.rs`'s one already write `facet.0` and would have
converted trivially had the fourth (blocked) caller not existed — F5 (no
partial signatures) is why the whole function stays bare rather than
converting the three ready callers and leaving `tick.rs` as a lone `.0`
holdout. Count: `items/spawn.rs` 2 of 4 signatures converted (`spawn_item`/
`spawn_container` stay bare, blocked); `drag.rs` 3→0, `trade.rs` 1→0.

## `state::harvest` + `skills::handlers::harvest` — done

Both halves went together, as F3 scheduled them, and both were the pilot's
shape with no blocker:

- `state::harvest::Banks::get` and its private `default_vein` →
  `facet: Facet`. `default_vein` keeps one `.0`, at the arithmetic leaf where
  ServUO's seed wants the *number* (`u64::from(facet.0) * 3`) — the same
  reading as F2's `uofiles::map` carve-out, except here the domain type is
  carried all the way to the expression that consumes it rather than being
  dropped at the signature. That is the distinction worth keeping: a `.0` on
  the last line of a computation is not the same defect as a `.0` on a
  parameter.
- `skills::handlers::harvest.rs`'s `resolve_harvest_target` → `facet: Facet`,
  dropping its `state.facets.get(&Facet(facet))` rewrap, and the `banks.get`
  call site's `facet.0`. Its one non-test caller,
  `world::tick/staff.rs`'s `TargetPurpose::Harvest` arm, already held a
  `Facet` from `self.state.facet_of(actor)` — found by the workspace-wide
  caller grep pilot amendment 3 asks for, in a crate (`world`) whose own
  stage has not started; the one-line call site went with this stage for the
  same reason `quests` went with the pilot.
- As predicted above, `spawn_item`'s call site in the same file stays
  `facet.0` — that function is blocked on `world`'s `Command::SpawnItem`.

Counts: `state/harvest.rs` 2 signatures → 0 bare (plus 14 test call sites),
`skills/handlers/harvest.rs` 1 → 0, `world/tick/staff.rs` one `.0` dropped,
`world/tick/harvest_tests.rs` 6 call sites wrapped.

## Amendment: `state`'s real stage is four component structs, not `harvest.rs`

The F3 table counted `state` as "`harvest.rs`, 2". With those converted, the
crate's remaining bare `facet: u8` is four **component** fields —
`components.rs`'s `RuneMark`, `RunebookEntry`, `Moongate`, `InRegion` — none
of which the original survey saw (it grepped signatures, and these are
struct fields on saved components). `RuneMark` was already known from the
`magic` amendment below; the other three are new here. All four are read by
`world::tick/{travel,gates,regions}.rs` and written by
`persistence::{record,sqlite}.rs`, so they are *both* the `world` stage's
dependency and an F2 disk-seam question: whether the record structs keep the
`u8` (they should, per F2) while the live component carries `Facet`.
**`state`'s stage is therefore not done — it is merged into `world`'s**,
because none of the four can convert without their `world` readers in the
same commit (F5). Do not treat "`state::harvest` done" as "`state` done".

## The travel half of `world`, and `magic` with it — done

The four `state` component fields the amendment above named, plus every
`world` reader and writer of them, plus the whole of `magic::travel.rs` — one
commit, because none of the three could move without the other two (F5). What
made this a stage rather than the `world` stage is that **nothing in it
touches `Command`**: every function here is called from another function in
the same three files, and the two that *are* `Command` handlers were left bare
on purpose (below).

- **`state::components.rs`: `RuneMark`, `RunebookEntry`, `Moongate`,
  `InRegion` → `facet: Facet`.** All four are saved components, so the disk
  seam is where their `u8` now lives and nowhere else —
  `world::tick/persist.rs` gained four conversions (`mark.facet.0`,
  `entry.facet.0` out; `Facet(facet)`, `Facet(entry.facet)` back), each the
  F2 shape and one of them already carrying that comment verbatim.
- **`magic::travel.rs` closed entirely**: `may_travel`, `describe`,
  `public_gate_at`, the `PublicGate.facet` field and the private `gate()`
  row-constructor take `Facet`; `standing_at` and `destination_of` *return*
  `(Facet, Point)` instead of `(u8, Point)`. The nine-row `PUBLIC_MOONGATES`
  table now reads `gate("Britain", Facet(0), ...)` — one wrap per row rather
  than a bare `u8` parameter kept for terseness, because a bare `facet: u8`
  left anywhere in the file is exactly what F6's gate would have to
  allowlist. `describe` keeps one `.0`, inside its `format!` — the display
  leaf, the same reading as `default_vein`'s arithmetic leaf in the harvest
  stage.
- **`world::tick/travel.rs`**: `travel_to` and `can_stand_at` → `Facet`, and
  with them five `Facet(facet)` rewraps and two `.0`s vanish from `mark_rune`
  (whose `facet` came from `state.facet_of(caster)` all along).
- **`world::tick/gates.rs`**: `open_gate_to`, `spawn_gate`, `gate_at`,
  `public_gate_entity`, `travel_through` → `Facet`. `spawn_gate`'s
  `registry.insert(entity, Facet(facet))` became `insert(entity, facet)` —
  the component it inserts *is* `Facet`, so the parameter had been unwrapped
  purely to be wrapped again on the next line.
- **`world::tick/regions.rs`**: the private `Crossing.facet` and the public
  `World::region_at` → `Facet`; `restore_regions`'s `BTreeMap` is keyed by
  `Facet` with the conversion moved to where it reads the record.
  `find_crossings`'s `seen.facet == facet.0` is now `== facet`, which is the
  comparison the field's own doc comment argues for.
- **`world::events.rs`: `RegionChanged.facet` → `Facet`.** Its only readers
  are `guard_crossings` and two tests, so it went with its producer.

**Left bare on purpose: `register_regions` and `clear_regions`.** Both are
handlers for a `Command` variant whose field is still a `u8`, so wrapping them
would move the `.0` into `tick.rs`'s dispatch `match` and buy nothing — the
`spawn_item` finding again, third instance. A comment on `clear_regions` says
so in the source, since that is where the next reader will ask.

Counts: `state/components.rs` 4 → 0. `magic/travel.rs` 5 → 0. `world/tick/
travel.rs` 2 → 0, `gates.rs` 5 → 0, `regions.rs` 4 → 2 (the two `Command`
handlers), `events.rs` 1 → 0. Call sites updated: `world/tick/travel_tests.rs`
(21 lines), `server/src/scripting.rs` (3, `World::region_at` being `pub`),
`items/src/drag.rs` (0 — both sides of `RunebookEntry { facet: mark.facet }`
moved together, which is the pilot's shape at its purest).

## `Facet` gained `Display` — done

Closed the backlog item found while doing the travel stage above: `Facet` had
neither `Display` nor `tracing::Value`, which is why three `.0`s survived that
were not seams — `magic::describe`'s `format!`, and `restore_regions`'s two
log lines, written `facet = facet.0` because `warn!(facet, ..)` needs
`tracing::Value` and the newtype had neither. `impl std::fmt::Display for
Facet` (`crates/common/protocol/src/world.rs`, next to the type) settles both:
`describe`'s `format!` now interpolates `facet` directly, and both
`restore_regions` log lines take it through `%facet` — the syntax `tracing`
accepts for any `Display` field. `gm.rs`/`spawner.rs`'s own `facet.0` log
sites are `world`'s own stage, not touched here, same as every other bare
`facet: u8` in those files. Done before `world`'s stage starts, per the
backlog note's own reasoning: that stage's log lines get written once now.

## `scripting` and the rest of `world` — done, together, as predicted

Landed in one stage, because the dependency the previous section named held
exactly as expected: `scripting`'s `Command` fields and `world::tick/
command.rs`'s own `Command` enum feed each other through the `into_world`
bridge (`server/src/scripting.rs`), so converting one side without the other
would only have moved the `.0`, not removed it.

- **`Facet` gained `#[serde(transparent)]` `Deserialize`/`Serialize`** — the
  F4 answer applied. `scripting::lib.rs`'s `Command` enum (8 fields),
  `engine/ops.rs`'s seven `*Spec` structs (`SpawnSpec`, `ContainerSpec`,
  `MobileSpec`, `SpawnerSpec`, `RegionsSpec`, `DecorSpec`, `DoorRegionSpec`)
  and `server/src/scripting.rs`'s `into_world` bridge all took `Facet`
  directly; the `spec.facet` forward sites collapsed to plain field-shorthand,
  no `Facet(spec.facet)` wrap left anywhere. `op_clear_regions` stays the one
  bare `facet: u8` — the F4 exception, wrapped at its own push site
  (`Facet(facet)`) rather than upstream.
- **`world::tick/command.rs`'s `Command` enum**: all seven remaining variants
  (`SpawnItem`, `SpawnContainer`, `SpawnMobile`, `RegisterSpawner`,
  `RegisterRegions`, `ClearRegions`, `Decorate`, `GenerateDoors` — eight
  counting `GenerateDoors` separately) → `Facet`. `tick.rs`'s dispatch `match`
  needed no changes at all beyond the type flowing through: every arm already
  destructured `facet` by shorthand and passed it straight to a handler.
- **Every handler `Command`'s fields fed, unblocked in the same commit**:
  `World::register_regions`, `clear_regions` (its explanatory "bare on
  purpose" comment removed — it is no longer true), `decorate`,
  `generate_doors`, `place_decoration`, `spawn_field_tile`, `spawn_corpse`,
  `with_facet` (public API, `server/src/boot.rs`'s one call site wraps the
  config-read `u8` at the boundary) → `Facet`. `items::spawn_item` and
  `spawn_container` (`npc`/`items`'s SpawnSpec.facet` was blocked on exactly
  these) → `Facet`, unblocking `npc::SpawnSpec.facet` and
  `items::spawn.rs`'s remaining two signatures the same session.
  `spawner::SpawnArea.facet` → `Facet` too, found the same way `state`'s four
  component fields were: not in the original survey because it is a struct
  field a signature grep does not see. `SpawnerRecord.facet` (the disk
  record) stays `u8`, per F2 — `tick/spawners.rs`'s save/restore pair wraps
  and unwraps at exactly that seam.
- **Every caller updated**: `gm.rs`'s six admin-command call sites,
  `skills::handlers::harvest.rs`'s one, `npc::guards.rs`'s `make_guard`
  (dropped its `facet: facet.0`), and three `pub(super)` test helpers in
  `world/tick/tests.rs` (`add_empty_facet`, `add_empty_facet_sized`,
  `enter_on_facet`) whose ~20 call sites across `tests.rs`/`travel_tests.rs`/
  `region_tests.rs` all took the wrap at the literal.

Counts: `scripting/lib.rs` 8 → 0, `engine/ops.rs` 8 → 1 (`op_clear_regions`,
allowlisted), `world/tick/command.rs` 7 → 0, `spawner.rs` 1 → 0 (plus
`SpawnerRecord.facet` in `persistence`, unchanged, F2), `tick/{decor,fields,
death}.rs` and `tick.rs::with_facet` all 0 bare signatures remaining.

## The gate — added

`crates/common/protocol/tests/facet_bare_fields.rs`, per F6 and "The gate"
above: a fixed allowlist of `(file, occurrence count, reason)`, checked
against a workspace-wide text walk. Seven files remain on it, all F2/F4
carve-outs or their "follows its crate's fix" examples — `persistence::
{record,sqlite,pg}.rs` (the disk seam), `uofiles::map.rs` and its
`examples/tile_probe.rs` (the raw-index/filename argument), `client/render/
examples/shard/mod.rs` (a standalone diagnostic tool with no `protocol`
dependency, reading a SQL column the same way `record.rs` does), and
`scripting::engine/ops.rs`'s one `op_clear_regions` (F4). Every other
`facet: u8` in the workspace is gone. The sweep this plan opened is closed;
future stages are whatever the allowlist's reasons stop being true for.

## What's next

Nothing scheduled. The workspace-wide sweep is done: every `facet: u8` this
plan's survey found, and every one later stages turned up that the survey
missed (`state::components.rs`'s four fields, `magic::travel.rs`,
`spawner::SpawnArea`, three test helpers), is either `Facet` or on the gate's
allowlist for a reason recorded there. Should a new bare `facet: u8` appear —
a new packet field, a new `Command` variant, a new spec struct — the gate
catches it on the next `cargo test`; F1–F6 above are the decisions that tell
whoever fixes it where it belongs.
