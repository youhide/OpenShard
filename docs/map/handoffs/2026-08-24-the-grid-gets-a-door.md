# 2026-08-24 — the grid gets a door

The follow-up [the bucket left](2026-08-24-a-bucket-is-two-lists.md), and the
only one of its four it named with a flag: **`FacetState::sectors` was public and
written from forty-five places in six crates.** That entry did not do it on
purpose — it was a second refactor over the same call sites, and doing both at
once would have hidden which change broke what. It is done now, and the shape it
took is not the one the backlog guessed.

## Where it stands

### The field is private, and there are exactly three writes ✅

`FacetState::sectors` is private, read through `FacetState::sectors()`. The
writes are three methods on `WorldState`:

| | sites | what it is |
|---|---|---|
| `place_mobile(facet, entity, at)` | 12 | a body: a player entering, a creature spawned, a step, a teleport, a dismounted horse |
| `place_item(facet, entity, at)` | 19 | everything else on the ground: an item, a corpse, a door, a house, a ship, a moongate, a spell field |
| `unplace(facet, entity)` | 12 | the removal — a despawn, a decay, an item picked up, a mount ridden, a boat sunk, a traveller leaving a facet |

Forty-three, plus the two neighbours of the field that were already private for
the same reason: `obstructions` and `boats`, which are written through
`FacetState::block` / `unblock` / `moor` / `cast_off`. The argument the struct
already made about them — *"every write here has to be followed by …, and a
public field is a way to forget"* — is now made about all three.

The forgettable half here was `remove`, and it is worth naming what forgetting it
costs: a row left in the grid is not stale data sitting quietly. It is handed to
every lookup that passes over that tile, for as long as the shard runs, and
`position_of` keeps swearing the thing is there.

### Two calls, not one call with an argument 🚩

The backlog proposed `place(entity, facet, at, occupant)`. That was refused, and
the reason is the second half of the same sentence it was written in: the seam is
*where `Occupant` is named once per kind of thing rather than once per call
site*. A single method with an `Occupant` argument moves the naming without
reducing it — thirty-one call sites would still spell the variant.

So it is two methods, and `Occupant::Mobile` and `Occupant::Item` each appear
exactly once in the shard outside `sectors.rs`. **This keeps the previous
session's rule intact rather than weakening it.** The kind is still *declared* by
the caller and never derived from the registry — a caller declares it by which of
the two it reaches for. It cannot go stale for the same reason it could not
before: nothing reads a component to decide.

### The asymmetry, re-run against the seam

The previous entry's controls were run by hand at one call site each. The seam is
the thing that makes them cheap, so they were re-run against it — one line, and
it moves every site of that kind at once:

| the control | what fails |
|---|---|
| `place_mobile` files `Occupant::Item` | **64** tests: sight, chat, guards, the chase, death |
| `place_item` files `Occupant::Mobile` | **1** — `the_shard_files_what_it_spawns_as_what_it_is`, on its corpse assertion |

Same asymmetry, same reason: a body in the item list is invisible to everything
that looks for people, and an item in the mobile list is merely wasteful. What
changed is that there is now one place to get it wrong instead of forty-three,
and that place is loud.

### A facet's extent stopped being four pokes

Housing's `britannia_with` fixture built a 32×32 facet and then wrote `width`,
`height`, a fresh `Sectors` and a fresh `Regions` over it — four public-field
assignments to resize three indexes that are all sized from the same pair, which
is precisely what `FacetState::new`'s doc says it exists to make unspellable. It
takes the extent as an argument now (`ground_sized`), and the fixture is one
call.

## What was decided

**The seam is on `WorldState`, not beside `block`/`unblock` on `FacetState`.**
The sibling precedent argued the other way and was overruled by the call sites:
every one of the forty-three holds a `&mut WorldState` and a `Facet`, and routing
through `facet_state_mut(facet)` at each of them is exactly what made the write
*look* like a field poke. The readers of this index — `crowd_near`,
`broadcast_from`, `reveal`, `refresh_around` — are already on `WorldState`, so
its readers and its writers are now in one place. `block`/`unblock` stay where
they are: their callers genuinely hold a `FacetState` (housing's
`block_footprint` takes one).

**`Sectors::insert` and `remove` stay `pub`.** The field being private is what
makes the pair unbypassable; narrowing the methods to `pub(crate)` on top of that
would buy nothing and would make `Sectors` a public type whose mutators are
invisible in its own docs. This is what `Obstructions::block` already does beside
it — public method, private field, one seam — and matching it was worth more than
the extra word.

**The three methods write the grid and nothing else.** They do *not* also write
`Position` and the facet component, though nearly every call site writes all
three together and the trio is plainly the real forgettable unit. That is a
different refactor with a different blast radius — some sites write `Position`
several statements earlier, with events in between — and folding it in here would
have repeated exactly the mistake the previous entry avoided by not doing this
one.

## What is clean

`cargo test --workspace`: **3,524 passed, 0 failed**, 36 ignored — the same count
as before the change, which is the claim: this moves no behaviour.
`cargo clippy --workspace --all-targets`: the same five findings in the same
three files, none this session's — `uofiles/src/map.rs`, `render/tests/traced.rs`
×3, `client/app/src/link.rs`. `cargo fmt --all` silent.

Every rewritten site was diffed kind-for-kind before the suite was trusted: 19
`Occupant::Item` → `place_item`, 12 `Occupant::Mobile` → `place_mobile`, 12
`sectors.remove` → `unplace`, and no site changed which list it files into.

## What is next

The three the bucket left, minus the one this closed, plus what this one found.

| | what would close it |
|---|---|
| **`move_to`'s signature still does not say "a mobile"** | It calls `place_mobile` now, so the fact is written down twice — in the doc and in the call — and enforced by neither. An item put through it lands in the mobile list and vanishes from the crafting scan, the item list's one reader. What would fix it is a `move_item_to` beside it, or a kind on the signature; both want a caller who needs it, and there is not one yet. |
| **`aboard` sweeps a square around the ship's first covered tile** | Unchanged. A galleon moored east-west sweeps a box as wide as it is long in both axes. Mobiles-only now, which is most of the cost gone by accident; the shape is still wrong and the deck test would not notice. |
| **The shove** | Unchanged — `Mobile.CheckShove`, four rules and two clilocs, still blocked first on this engine having no facet rulesets. |
| **Two bodies on a deck that moves under them** | Unchanged, still unexamined at both ends. |
| The three the client's half left | [the 08-23 entry](2026-08-23-the-other-end-of-the-wire-gets-the-rule.md) |

And one this session made:

- **`FacetState::regions`, `width` and `height` are still public**, and the
  fixture change above emptied the case for two of them: after it, **nothing in
  the shard assigns `width` or `height`**. They are set by `FacetState::new` and
  read by the two packets that tell a client the map size (`0x1B` at login,
  `0x76` at a facet change) — a pair of accessors and they are shut, with no call
  site to rewrite.

  `regions` is the weaker case and worth saying why: it is a public field, but
  its writes already go through `Regions::set` / `Regions::clear` at four sites,
  so the type carries its own seam and the field only leaks `&mut` to it. That is
  not the `sectors` defect, where the field *was* the API. Worth tidying by
  whoever next touches `FacetState`; not worth a refactor of its own.
