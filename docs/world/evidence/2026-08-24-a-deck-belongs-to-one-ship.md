# 2026-08-24 — a deck belongs to one ship

Two entries from [the grid's door](2026-08-24-the-grid-gets-a-door.md): the one
it left for whoever came next, and the one it filed as a shape complaint. The
second was not a shape complaint.

## Where it stands

### A facet's extent is set once ✅

`FacetState::width` and `height` are private, read through `width()` and
`height()`. The previous entry predicted "no call site to rewrite" and was
right: there were two readers, both the packet that tells a client how big its
map is — `0x1B` at login, `0x76` at a facet change — and each was a one-word
change. Nothing assigned either, because that entry's own fixture change had
emptied the last case.

The invariant is `FacetState::new`'s, restated where it can be enforced: the
sector grid and the region index are sized from this pair, so a facet whose
width is assigned after construction has two indexes that disagree with it and
nothing that would notice.

### `regions` stays public, and now says so 🚩

The previous entry called it "the weaker case and worth saying why", then left
the field itself silent about it — so the next reader would have had to
re-derive the answer from the neighbours, all five of which are private.

It is public on purpose. `Regions` carries its own seam: `set` and `clear` both
rebuild the bucket grid that accelerates `at`, and that grid is private to the
type, so a caller holding `&mut` to the field still cannot leave it disagreeing
with itself. That is not the `sectors` defect, where the **field was the API**.
An accessor pair here would rename the leak rather than close one. That argument
is in the field's doc now, which is the only place it can stop being asked.

### `aboard` was not returning the wrong shape. It was returning the wrong ship 🚩

The backlog had this filed as cost and tidiness — *"a galleon moored east-west
sweeps a box as wide as it is long in both axes … the shape is still wrong and
the deck test would not notice."* Both halves are true and neither is the defect.

The manifest found its candidates with one sector sweep and then decided them
with `Boats::deck_at`, **which answers for the whole facet**. It says *there is a
floor at this tile*, never whose. So a ship under way took as passengers anyone
standing on **any** ship's deck that its sweep reached, and `step` then
translated each of them by its own delta — off their own deck, into the water,
with the sector grid updated to agree.

Two ships is not a hypothetical case this engine merely permits. `Boats` has
always indexed a tile as a list of planks with a `boat` field on each, and
`casting_off_one_boat_leaves_the_other` is a test of two of them sharing one
tile. The field was there the whole time; the manifest was the one reader that
did not look at it.

The scale is the sweep's, and this is the one place the shape complaint was
load-bearing: the reach was the *whole length of the hull* measured from its
north-west corner, so a galleon put twenty-odd tiles of sea on every side of its
bow inside the net.

`Boats::carries(boat, x, y, z)` is the named half of `deck_at`:

| | the question | who asks it |
|---|---|---|
| `deck_at(x, y, near_z)` | what am I standing on | a body taking a step — whose ship it is could not matter less |
| `carries(boat, x, y, z)` | is this one of mine | a ship about to sail |

No "nearest surface" in the second: the body is already at `z`, and standing on
a plank means standing on its top — which is exactly what `deck_at(..) ==
Some(z)` was testing, since a surface at `z` is the unique nearest one to `z`.

### The sweep is the berth's own box now, and that is *not* what fixed it

Centred on the bounding box rather than on `covered.first()`, with the radius
half the longer span rather than all of it. **The control says plainly that this
did not fix anything**, and it is worth writing down because the tempting
account of this session is that tightening the shape closed the hole:

| the control | what happens |
|---|---|
| new geometry, old `deck_at` filter | the sailor is **still** dragged — 3 tiles is still inside a radius of 4 |
| old geometry, new `carries` filter | correct |

The box is worth having anyway, and the reason is exactly this defect: the
surplus of a sweep is where the wrong answers live, and a sweep whose surplus is
twenty tiles of open sea is one nothing was ever going to notice was too big.

### The fixture needed a longer ship

A three-tile sloop cannot express the property `aboard` is sensitive to — how
far the far end of a ship is from the near end — because at that size *every*
sweep containing the sloop contains barely more than the sloop. So the fixture
grew a **galley**: eight deck tiles in a line, no hull, which is the shortest
thing that behaves like a galleon. The bug is invisible with the sloop alone,
which is why sixteen boat tests did not have it.

Adding it broke `a_multi_that_is_not_a_ship_is_refused_and_leaves_nothing`,
which spelled its unknown id `SLOOP + 1` — making "an id nobody knows" a
statement about the *neighbourhood* of a known one. It is `UNKNOWN_MULTI` now.

## What is clean

`cargo test --workspace`: **3,527 passed, 0 failed**, 36 ignored — three more
than the previous entry's 3,524, which are the three tests added here.
`cargo clippy --workspace --all-targets`: the same five findings in the same
three files, none this session's — `uofiles/src/map.rs`,
`render/tests/traced.rs` ×3, `client/app/src/link.rs`. `cargo fmt --all` silent.

## What is next

| | what would close it |
|---|---|
| **`move_to`'s signature still does not say "a mobile"** | Unchanged from the previous entry. It calls `place_mobile`, so the fact is written twice and enforced by neither; an item put through it lands in the mobile list and vanishes from the crafting scan. Wants a caller who needs `move_item_to`, and there is still not one. |
| **The shove** | Unchanged — `Mobile.CheckShove`, four rules and two clilocs, still blocked first on this engine having no facet rulesets. |
| **Two bodies on a deck that moves under them** | Unchanged, still unexamined at both ends. **Now the more interesting of the two remaining boat items**, because the one beside it turned out to be a real defect rather than tidying — this one is the last unexamined claim in `step`. |
| The three the client's half left | [the 08-23 entry](2026-08-23-the-other-end-of-the-wire-gets-the-rule.md) |

And what this session found:

- **`step` moves the manifest with no order and no collision check.** Each
  occupant is translated by the ship's delta and written, one after another,
  and nothing asks whether two of them land on the same tile — which is the
  "two bodies on a deck" item above, now with a specific place to look. The
  translation is uniform, so two bodies *cannot* collide with each other by
  sailing; what is unexamined is a body that was already sharing a tile with
  another, and a body whose destination has something else standing on it that
  the ship is not carrying.

- **`aboard` is `O(mobiles in up to four sectors)` per ship per step, and the
  sweep is the only reason.** With `carries` doing the deciding, the exact
  answer is "the mobiles standing on these N tiles", and the index that could
  answer it directly does not exist — `Sectors` is keyed by bucket, not by
  tile. Not worth building for a handful of ships; worth naming, because the
  next reader will wonder why a ship asks a neighbourhood a question about
  itself.

- **`check_course` and `check_berth` differ by one comparison and say so in
  prose.** `check_berth` refuses any boat in the tile; `check_course` refuses
  any boat that is not this one. Two nearly identical loops over a berth, each
  walking water and boats, with a doc comment on the second explaining that the
  difference "is one comparison and it is the whole of why a ship can sail
  forward". That is a parameter wearing a comment — `check(state, facet,
  berth, ignoring: Option<EntityId>)` would say it in the signature. Left
  alone: it is two callers and the prose is currently correct, which is the
  weakest kind of case for touching working code.
