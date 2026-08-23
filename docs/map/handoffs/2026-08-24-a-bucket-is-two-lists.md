# 2026-08-24 — a bucket is two lists

The first of the three [the other end of the wire
left](2026-08-23-the-other-end-of-the-wire-gets-the-rule.md), which is the one
that entry called *unchanged* because it had been sitting in the roadmap since
before the mobile obstacle made it worse: **a sector lookup walked every entry in
a bucket, and a decorated house is four thousand entries.** It is closed the way
that backlog said to close it, and what the closing found is that the backlog had
one of its two item readers wrong.

## Where it stands

### The read was the linear half, and the callers were the reason ✅

`Sectors` was already O(1) to insert, move and remove — `located` holds an
entity's bucket *and* its row, which was the lesson learned from this exact case
the last time. What stayed linear was the read: `nearby` walked every entry of up
to four buckets. Correct, and cheap while a bucket held mobiles.

Housing's own caps say why it stopped being cheap. `LOCKDOWNS_PER_TILE` is 4, so
a castle's 992 tiles are about **4,000 locked-down items**, and at 64 tiles a
side that castle is one or two buckets. Every lookup touching it compared four
thousand rows — and the lookups are per NPC per tick (AI sight), per line spoken
(chat), per guard call, per pet whistle, per area spell, and, since the mobile
obstacle closed, **per step by anyone** (`crowd_near`). The bill landed on the
NPC standing in the street outside somebody's keep, never on the keep.

### Nineteen readers, and seventeen of them wanted mobiles

| what it wants | who |
|---|---|
| **mobiles** (17) | `crowd_near`, `broadcast_from`, `audience_of`, `reveal`, AI sight, both area spells, a field's victims, a sector waking, pets, guards, quest listeners, a ship's manifest, chat, both stealth sweeps, the bard's audience |
| **items** (1) | the crafting workshop scan — a forge is furniture, and the smith standing beside it is not what is being asked |
| **both** (1) | `refresh_around`, which fills a *screen*: a player walking up to a house has to be sent the house |

So: `mobiles_near`, `items_near`, `everything_near`, `mobiles_in_block` — and
**`nearby` is gone as a name.** That is deliberate rather than tidy: a rename
forces every call site to be revisited, where keeping the old name would have let
any of them keep the old cost by inheritance.

**The roadmap had one of them wrong.** It named `tick/fields.rs` as an item
reader. A field damages whoever *stands on it* and filtered its sweep by `Body` —
a mobile reader all along, and one of six sites paying for the furniture twice,
once to walk it and once to reject it.

### 🚩 Six callers re-asked a question the lookup had already answered

`nearby`'s doc said "Exact: … nothing outside `range` comes back", and chat,
both stealth sweeps, the bard, a guard's call and the AI's sight each filtered by
Chebyshev distance again anyway. Harmless, and a Chebyshev per candidate on paths
that run per tick. They went with the rename, and `mobiles_near`'s doc now says
the quiet part: *a caller that filters by distance again is asking a question
this already answered.*

## What was decided

**The kind is declared at the insert and never derived.** `Occupant::Mobile` /
`Occupant::Item`, named at each of the twenty-five places the shard puts
something on the grid (and seven more in tests). The fact does exist in the
registry — a mobile carries a `Body`, a ground item a `Drawn`, one or the other
and never both — and reading it *inside* the index was the alternative. It was
refused because it makes the answer depend on **whether the component went on
before the index did**, which is a bug that appears in whichever spawn path
somebody reorders next year, in one subsystem, silently. Every caller already
knows what it is placing; saying so costs a word and cannot go stale.

**One row per entity, whichever list it is in.** The obvious alternative to two
lists in a bucket is two grids side by side, which reads even better at the call
site — and would let two callers file one entity in both, forever. `located`
makes that impossible: an entity handed to `insert` under a different `Occupant`
is *moved*, by the same mechanism that moves it between sectors. There is a test
for it, because it is the bug the split invented.

**A corpse is an item and a dismounted horse is a mobile.** The two edges. A
corpse carries a body *graphic* and is a container; a mount is an item on a layer
while it is ridden and a body again the moment it is not — that insert is the one
place in the shard where the same entity legitimately changes lists.

**The redundant `Body` checks came out where the list already says it.** The
index is a copy of a registry fact kept honest by the tick, which is the bargain
`Sectors` already makes for `Position` — reading the kind off it is the same
bargain, not a weaker one. `crowd_near` keeps `body_blocks`, because that asks
something the list cannot: a body in the list can still be dead, or a hidden game
master, and be in nobody's way.

## What is clean

`cargo test --workspace`: **3,524 passed, 0 failed**, 36 ignored — one new test in
the world crate and three in `sectors.rs`.
`cargo clippy --workspace --all-targets`: the same five findings in the same
three files, none this session's — `uofiles/src/map.rs`, `render/tests/traced.rs`
×3, `client/app/src/link.rs`. `cargo fmt --all` silent.

**The controls were run by hand**, and they are asymmetric, which is the thing to
remember:

| the control | what fails |
|---|---|
| the corpse filed as `Occupant::Mobile` | the new guard, on its corpse assertion, and nothing else |
| an entering player filed as `Occupant::Item` | **fifty** tests across sight, chat, guards, the chase and death |

A body in the item list is invisible to everything that looks for people. An item
in the mobile list is merely wasteful. The suite already had the first direction
covered fifty ways; the new test is there for the second, and it holds the grid
against the *registry* — the real spawn paths run (a player entering, a creature
spawned, an item and a container placed, a corpse left by a death) and every row
of both lists has to agree with the `Body` the registry has.

## What is next

The two the previous entry left, unchanged, plus what this one found.

| | what would close it |
|---|---|
| **The shove.** A player hard-blocked where UO would have let them past for 10 stamina | `Mobile.CheckShove`, four rules and two clilocs. Still wants an owner for "may I walk into somebody" as a *gameplay* question, and still blocked first on a thing that is not the shove: this engine has no facet rulesets, and the rule's first branch is ServUO's `MapRules.FreeMovement` |
| **Two bodies on a deck that moves under them** — still simply unexamined, at both ends | — |
| The three the client's half left: its crowd built per ask against its clutter per view, a living mobile wearing a ghost graphic, and the `0x20`'s half-ignored flag byte | [the previous entry](2026-08-23-the-other-end-of-the-wire-gets-the-rule.md) |

And four this session made, filed in [`roadmap.md`](../../roadmap.md) under the
entry this closed:

- 🚩 **`FacetState::sectors` is public and written from forty-five places in six
  crates.** Its two neighbours in the same struct are private on an argument that
  applies to it word for word — "a public field is a way to forget". What is
  forgettable here is `remove`. A `WorldState::place` / `unplace` pair is the
  seam, and it is where `Occupant` would be named once per kind of thing rather
  than once per call site. Not done here: it is a second refactor over the same
  sites, and doing both at once would have hidden which change broke what.
- **`move_to` files its traveller as a mobile, and its callers make that true
  rather than its signature.** All six are bodies. An item put through it would
  land in the mobile list and vanish from the one reader of the item list.
- **`aboard` sweeps a square around the ship's first covered tile**, so a
  galleon moored east-west sweeps a box as wide as it is long in both axes. It is
  mobiles-only now, which is most of the cost gone by accident; the shape is
  still wrong.
- **One full-suite run reported a single failure with no name captured**, and
  three consecutive runs since have been clean. Nothing to chase without the
  panic line — recorded so whoever sees the next one knows it is not the first.
